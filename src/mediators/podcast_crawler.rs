use error_helpers;
use errors::*;
use http_requester::HTTPRequesterFactory;
use mediators::common;
use mediators::podcast_updater::PodcastUpdater;

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

pub struct PodcastCrawler {
    // Number of workers to use. Should generally be the size of the thread pool minus one for the
    // control process.
    pub num_workers: u32,

    pub pool:                   Pool<ConnectionManager<PgConnection>>,
    pub http_requester_factory: Box<HTTPRequesterFactory>,
}

impl PodcastCrawler {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |log| self.run_inner(log))
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
                    .spawn(move || {
                        work(&log, &pool_clone, &*factory_clone, &work_recv_clone);
                    })
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

        Ok(RunResult {
            num_podcasts: num_podcasts,
        })
    }

    // Steps
    //

    fn page_podcasts(&mut self, log: &Logger, work_send: &Sender<PodcastTuple>) -> Result<i64> {
        let log = log.new(o!("thread" => "control"));
        common::log_timed(&log.new(o!("step" => "page_podcasts")), |log| {
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

                for podcast in &podcasts {
                    work_send.send(podcast.clone());
                }

                last_id = podcasts[podcasts.len() - 1].id;
                num_podcasts += podcasts.len() as i64;
            }

            Ok(num_podcasts)
        })
    }

    fn select_podcasts(
        log: &Logger,
        conn: &PgConnection,
        start_id: i64,
    ) -> Result<Vec<PodcastTuple>> {
        let res = common::log_timed(
            &log.new(o!("step" => "query_podcasts", "start_id" => start_id)),
            |_log| {
                // See comment on similar function in podcast_reingester -- unfortunately
                // Diesel's query DSL cannot handle subselects.
                diesel::sql_query(
                    "
                        SELECT id,
                            (
                               SELECT feed_url
                               FROM podcast_feed_location
                               WHERE podcast_feed_location.podcast_id = podcast.id
                               ORDER BY last_retrieved_at DESC
                               LIMIT 1
                            )
                        FROM podcast
                        WHERE id > $1
                            AND last_retrieved_at <= NOW() - $2::interval
                        ORDER BY id
                        LIMIT $3",
                ).bind::<BigInt, _>(start_id)
                    .bind::<Text, _>(REFRESH_INTERVAL)
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

const PAGE_SIZE: i64 = 100;

// Target interval at which we want to refresh every podcast feed.
//
// Should be formatted as a string that's coercable to the Postgres interval
// type.
static REFRESH_INTERVAL: &'static str = "1 hours";

//
// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a
// struct.
#[derive(Clone, Debug, QueryableByName)]
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
    http_requester_factory: &HTTPRequesterFactory,
    work_recv: &Receiver<PodcastTuple>,
) {
    let conn = match pool.try_get() {
        Some(conn) => conn,
        None => {
            error!(
                log,
                "Error acquiring connection from connection pool (is num_workers misconfigured?)"
            );
            return;
        }
    };
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

                let res = PodcastUpdater {
                    conn: &*conn,
                    // Allow the updater to short circuit if it turns out the podcast doesn't need
                    // to be updated
                    disable_shortcut: false,
                    feed_url:    feed_url,
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
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use http_requester::{HTTPRequesterFactoryPassThrough, HTTPRequesterPassThrough};
    use mediators::podcast_crawler::*;
    use mediators::podcast_updater::PodcastUpdater;
    use schema;
    use test_helpers;

    use chrono::Utc;
    use r2d2::{Pool, PooledConnection};
    use r2d2_diesel::ConnectionManager;
    use rand::Rng;
    use std::sync::Arc;
    use time::Duration;

    #[test]
    #[ignore]
    fn test_crawler_update() {
        let mut bootstrap = TestBootstrap::new();

        // Insert lots of data to be crawled
        let num_podcasts = (test_helpers::NUM_CONNECTIONS as i64) * 10;
        for _i in 0..num_podcasts {
            insert_podcast(&bootstrap.log, &*bootstrap.conn);
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
    fn test_crawler_no_update() {
        let mut bootstrap = TestBootstrap::new();

        // Just add one podcast given no data will be crawled anyway: any inserted
        // podcasts are marked as last_retrieved_at too recently, so the
        // crawler will ignore them
        insert_podcast(&bootstrap.log, &*bootstrap.conn);

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(0, res.num_podcasts);
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        conn: PooledConnection<ConnectionManager<PgConnection>>,
        log:  Logger,
        pool: Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let conn = pool.get().map_err(Error::from).unwrap();
            TestBootstrap {
                conn: conn,
                log:  test_helpers::log_sync(),
                pool: pool,
            }
        }

        fn mediator(&mut self) -> (PodcastCrawler, Logger) {
            (
                PodcastCrawler {
                    // Number of connections minus one for the reingester's control thread and
                    // minus another one for a connection that a test case
                    // might be using for setup.
                    num_workers:            test_helpers::NUM_CONNECTIONS - 1 - 1,
                    pool:                   self.pool.clone(),
                    http_requester_factory: Box::new(HTTPRequesterFactoryPassThrough {
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

    fn insert_podcast(log: &Logger, conn: &PgConnection) {
        let mut rng = rand::thread_rng();
        PodcastUpdater {
            conn:             conn,
            disable_shortcut: false,

            // Add a little randomness to feed URLs so that w don't just insert one podcast and
            // update it over and over.
            feed_url: format!("https://example.com/feed-{}.xml", rng.gen::<u64>()).to_string(),

            http_requester: &mut HTTPRequesterPassThrough {
                data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
            },
        }.run(log)
            .unwrap();
    }
}
