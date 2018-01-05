use errors::*;
//use url_fetcher::URLFetcher;

use diesel::pg::PgConnection;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

pub struct PodcastReingester {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl PodcastReingester {
    pub fn run(&mut self, _log: &Logger) -> Result<RunResult> {
        Ok(RunResult {})
    }
}

pub struct RunResult {}
