use error_helpers;
use errors::*;
use http_requester::HttpRequesterFactory;
use mediators::common;
use mediators::podcast_updater;
use time_helpers;

use chan;
use chan::{Receiver, Sender};
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::thread;

pub struct Mediator {
    // Number of workers to use. Should generally be the size of the thread pool minus one for the
    // control process.
    pub num_workers: u32,

    pub pool:                   Pool<ConnectionManager<PgConnection>>,
    pub http_requester_factory: Box<HttpRequesterFactory>,
}

impl Mediator {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| self.run_inner(log))
    }

    pub fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let mut workers = vec![];

        let num_podcasts = {
            let (work_send, work_recv) = chan::sync(100);
            for i in 0..self.num_workers {
                let thread_name = common::thread_name(i);
                let log =
                    log.new(o!("thread" => thread_name.clone(), "num_threads" => self.num_workers));
                let pool_clone = self.pool.clone();
                let factory_clone = self.http_requester_factory.clone_box();
                let work_recv_clone = work_recv.clone();

                workers.push(thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || work(&log, &pool_clone, &*factory_clone, &work_recv_clone))
                    .map_err(Error::from)?);
            }

            self.page_podcasts(log, &work_send)?

            // `work_send` is dropped, which unblocks our threads' select, passes them a
            // `None` result, and lets them to drop back to main
        };

        // Wait for threads to rejoin
        for worker in workers {
            let _ = worker.join();
        }

        info!(log, "Finished crawling"; "num_podcast" => num_podcasts);
        Ok(RunResult { num_podcasts })
    }

    //
    // Steps
    //

    fn page_podcasts(&mut self, log: &Logger, work_send: &Sender<PodcastTuple>) -> Result<i64> {
        let log = log.new(o!("thread" => "control"));
        time_helpers::log_timed(&log.new(o!("step" => "page_podcasts")), |log| {
            let conn = &*(self.pool.get().map_err(Error::from))?;

            let mut last_id = 0i64;
            let mut num_podcasts = 0i64;
            loop {
                let podcasts = Self::select_podcasts(log, &*conn, last_id)?;

                // If no results came back, we're done
                if podcasts.is_empty() {
                    info!(log, "All podcasts consumed -- finishing");
                    break;
                }

                last_id = podcasts[podcasts.len() - 1].id;
                num_podcasts += podcasts.len() as i64;

                for podcast in podcasts.into_iter() {
                    work_send.send(podcast);
                }
            }

            Ok(num_podcasts)
        })
    }

    fn select_podcasts(
        log: &Logger,
        conn: &PgConnection,
        start_id: i64,
    ) -> Result<Vec<PodcastTuple>> {
        let res = time_helpers::log_timed(
            &log.new(o!("step" => "query_podcasts", "start_id" => start_id)),
            |_log| {
                // We select into a custom type because Diesel's query DSL cannot handle
                // subselects.
                diesel::sql_query(include_str!("../static/sql/podcast_crawler_select.sql"))
                    .bind::<Text, _>(REFRESH_INTERVAL_SHORT)
                    .bind::<Text, _>(REFRESH_INTERVAL_LONG)
                    .bind::<BigInt, _>(start_id)
                    .bind::<BigInt, _>(JITTER_MINUTES)
                    .bind::<BigInt, _>(PAGE_SIZE)
                    .load::<PodcastTuple>(conn)
            },
        )?;

        Ok(res)
    }
}

pub struct RunResult {
    pub num_podcasts: i64,
}

//
// Private constants
//

// A random number of minutes applied to timestamps when calculating what
// podcasts are ready to be refreshed.
//
// Jitter is integrated to space out big clumps of podcasts that might
// otherwise all get worked at once and generally try to even out the crawler's
// workload as much as possible. This is particularly useful if the `reingest`
// command is ever used because without some jitter all podcast timestamps
// would stay in hourly lockstep.
const JITTER_MINUTES: i64 = 10;

// Work is chunked so that the database fetcher and workers and able to get
// some parallelism. It's also useful for protecting us against the degenerate
// case where the system has been down for a while and everything needs
// crawling simultaneously. That initial fetch might take a significant
// amount of time to come back.
const PAGE_SIZE: i64 = 100;

// Target interval at which we want to refresh podcast feeds that are updated
// infrequently. We back of these a little bit so that we're not incessantly
// crawling feeds that are almost never updated (and which in some cases may
// never be updated again). This interval shouldn't be *too* long because there
// are some high-quality podcasts that almost never see updates, but which we'd
// still like to see new episodes of as soon as possible (e.g., "Hardcore
// History").
//
// Should be formatted as a string that's coercable to the Postgres interval
// type.
static REFRESH_INTERVAL_LONG: &'static str = "1 day";

// Target interval at which we want to refresh podcast feeds that are updated
// relatively frequently.
//
// Should be formatted as a string that's coercable to the Postgres interval
// type.
static REFRESH_INTERVAL_SHORT: &'static str = "1 hour";

//
// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a
// struct.
#[derive(Debug, QueryableByName)]
#[table_name = "podcast"]
struct PodcastTuple {
    #[sql_type = "BigInt"]
    id: i64,

    #[sql_type = "Text"]
    feed_url: String,
}

//
// Private functions
//

fn work(
    log: &Logger,
    pool: &Pool<ConnectionManager<PgConnection>>,
    http_requester_factory: &HttpRequesterFactory,
    work_recv: &Receiver<PodcastTuple>,
) -> Result<()> {
    debug!(log, "Thread waiting for a connection");
    let conn = pool.get()?;
    debug!(log, "Thread acquired a connection");
    let mut http_requester = http_requester_factory.create();

    loop {
        chan_select! {
            work_recv.recv() -> podcast => {
                let podcast: PodcastTuple = match podcast {
                    Some(t) => t,
                    None => {
                        debug!(log, "Received empty data over channel -- dropping");
                        break;
                    }
                };

                let feed_url = podcast.feed_url.to_string();

                let res = podcast_updater::Mediator {
                    conn: &*conn,
                    // Allow the updater to short circuit if it turns out the podcast doesn't need
                    // to be updated
                    disable_shortcut: false,
                    feed_url,
                    http_requester: &mut *http_requester,
                }.run(log);

                if let Err(e) = res {
                    error_helpers::print_error(log, &e);

                    if let Err(inner_e) = error_helpers::report_error(log, &e) {
                        error_helpers::print_error(log, &inner_e);
                    }
                }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use http_requester::HttpRequesterFactoryPassThrough;
    use mediators::podcast_crawler::*;
    use schema;
    use test_data;
    use test_helpers;

    use chrono::Utc;
    use r2d2::{Pool, PooledConnection};
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;
    use time::Duration;

    #[test]
    #[ignore]
    fn test_crawler_short_interval_update() {
        let mut bootstrap = TestBootstrap::new();

        // Insert lots of data to be crawled
        let num_podcasts = (test_helpers::MAX_NUM_CONNECTIONS as i64) * 10;
        for _i in 0..num_podcasts {
            test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);
        }

        // Mark all podcasts as stale so that the crawler will find them
        diesel::update(schema::podcast::table)
            .set(schema::podcast::last_retrieved_at.eq(Utc::now() - Duration::hours(24)))
            .execute(&*bootstrap.conn)
            .unwrap();

        debug!(&bootstrap.log, "Finished setup (starting the real test)");

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(num_podcasts, res.num_podcasts);
    }

    #[test]
    #[ignore]
    fn test_crawler_short_interval_no_update() {
        let mut bootstrap = TestBootstrap::new();

        // Just add one podcast given no data will be crawled anyway: any inserted
        // podcasts are marked as last_retrieved_at too recently, so the
        // crawler will ignore them
        test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(0, res.num_podcasts);
    }

    #[test]
    #[ignore]
    fn test_crawler_long_interval_update() {
        let mut bootstrap = TestBootstrap::new();

        test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);

        diesel::update(schema::podcast_feed_content::table)
            .set(schema::podcast_feed_content::retrieved_at.eq(Utc::now() - Duration::weeks(52)))
            .execute(&*bootstrap.conn)
            .unwrap();

        // Mark podcast as *very* stale so that the crawler will find them despite it
        // not having been updated in a long time.
        diesel::update(schema::podcast::table)
            .set(schema::podcast::last_retrieved_at.eq(Utc::now() - Duration::weeks(1)))
            .execute(&*bootstrap.conn)
            .unwrap();

        debug!(&bootstrap.log, "Finished setup (starting the real test)");

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(1, res.num_podcasts);
    }

    #[test]
    #[ignore]
    fn test_crawler_long_interval_no_update() {
        let mut bootstrap = TestBootstrap::new();

        test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);

        diesel::update(schema::podcast_feed_content::table)
            .set(schema::podcast_feed_content::retrieved_at.eq(Utc::now() - Duration::weeks(52)))
            .execute(&*bootstrap.conn)
            .unwrap();

        // Mark podcast as somewhere between the refresh intervals of our long and
        // short intervals (with a good bit of padding to avoid non-determinism
        // that might be caused by jitter). not having been updated in a long
        // time.
        diesel::update(schema::podcast::table)
            .set(schema::podcast::last_retrieved_at.eq(Utc::now() - Duration::hours(12)))
            .execute(&*bootstrap.conn)
            .unwrap();

        debug!(&bootstrap.log, "Finished setup (starting the real test)");

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(0, res.num_podcasts);
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
        pool:    Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let conn = pool.get().map_err(Error::from).unwrap();
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                conn:    conn,
                log:     test_helpers::log_sync(),
                pool:    pool,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    // Number of connections minus one for the reingester's control thread and
                    // minus another one for a connection that a test case
                    // might be using for setup.
                    num_workers:            test_helpers::MAX_NUM_CONNECTIONS - 1 - 1,
                    pool:                   self.pool.clone(),
                    http_requester_factory: Box::new(HttpRequesterFactoryPassThrough {
                        data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
                    }),
                },
                self.log.clone(),
            )
        }
    }

    impl Drop for TestBootstrap {
        fn drop(&mut self) {
            test_helpers::clean_database(&self.log, &*self.conn);
        }
    }
}
