use error_helpers;
use errors::*;
use http_requester::HttpRequesterPassThrough;
use mediators::common;
use mediators::podcast_updater;
use time_helpers;

use chan;
use chan::{Receiver, Sender};
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Bytea, Text};
use flate2::read::GzDecoder;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::io::prelude::*;
use std::sync::Arc;
use std::thread;

pub struct Mediator {
    // Number of workers to use. Should generally be the size of the thread pool minus one for the
    // control process.
    pub num_workers: u32,

    pub pool: Pool<ConnectionManager<PgConnection>>,
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
                let work_recv_clone = work_recv.clone();

                workers.push(thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || work(&log, &pool_clone, &work_recv_clone))
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

        info!(log, "Finished reingesting"; "num_podcast" => num_podcasts);
        Ok(RunResult { num_podcasts })
    }

    // Steps
    //

    fn page_podcasts(&mut self, log: &Logger, work_send: &Sender<PodcastTuple>) -> Result<i64> {
        let log = log.new(o!("thread" => "control"));
        time_helpers::log_timed(&log.new(o!("step" => "page_podcasts")), |log| {
            let conn = &*(self.pool.get().map_err(Error::from))?;

            let mut last_id = 0i64;
            let mut num_podcasts = 0i64;
            loop {
                let podcast_tuples = Self::select_podcasts(log, &*conn, last_id)?;

                // If no results came back, we're done
                if podcast_tuples.is_empty() {
                    info!(log, "All podcasts consumed -- finishing");
                    break;
                }

                for podcast_tuple in &podcast_tuples {
                    work_send.send(podcast_tuple.clone());
                }

                last_id = podcast_tuples[podcast_tuples.len() - 1].id;
                num_podcasts += podcast_tuples.len() as i64;
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
                // Fell back to `sql_query` because implementing this in Diesel's query language
                // has proven to be somewhere between frustratingly difficult to impossible.
                //
                // First of all, Diesel cannot properly implement taking a single result from a
                // subselect -- it can only take results as `Vec<_>`. I asked in the Gitter
                // channel and the response confirmed the problem, but only
                // relunctantly so, and I wouldn't expect this to get fixed
                // anytime soon.
                //
                // Secondly, even using the `Vec<_>` workaround, I was able to get the
                // subselects to a state where they'd successfully compile, but
                // produce an invalid query at runtime. On debug it turned out
                // that the query was invalid because neither subselect was
                // being wrapped in parentheses (`SELECT ...` instead of `(SELECT
                // ...)`). This might be solvable somehow, but examples in tests and
                // documentation are quite poor, so I gave up and fell back to
                // this.
                diesel::sql_query(
                    "
                SELECT id,
                    (
                       SELECT content_gzip
                       FROM podcast_feed_content
                       WHERE podcast_feed_content.podcast_id = podcast.id
                       ORDER BY retrieved_at DESC
                       LIMIT 1
                    ),
                    (
                       SELECT feed_url
                       FROM podcast_feed_location
                       WHERE podcast_feed_location.podcast_id = podcast.id
                       ORDER BY last_retrieved_at DESC
                       LIMIT 1
                    )
                FROM podcast
                WHERE id > $1
                ORDER BY id
                LIMIT $2",
                ).bind::<BigInt, _>(start_id)
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

// Private constants
//

const PAGE_SIZE: i64 = 100;

// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a
// struct.
#[derive(Clone, Debug, QueryableByName)]
#[table_name = "podcast"]
struct PodcastTuple {
    #[sql_type = "BigInt"]
    id: i64,

    #[sql_type = "Bytea"]
    content_gzip: Vec<u8>,

    #[sql_type = "Text"]
    feed_url: String,
}

// Private functions
//

fn work(
    log: &Logger,
    pool: &Pool<ConnectionManager<PgConnection>>,
    work_recv: &Receiver<PodcastTuple>,
) -> Result<()> {
    debug!(log, "Thread waiting for a connection");
    let conn = pool.get()?;
    debug!(log, "Thread acquired a connection");

    loop {
        chan_select! {
            work_recv.recv() -> podcast_tuple => {
                let podcast_tuple: PodcastTuple = match podcast_tuple {
                    Some(t) => t,
                    None => {
                        debug!(log, "Received empty data over channel -- dropping");
                        break;
                    }
                };

                if let Err(e) = work_inner(log, &*conn, &podcast_tuple) {
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

fn work_inner(log: &Logger, conn: &PgConnection, podcast_tuple: &PodcastTuple) -> Result<()> {
    let mut decoder = GzDecoder::new(podcast_tuple.content_gzip.as_slice());
    let mut content: Vec<u8> = Vec::new();
    decoder.read_to_end(&mut content)?;

    let feed_url = podcast_tuple.feed_url.to_string();

    podcast_updater::Mediator {
        conn,

        // The whole purpose of this mediator is to redo past work, so we need to make
        // sure that we've disabled any shortcuts that might otherwise be enabled.
        disable_shortcut: true,

        feed_url,
        http_requester: &mut HttpRequesterPassThrough {
            data: Arc::new(content),
        },
    }.run(log)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use mediators::podcast_reingester::*;
    use test_data;
    use test_helpers;

    use r2d2::{Pool, PooledConnection};
    use r2d2_diesel::ConnectionManager;

    #[test]
    #[ignore]
    fn test_concurrency() {
        let mut bootstrap = TestBootstrap::new();

        // Insert lots of data to be reingested
        let num_podcasts = (test_helpers::MAX_NUM_CONNECTIONS as i64) * 10;
        for _i in 0..num_podcasts {
            test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);
        }

        debug!(&bootstrap.log, "Finished setup (starting the real test)");

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert_eq!(num_podcasts, res.num_podcasts);
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
                    num_workers: test_helpers::MAX_NUM_CONNECTIONS - 0 - 1,
                    pool:        self.pool.clone(),
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
