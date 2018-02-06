use errors::*;
use mediators::common;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::types::BigInt;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::thread;

pub struct Cleaner {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl Cleaner {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.run_inner(&log)
        })
    }

    pub fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let mut workers = vec![];

        // This is the only cleaner for now, but there will be more!
        {
            let thread_name = "directory_podcast_content_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            workers.push(thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    clean_directory_podcast_content(&log, pool_clone);
                })
                .chain_err(|| "Failed to spawn thread")?);
        }

        // Wait for threads to rejoin
        for worker in workers {
            let _ = worker.join();
        }

        Ok(RunResult {})
    }
}

pub struct RunResult {}

//
// Private constants
//

// The maximum number of objects to try and delete as part of one batch. It's a
// good idea to constrain batch sizes so that we don't have any queries in the
// system that are too long-lived and affect replication and other critical
// facilities.
const DELETE_LIMIT: i64 = 1000;

// The maximum number of content rows to keep around for any given podcast.
const DIRECTORY_PODCAST_CONTENT_LIMIT: i64 = 10;

//
// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a
// struct.
#[derive(Clone, Debug, QueryableByName)]
//#[table_name = "directory_podcast_content"]
struct DirectoryPodcastContentTuple {
    #[sql_type = "BigInt"]
    id: i64,
}

//
// Private functions
//

fn clean_directory_podcast_content(log: &Logger, pool: Pool<ConnectionManager<PgConnection>>) {
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

    loop {
        diesel::sql_query(format!(
            "
                DELETE FROM directory_podcast_content
                WHERE id IN (
                    SELECT id,
                        row_number() OVER (ORDER BY retrieved_at DESC)
                    FROM directory_podcast_content
                    WHERE row_number > {}
                    LIMIT {}
                )
                RETURNING id",
            DIRECTORY_PODCAST_CONTENT_LIMIT, DELETE_LIMIT
        )).load::<DirectoryPodcastContentTuple>(&*conn)?;
    }
}
