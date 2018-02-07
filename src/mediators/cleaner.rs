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
            let thread_name = "podcast_feed_content_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            workers.push(thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    clean_podcast_feed_content(&log, pool_clone);
                })
                .chain_err(|| "Failed to spawn thread")?);
        }

        // Wait for threads to rejoin
        for worker in workers {
            let _ = worker.join();
        }

        // TODO: This should be a real number
        Ok(RunResult { num_cleaned: 0 })
    }
}

pub struct RunResult {
    pub num_cleaned: i64,
}

//
// Private constants
//

// The maximum number of objects to try and delete as part of one batch. It's a
// good idea to constrain batch sizes so that we don't have any queries in the
// system that are too long-lived and affect replication and other critical
// facilities.
const DELETE_LIMIT: i64 = 1000;

// The maximum number of content rows to keep around for any given podcast.
pub const PODCAST_FEED_CONTENT_LIMIT: i64 = 10;

//
// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a
// struct.
#[derive(Clone, Debug, QueryableByName)]
struct DeletePodcastFeedContentBatchResults {
    #[sql_type = "BigInt"]
    count: i64,
}

//
// Private functions
//

fn clean_podcast_feed_content(log: &Logger, pool: Pool<ConnectionManager<PgConnection>>) {
    let conn = match pool.try_get() {
        Some(conn) => conn,
        None => {
            error!(
                log,
                "Error acquiring connection from connection pool (too few max connections?)"
            );
            return;
        }
    };
    debug!(log, "Thread acquired a connection");

    let mut num_cleaned = 0;
    loop {
        let res = delete_podcast_feed_content_batch(log, &*conn);

        if let Err(e) = res {
            error_helpers::print_error(&log, &e);

            if let Err(inner_e) = error_helpers::report_error(&log, &e) {
                error_helpers::print_error(&log, &inner_e);
            }
            break;
        }

        let batch = res.unwrap();
        if batch.count < 1 {
            info!(log, "Cleaned all directory podcast contents"; "num_cleaned" => num_cleaned);
            break;
        }
        info!(log, "Cleaned batch of directory podcast contents"; "num_cleaned" => batch.count);
        num_cleaned += batch.count;
    }
}

fn delete_podcast_feed_content_batch(
    log: &Logger,
    conn: &PgConnection,
) -> Result<DeletePodcastFeedContentBatchResults> {
    common::log_timed(
        &log.new(o!("step" => "delete_podcast_feed_content_batch", "limit" => DELETE_LIMIT)),
        |ref _log| {
            diesel::sql_query(format!(
                "
                    WITH numbered AS (
                        SELECT id,
                            ROW_NUMBER() OVER (ORDER BY podcast_id, retrieved_at DESC)
                        FROM podcast_feed_content
                    ),
                    batch AS (
                        DELETE FROM podcast_feed_content
                        WHERE id IN (
                            SELECT id
                            FROM numbered
                            WHERE row_number > {}
                            LIMIT {}
                        )
                        RETURNING id
                    )
                    SELECT COUNT(*)
                    FROM batch
                    ",
                PODCAST_FEED_CONTENT_LIMIT, DELETE_LIMIT
            )).get_result::<DeletePodcastFeedContentBatchResults>(conn)
                .chain_err(|| "Error deleting directory podcast content batch")
        },
    )
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use mediators::cleaner::*;
    use mediators::podcast_updater::PodcastUpdater;
    use model;
    use model::insertable;
    use schema;
    use test_helpers;
    use url_fetcher::URLFetcherPassThrough;

    use chrono::Utc;
    use r2d2::PooledConnection;
    use rand::Rng;
    use std::sync::Arc;

    #[test]
    #[ignore]
    fn test_clean_podcast_feed_content() {
        let mut bootstrap = TestBootstrap::new();

        let podcast = insert_podcast(&bootstrap.log, &*bootstrap.conn);
        for _i in 0..25 {
            insert_podcast_feed_content(&bootstrap.log, &*bootstrap.conn, &podcast);
        }
        assert_eq!(
            Ok(25 + 1),
            schema::podcast_feed_content::table
                .count()
                .first(&*bootstrap.conn)
        );

        let (mut mediator, log) = bootstrap.mediator();
        let _res = mediator.run(&log).unwrap();

        assert_eq!(
            Ok(PODCAST_FEED_CONTENT_LIMIT),
            schema::podcast_feed_content::table
                .count()
                .first(&*bootstrap.conn)
        );
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

    impl Drop for TestBootstrap {
        fn drop(&mut self) {
            test_helpers::clean_database(&self.log, &*self.conn);
        }
    }

    fn insert_podcast(log: &Logger, conn: &PgConnection) -> model::Podcast {
        let mut rng = rand::thread_rng();
        PodcastUpdater {
            conn:             conn,
            disable_shortcut: false,

            // Add a little randomness to feed URLs so that w don't just insert one podcast and
            // update it over and over.
            feed_url: format!("https://example.com/feed-{}.xml", rng.gen::<u64>()).to_string(),

            url_fetcher: &mut URLFetcherPassThrough {
                data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
            },
        }.run(log)
            .unwrap()
            .podcast
    }

    fn insert_podcast_feed_content(_log: &Logger, conn: &PgConnection, podcast: &model::Podcast) {
        let mut rng = rand::thread_rng();

        let content_ins = insertable::PodcastFeedContent {
            content:      "feed body".to_owned(),
            podcast_id:   podcast.id,
            retrieved_at: Utc::now(),

            // There's a length check on this field in Postgres, so generate a string that's
            // exactly 64 characters long.
            sha256_hash: rng.gen_ascii_chars().take(64).collect(),
        };

        diesel::insert_into(schema::podcast_feed_content::table)
            .values(&content_ins)
            .execute(conn)
            .unwrap();
    }
}
