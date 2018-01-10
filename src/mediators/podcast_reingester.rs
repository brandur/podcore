use errors::*;
use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use url_fetcher::URLFetcherPassThrough;

use chan;
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::types::{BigInt, Text};
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::thread;

pub struct PodcastReingester {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl PodcastReingester {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.run_inner(&log)
        })
    }

    pub fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let mut workers = vec![];

        let (done_send, done_recv) = chan::sync(NUM_THREADS);
        let (work_send, work_recv) = chan::sync(100);
        for id in 0..NUM_THREADS {
            let log = log.new(o!("thread" => id));
            let pool = self.pool.clone();

            workers.push(thread::spawn(move || {
                // TODO: Something better than `unwrap()`.
                let conn = &*(pool.get()
                    .chain_err(|| "Error acquiring connection from connection pool"))
                    .unwrap();

                loop {
                    chan_select! {
                        done_recv.recv() => {
                            break;
                        },
                        work_recv.recv() -> podcast_tuple => {
                            let podcast_tuple: PodcastTuple = podcast_tuple.unwrap();

                            let content = podcast_tuple.content.as_bytes().to_vec();
                            let feed_url = podcast_tuple.feed_url.to_string();

                            let res = PodcastUpdater {
                                conn: &*conn,

                                // The whole purpose of this mediator is to redo past work, so we need to make
                                // sure that we've disabled any shortcuts that might otherwise be enabled.
                                disable_shortcut: true,

                                feed_url:    feed_url,
                                url_fetcher: &mut URLFetcherPassThrough { data: content },
                            }.run(&log);

                            if let Err(e) = res {
                                error!(log, "Error processing podcast: {}", e);
                            }
                        },
                    }
                }
            }));
        }

        let conn = &*(self.pool
            .get()
            .chain_err(|| "Error acquiring connection from connection pool"))?;

        // TODO: Loop in pages.
        let podcast_tuples = Self::select_podcasts(&log, &*conn)?;
        for ref podcast_tuple in &podcast_tuples {
            let x = podcast_tuple.clone();
            work_send.send(*x);
        }

        // Signal done.
        //
        // TODO: This is also possible by allowing the channel to drop. Investigate this later once
        // other things are working ... (Chan's README sort of has an example of this. Look for
        // "sdone".)
        for _ in &workers {
            done_send.send(true);
        }

        // Wait for threads to rejoin
        for worker in workers {
            let _ = worker.join();
        }

        Ok(RunResult {})
    }

    //
    // Steps
    //

    fn select_podcasts(log: &Logger, conn: &PgConnection) -> Result<Vec<PodcastTuple>> {
        let res = common::log_timed(&log.new(o!("step" => "query_podcasts")), |ref _log| {
            // Fell back to `sql_query` because implementing this in Diesel's query language has
            // proven to be somewhere between frustrating difficult to impossible.
            //
            // First of all, Diesel cannot properly implement taking a single result from a
            // subselect -- it can only take results as `Vec<_>`. I asked in the Gitter channel the
            // reponse confirmed the problem, but quite relunctant to, so I wouldn't expect this to
            // get fixed anytime soon.
            //
            // Secondly, even using the `Vec<_>` workaround, I was able to get the subselects to a
            // state where they'd successfully compile, but produce an invalid query at runtime.
            // On debug it turned out that the query was invalid because neither subselect was
            // being wrapped in parentheses (`SELECT ...` instead of `(SELECT ...)`). This might be
            // solvable somehow, but examples in tests and documentation are quite poor, so I gave
            // up and fell back to this.
            diesel::sql_query(
                "
                SELECT id,
                    (
                        SELECT content
                        FROM podcast_feed_contents
                        WHERE podcast_feed_contents.podcast_id = podcasts.id
                        ORDER BY retrieved_at DESC
                        LIMIT 1
                    ),
                    (
                        SELECT feed_url
                        FROM podcast_feed_locations
                        WHERE podcast_feed_locations.podcast_id = podcasts.id
                        ORDER BY last_retrieved_at DESC
                        LIMIT 1
                    )
                FROM podcasts",
            ).load::<PodcastTuple>(conn)
        })?;

        Ok(res)
    }
}

pub struct RunResult {}

//
// Private statics
//

// Put an upper bound on the number of threads to use just so that we don't blow up Postgres with
// inbound connections.
static NUM_THREADS: usize = 3;

//
// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a struct.
#[derive(Clone, Debug, QueryableByName)]
struct PodcastTuple {
    #[sql_type = "BigInt"]
    id: i64,

    #[sql_type = "Text"]
    content: String,

    #[sql_type = "Text"]
    feed_url: String,
}

//
// Private functions
//
