use errors::*;
use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use url_fetcher::URLFetcherFactory;

use chan;
use chan::{Receiver, Sender};
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::types::{BigInt, Text};
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::thread;

pub struct PodcastCrawler {
    // Number of workers to use. Should generally be the size of the thread pool minus one for the
    // control process.
    pub num_workers: u32,

    pub pool:                Pool<ConnectionManager<PgConnection>>,
    pub url_fetcher_factory: Box<URLFetcherFactory>,
}

impl PodcastCrawler {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.run_inner(&log)
        })
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
                let factory_clone = self.url_fetcher_factory.clone_box();
                let work_recv_clone = work_recv.clone();

                workers.push(thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || {
                        work(&log, pool_clone, factory_clone, work_recv_clone);
                    })
                    .chain_err(|| "Failed to spawn thread")?);
            }

            self.page_podcasts(log, work_send)?

            // `work_send` is dropped, which unblocks our threads' select, passes them a `None`
            // result, and lets them to drop back to main
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

    fn page_podcasts(&mut self, log: &Logger, work_send: Sender<PodcastTuple>) -> Result<i64> {
        let log = log.new(o!("thread" => "control"));
        common::log_timed(&log.new(o!("step" => "page_podcasts")), |ref log| {
            let conn = &*(self.pool
                .get()
                .chain_err(|| "Error acquiring connection from connection pool"))?;

            let mut last_id = 0i64;
            let mut num_podcasts = 0i64;
            loop {
                let podcasts = Self::select_podcasts(&log, &*conn, last_id)?;

                // If no results came back, we're done
                if podcasts.len() == 0 {
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
            |ref _log| {
                // See comment on similar function in podcast_reingester -- unfortunately Diesel's
                // query DSL cannot handle subselects.
                diesel::sql_query(format!(
                    "
                SELECT id,
                    (
                       SELECT feed_url
                       FROM podcast_feed_locations
                       WHERE podcast_feed_locations.podcast_id = podcasts.id
                       ORDER BY last_retrieved_at DESC
                       LIMIT 1
                    )
                FROM podcasts
                WHERE id > {}
                    AND last_retrieved_at <= NOW() - '{} hours'::interval
                ORDER BY id
                LIMIT {}",
                    start_id, REFRESH_INTERVAL_HOURS, PAGE_SIZE
                )).load::<PodcastTuple>(conn)
            },
        )?;

        Ok(res)
    }
}

pub struct RunResult {
    pub num_podcasts: i64,
}

// Private constants
//

const PAGE_SIZE: i64 = 100;

static REFRESH_INTERVAL_HOURS: i64 = 1;

// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a struct.
#[derive(Clone, Debug, QueryableByName)]
#[table_name = "podcasts"]
struct PodcastTuple {
    #[sql_type = "BigInt"]
    id: i64,

    #[sql_type = "Text"]
    feed_url: String,
}

// Private functions
//

fn work(
    log: &Logger,
    pool: Pool<ConnectionManager<PgConnection>>,
    url_fetcher_factory: Box<URLFetcherFactory>,
    work_recv: Receiver<PodcastTuple>,
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
    let mut url_fetcher = url_fetcher_factory.create();

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
                    url_fetcher: &mut *url_fetcher,
                }.run(&log);

                if let Err(e) = res {
                    error!(log, "Error processing podcast: {}", e);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use mediators::podcast_crawler::*;
    use mediators::podcast_updater::PodcastUpdater;
    use schema::podcasts;
    use test_helpers;
    use url_fetcher::{URLFetcherFactoryPassThrough, URLFetcherPassThrough};

    use chrono::Utc;
    use rand::Rng;
    use r2d2::{Pool, PooledConnection};
    use r2d2_diesel::ConnectionManager;
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
        diesel::update(podcasts::table)
            .set(podcasts::last_retrieved_at.eq(Utc::now() - Duration::hours(24)))
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

        // Just add one podcast given no data will be crawled anyway: any inserted podcasts are
        // marked as last_retrieved_at too recently, so the crawler will ignore them
        insert_podcast(&bootstrap.log, &*bootstrap.conn);

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(0, res.num_podcasts);
    }

    // Private types/functions
    //

    const MINIMAL_FEED: &[u8] = br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <title>Title</title>
    <item>
      <guid>1</guid>
      <media:content url="https://example.com/item-1" type="audio/mpeg"/>
      <pubDate>Sun, 24 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 1 Title</title>
    </item>
  </channel>
</rss>"#;

    struct TestBootstrap {
        conn: PooledConnection<ConnectionManager<PgConnection>>,
        log:  Logger,
        pool: Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let conn = pool.get()
                .expect("Error acquiring connection from connection pool");
            TestBootstrap {
                conn: conn,
                log:  test_helpers::log_sync(),
                pool: pool,
            }
        }

        fn mediator(&mut self) -> (PodcastCrawler, Logger) {
            (
                PodcastCrawler {
                    // Number of connections minus one for the reingester's control thread and minus
                    // another one for a connection that a test case might be using for setup.
                    num_workers:         test_helpers::NUM_CONNECTIONS - 1 - 1,
                    pool:                self.pool.clone(),
                    url_fetcher_factory: Box::new(URLFetcherFactoryPassThrough {
                        data: Arc::new(MINIMAL_FEED.to_vec()),
                    }),
                },
                self.log.clone(),
            )
        }
    }

    impl Drop for TestBootstrap {
        fn drop(&mut self) {
            debug!(&self.log, "Cleaning database on bootstrap drop");
            (*self.conn)
                .execute("TRUNCATE TABLE podcasts CASCADE")
                .unwrap();
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

            url_fetcher: &mut URLFetcherPassThrough {
                data: Arc::new(MINIMAL_FEED.to_vec()),
            },
        }.run(log)
            .unwrap();
    }
}
