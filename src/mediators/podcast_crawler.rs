use errors::*;
use mediators::common;

use diesel::pg::PgConnection;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

pub struct PodcastCrawler {
    // Number of workers to use. Should generally be the size of the thread pool minus one for the
    // control process.
    pub num_workers: u32,

    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl PodcastCrawler {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.run_inner(&log)
        })
    }

    pub fn run_inner(&mut self, _log: &Logger) -> Result<RunResult> {
        Ok(RunResult {})
    }
}

pub struct RunResult {}
