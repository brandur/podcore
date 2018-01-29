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
mod tests {}
