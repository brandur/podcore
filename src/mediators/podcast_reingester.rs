use errors::*;
use mediators::common;
use schema::{podcast_feed_contents, podcast_feed_locations, podcasts};
//use url_fetcher::URLFetcher;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

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
        let conn = &*(self.pool
            .get()
            .chain_err(|| "Error acquiring connection from connection pool"))?;
        let res = common::log_timed(&log.new(o!("step" => "query_podcasts")), |ref _log| {
            podcasts::table
                .left_outer_join(podcast_feed_contents::table)
                .left_outer_join(podcast_feed_locations::table)
                .order((
                    podcasts::id,
                    podcast_feed_contents::retrieved_at.desc(),
                    podcast_feed_locations::last_retrieved_at.desc(),
                ))
                .select((podcasts::id))
                .load::<(i64)>(conn)
            /*
                .load::<Vec<i64>>(
                    &*(self.pool
                        .get()
                        .chain_err(|| "Error acquiring connection from connection pool")?),
                )
                .select((
                    podcasts::id,
                    podcast_feed_contents::content,
                    podcast_feed_locations::feed_url,
                ))
                .load::<Vec<(i64, String, String)>>(
                    &*(self.pool
                        .get()
                        .chain_err(|| "Error acquiring connection from connection pool")?),
                )
                */
        })?;

        Ok(RunResult {})
    }
}

pub struct RunResult {}
