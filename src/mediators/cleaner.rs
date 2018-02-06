use error_helpers;
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
pub const DIRECTORY_PODCAST_CONTENT_LIMIT: i64 = 10;

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

    let mut num_cleaned = 0;
    loop {
        let res = select_directory_podcast_content_batch(log, &*conn);

        if let Err(e) = res {
            error_helpers::print_error(&log, &e);

            if let Err(inner_e) = error_helpers::report_error(&log, &e) {
                error_helpers::print_error(&log, &inner_e);
            }
            break;
        }

        let batch = res.unwrap();
        if batch.len() < 1 {
            info!(log, "Cleaned all directory podcast contents"; "num_cleaned" => num_cleaned);
            break;
        }
        num_cleaned += batch.len();
    }
}

fn select_directory_podcast_content_batch(
    log: &Logger,
    conn: &PgConnection,
) -> Result<Vec<DirectoryPodcastContentTuple>> {
    common::log_timed(
        &log.new(o!("step" => "select_directory_podcast_content_batch", "limit" => DELETE_LIMIT)),
        |ref _log| {
            diesel::sql_query(format!(
                "
                    WITH batch AS (
                        SELECT id,
                            row_number() OVER (ORDER BY retrieved_at DESC)
                        FROM directory_podcast_content
                        WHERE row_number > {}
                        LIMIT {}
                    )
                    DELETE FROM directory_podcast_content
                    WHERE id IN (
                        SELECT id
                        FROM batch
                    )
                    RETURNING id",
                DIRECTORY_PODCAST_CONTENT_LIMIT, DELETE_LIMIT
            )).load::<DirectoryPodcastContentTuple>(conn)
                .chain_err(|| "Error selecting directory podcast content batch")
        },
    )
}

#[cfg(test)]
mod tests {
    use mediators::cleaner::*;
    use test_helpers;

    use r2d2::PooledConnection;

    #[test]
    fn test_clean_directory_podcast_content() {
        let mut bootstrap = TestBootstrap::new();
        let (mut mediator, log) = bootstrap.mediator();
        let _res = mediator.run(&log).unwrap();
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        conn: PooledConnection<ConnectionManager<PgConnection>>,
        log:  Logger,
        pool: Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let conn = pool.get().map_err(Error::from).unwrap();
            TestBootstrap {
                conn: conn,
                log:  test_helpers::log_sync(),
                pool: pool,
            }
        }

        fn mediator(&mut self) -> (Cleaner, Logger) {
            (
                Cleaner {
                    pool: self.pool.clone(),
                },
                self.log.clone(),
            )
        }
    }
}
