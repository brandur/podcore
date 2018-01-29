use errors::*;
use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use url_fetcher::URLFetcher;

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

    pub pool:        Pool<ConnectionManager<PgConnection>>,
    pub url_fetcher: Box<URLFetcher + Send>,
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
                let url_fetcher_clone = self.url_fetcher.clone_box();
                let work_recv_clone = work_recv.clone();

                workers.push(thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || {
                        work(&log, pool_clone, url_fetcher_clone, work_recv_clone);
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
                // See comment in the function of the same in podcast_ingester
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
                ORDER BY id
                LIMIT 100",
                    start_id
                )).load::<PodcastTuple>(conn)
            },
        )?;

        Ok(res)
    }
}

pub struct RunResult {
    pub num_podcasts: i64,
}

// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a struct.
#[derive(Clone, Debug, QueryableByName)]
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
    url_fetcher: Box<URLFetcher>,
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
    info!(log, "Thread acquired a connection");

    loop {
        chan_select! {
            work_recv.recv() -> podcast => {
                let podcast: PodcastTuple = match podcast {
                    Some(t) => t,
                    None => {
                        info!(log, "Received empty data over channel -- dropping");
                        break;
                    }
                };

                let feed_url = podcast.feed_url.to_string();

                let res = PodcastUpdater {
                    conn: &*conn,

                    // The whole purpose of this mediator is to redo past work, so we need to make
                    // sure that we've disabled any shortcuts that might otherwise be enabled.
                    disable_shortcut: true,

                    feed_url:    feed_url,
                    url_fetcher: url_fetcher,
                }.run(&log);

                if let Err(e) = res {
                    error!(log, "Error processing podcast: {}", e);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {}
