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

        let _podcast_tuples = Self::select_podcasts(&log, &*conn);

        Ok(RunResult {})
    }

    //
    // Steps
    //

    fn select_podcasts(
        log: &Logger,
        conn: &PgConnection,
    ) -> Result<Vec<(i64, Vec<String>, Vec<String>)>> {
        let res = common::log_timed(&log.new(o!("step" => "query_podcasts")), |ref _log| {
            // Note that although in SQL a subselect can be coerced into a single value, Diesel's
            // type system cannot support this. We workaround by storing these values to Vec<_>.
            podcasts::table
                .select((
                    podcasts::id,
                    podcast_feed_contents::table
                        .filter(podcast_feed_contents::podcast_id.eq(podcasts::id))
                        .order(podcast_feed_contents::retrieved_at.desc())
                        .limit(1)
                        .select(podcast_feed_contents::content),
                    podcast_feed_locations::table
                        .filter(podcast_feed_locations::podcast_id.eq(podcasts::id))
                        .order(podcast_feed_locations::last_retrieved_at.desc())
                        .limit(1)
                        .select(podcast_feed_locations::feed_url),
                ))
                .load::<(i64, Vec<String>, Vec<String>)>(conn)
        })?;

        for &(ref _podcast_id, ref content_vec, ref feed_url_vec) in &res {
            assert_eq!(1, content_vec.len());
            assert_eq!(1, feed_url_vec.len());
        }

        Ok(res)
    }
}

pub struct RunResult {}