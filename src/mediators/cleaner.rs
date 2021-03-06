use errors::*;
use time_helpers;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::thread;

pub struct Mediator {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl Mediator {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| self.run_inner(log))
    }

    pub fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account_thread = {
            let thread_name = "account_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            thread::Builder::new()
                .name(thread_name)
                .spawn(move || work(&log, &pool_clone, &delete_account_batch))
                .map_err(Error::from)?
        };

        let directory_podcast_thread = {
            let thread_name = "directory_podcast_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            thread::Builder::new()
                .name(thread_name)
                .spawn(move || work(&log, &pool_clone, &delete_directory_podcast_batch))
                .map_err(Error::from)?
        };

        let directory_search_thread = {
            let thread_name = "directory_search_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            thread::Builder::new()
                .name(thread_name)
                .spawn(move || work(&log, &pool_clone, &delete_directory_search_batch))
                .map_err(Error::from)?
        };

        let key_thread = {
            let thread_name = "key_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            thread::Builder::new()
                .name(thread_name)
                .spawn(move || work(&log, &pool_clone, &delete_key_batch))
                .map_err(Error::from)?
        };

        let podcast_feed_content_thread = {
            let thread_name = "podcast_feed_content_cleaner".to_owned();
            let log = log.new(o!("thread" => thread_name.clone()));
            let pool_clone = self.pool.clone();

            thread::Builder::new()
                .name(thread_name)
                .spawn(move || work(&log, &pool_clone, &delete_podcast_feed_content_batch))
                .map_err(Error::from)?
        };

        // `unwrap` followed by `?` might seem a little unusual. The `unwrap` is there
        // to unpack a thread that might have panicked (something that we hope
        // doesn't happen here and never expect to). Our work functions also
        // return a `Result<_>` which may contain an error that we've set which
        // is what the `?` is checking for.
        let num_account_cleaned = account_thread.join().unwrap()?;
        let num_directory_podcast_cleaned = directory_podcast_thread.join().unwrap()?;
        let num_directory_search_cleaned = directory_search_thread.join().unwrap()?;
        let num_key_cleaned = key_thread.join().unwrap()?;
        let num_podcast_feed_content_cleaned = podcast_feed_content_thread.join().unwrap()?;

        Ok(RunResult {
            // total number of cleaned resources
            num_cleaned: num_account_cleaned + num_directory_podcast_cleaned
                + num_directory_search_cleaned + num_key_cleaned
                + num_podcast_feed_content_cleaned,

            num_account_cleaned,
            num_directory_podcast_cleaned,
            num_directory_search_cleaned,
            num_key_cleaned,
            num_podcast_feed_content_cleaned,
        })
    }
}

pub struct RunResult {
    // total number of cleaned resources
    pub num_cleaned: i64,

    pub num_account_cleaned:              i64,
    pub num_directory_podcast_cleaned:    i64,
    pub num_directory_search_cleaned:     i64,
    pub num_key_cleaned:                  i64,
    pub num_podcast_feed_content_cleaned: i64,
}

//
// Private constants
//

// Target horizon beyond which we start to remove ephemeral accounts.
static ACCOUNT_DELETE_HORIZON: &'static str = "1 month";

// The maximum number of objects to try and delete as part of one batch. It's a
// good idea to constrain batch sizes so that we don't have any queries in the
// system that are too long-lived and affect replication and other critical
// facilities.
const DELETE_LIMIT: i64 = 1000;

// Target horizon beyond which we want to start removing old directory searches
// (they're cached for much less time than this, but we keep the records around
// for a while anyway, even if they're not used). Frequent searches that are
// still in used get upserted so that their timestamp is refreshed and they're
// never removed.
//
// Should be formatted as a string that's coercable to the Postgres interval
// type.
static DIRECTORY_SEARCH_DELETE_HORIZON: &'static str = "1 week";

// Target horizon beyond which we start to remove expired keys.
static KEY_DELETE_HORIZON: &'static str = "1 week";

// The maximum number of content rows to keep around for any given podcast.
pub const PODCAST_FEED_CONTENT_LIMIT: i64 = 5;

//
// Private types
//

// Exists because `sql_query` doesn't support querying into a tuple, only a
// struct.
#[derive(Clone, Debug, QueryableByName)]
struct DeleteResults {
    #[sql_type = "BigInt"]
    count: i64,
}

//
// Private functions
//

fn delete_account_batch(log: &Logger, conn: &PgConnection) -> Result<DeleteResults> {
    time_helpers::log_timed(
        &log.new(o!("step" => "delete_account_batch", "limit" => DELETE_LIMIT)),
        |_log| {
            diesel::sql_query(include_str!("../static/sql/cleaner_account.sql"))
                .bind::<Text, _>(ACCOUNT_DELETE_HORIZON)
                .bind::<BigInt, _>(DELETE_LIMIT)
                .get_result::<DeleteResults>(conn)
                .chain_err(|| "Error deleting account batch")
        },
    )
}

fn delete_directory_podcast_batch(log: &Logger, conn: &PgConnection) -> Result<DeleteResults> {
    time_helpers::log_timed(
        &log.new(o!("step" => "delete_directory_podcast_batch", "limit" => DELETE_LIMIT)),
        |_log| {
            // The idea here is to find "dangling" directory podcasts. Those are directory
            // podcasts that were never reified into a full podcast record (no
            // one ever clicked through to them) and for which there are
            // directory searches still pointing to (directory searches will
            // themselves be removed after a certain time period of disuse by another
            // cleaner below, but they won't remove any directory podcast records).
            diesel::sql_query(include_str!("../static/sql/cleaner_directory_podcast.sql"))
                .bind::<BigInt, _>(DELETE_LIMIT)
                .get_result::<DeleteResults>(conn)
                .chain_err(|| "Error deleting directory podcast content batch")
        },
    )
}

fn delete_directory_search_batch(log: &Logger, conn: &PgConnection) -> Result<DeleteResults> {
    time_helpers::log_timed(
        &log.new(o!("step" => "delete_directory_search_batch", "limit" => DELETE_LIMIT)),
        |_log| {
            // This works because directory_podcast_directory_search is ON DELETE CASCADE
            diesel::sql_query(include_str!("../static/sql/cleaner_directory_search.sql"))
                .bind::<Text, _>(DIRECTORY_SEARCH_DELETE_HORIZON)
                .bind::<BigInt, _>(DELETE_LIMIT)
                .get_result::<DeleteResults>(conn)
                .chain_err(|| "Error deleting directory search content batch")
        },
    )
}

fn delete_key_batch(log: &Logger, conn: &PgConnection) -> Result<DeleteResults> {
    time_helpers::log_timed(
        &log.new(o!("step" => "delete_key_batch", "limit" => DELETE_LIMIT)),
        |_log| {
            diesel::sql_query(include_str!("../static/sql/cleaner_key.sql"))
                .bind::<Text, _>(KEY_DELETE_HORIZON)
                .bind::<BigInt, _>(DELETE_LIMIT)
                .get_result::<DeleteResults>(conn)
                .chain_err(|| "Error deleting key batch")
        },
    )
}

fn delete_podcast_feed_content_batch(log: &Logger, conn: &PgConnection) -> Result<DeleteResults> {
    time_helpers::log_timed(
        &log.new(o!("step" => "delete_podcast_feed_content_batch", "limit" => DELETE_LIMIT)),
        |_log| {
            diesel::sql_query(include_str!(
                "../static/sql/cleaner_podcast_feed_content.sql"
            )).bind::<BigInt, _>(PODCAST_FEED_CONTENT_LIMIT)
                .bind::<BigInt, _>(DELETE_LIMIT)
                .get_result::<DeleteResults>(conn)
                .chain_err(|| "Error deleting directory podcast content batch")
        },
    )
}

fn work(
    log: &Logger,
    pool: &Pool<ConnectionManager<PgConnection>>,
    batch_delete_func: &Fn(&Logger, &PgConnection) -> Result<DeleteResults>,
) -> Result<i64> {
    debug!(log, "Thread waiting for a connection");
    let conn = pool.get()?;
    debug!(log, "Thread acquired a connection");

    let mut num_cleaned = 0;
    loop {
        let res = conn.transaction::<_, Error, _>(|| batch_delete_func(log, &*conn))?;
        num_cleaned += res.count;
        info!(log, "Cleaned batch"; "num_cleaned" => num_cleaned);

        if res.count < 1 {
            break;
        }
    }

    Ok(num_cleaned)
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use mediators::cleaner::*;
    use model;
    use model::insertable;
    use schema;
    use test_data;
    use test_helpers;

    use chrono::Utc;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use r2d2::PooledConnection;
    use rand::distributions::Alphanumeric;
    use rand::Rng;
    use std::io::prelude::*;
    use std::iter;
    use time::Duration;

    #[test]
    #[ignore]
    fn test_clean_account_cleans() {
        let mut bootstrap = TestBootstrap::new();

        let account = test_data::account::insert(&bootstrap.log, &bootstrap.conn);

        // Update the account so that it hasn't been seen in quite some time
        diesel::update(schema::account::table)
            .filter(schema::account::id.eq(account.id))
            .set(schema::account::last_seen_at.eq(Utc::now() - Duration::weeks(20)))
            .execute(&*bootstrap.conn)
            .unwrap();

        // This also has the effect of inserting an `account_podcast` row.
        test_data::account_podcast_episode::insert_args(
            &bootstrap.log,
            &bootstrap.conn,
            test_data::account_podcast_episode::Args {
                account: Some(&account),
                episode: None,
            },
        );

        test_data::key::insert_args(
            &bootstrap.log,
            &bootstrap.conn,
            test_data::key::Args {
                account:   Some(&account),
                expire_at: None,
            },
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(1, res.num_account_cleaned);
        assert_eq!(1, res.num_cleaned);
    }

    #[test]
    #[ignore]
    fn test_clean_account_ignores() {
        let mut bootstrap = TestBootstrap::new();

        // Insert an account that's ephemeral, but has been seen recently
        let _account = test_data::account::insert(&bootstrap.log, &bootstrap.conn);

        // Insert an account that's not ephemeral
        let permanent_account = test_data::account::insert_args(
            &bootstrap.log,
            &*bootstrap.conn,
            test_data::account::Args {
                email:     Some("foo@example.com"),
                ephemeral: false,
                mobile:    false,
            },
        );

        // Insert an account that's ephemeral, but created from a mobile client (these
        // are not deleted in case someone just hasn't opened their app in a
        // long time)
        let mobile_account = test_data::account::insert_args(
            &bootstrap.log,
            &*bootstrap.conn,
            test_data::account::Args {
                email:     None,
                ephemeral: true,
                mobile:    true,
            },
        );

        // For good measure (to test that the cleaner really won't clean permanent
        // accounts) update the permanent account so that it hasn't been seen in
        // a long time
        diesel::update(schema::account::table)
            .filter(
                schema::account::id
                    .eq(permanent_account.id)
                    .or(schema::account::id.eq(mobile_account.id)),
            )
            .set(schema::account::last_seen_at.eq(Utc::now() - Duration::weeks(20)))
            .execute(&*bootstrap.conn)
            .unwrap();

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(0, res.num_account_cleaned);
        assert_eq!(0, res.num_cleaned);
    }

    #[test]
    #[ignore]
    fn test_clean_directory_podcast_cleans() {
        let mut bootstrap = TestBootstrap::new();

        let _dir_podcast = test_data::directory_podcast::insert(&bootstrap.log, &*bootstrap.conn);

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(1, res.num_directory_podcast_cleaned);
        assert_eq!(1, res.num_cleaned);
    }

    #[test]
    #[ignore]
    fn test_clean_directory_podcast_ignores() {
        let mut bootstrap = TestBootstrap::new();

        // This directory podcast is attached to a hydrated podcast, so it shouldn't be
        // deleted.
        {
            let podcast = test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);
            let _dir_podcast = test_data::directory_podcast::insert_args(
                &bootstrap.log,
                &*bootstrap.conn,
                test_data::directory_podcast::Args {
                    podcast: Some(&podcast),
                },
            );
        }

        // This directory podcast is attached to a fresh search, so it shouldn't be
        // deleted.
        {
            let dir_podcast =
                test_data::directory_podcast::insert(&bootstrap.log, &*bootstrap.conn);
            let search = insert_directory_search(&bootstrap.log, &*bootstrap.conn);
            insert_directory_podcast_directory_search(
                &bootstrap.log,
                &*bootstrap.conn,
                &dir_podcast,
                &search,
            );
        }

        // This directory podcast is attached to an exception, and so shouldn't be
        // deleted.
        {
            let dir_podcast =
                test_data::directory_podcast::insert(&bootstrap.log, &*bootstrap.conn);
            insert_directory_podcast_exception(&bootstrap.log, &*bootstrap.conn, &dir_podcast);
        }

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(0, res.num_directory_podcast_cleaned);
        assert_eq!(0, res.num_cleaned);
    }

    #[test]
    #[ignore]
    fn test_clean_directory_search_cleans() {
        let mut bootstrap = TestBootstrap::new();

        let dir_podcast = test_data::directory_podcast::insert(&bootstrap.log, &*bootstrap.conn);
        let search = insert_directory_search(&bootstrap.log, &*bootstrap.conn);
        insert_directory_podcast_directory_search(
            &bootstrap.log,
            &*bootstrap.conn,
            &dir_podcast,
            &search,
        );

        // Search is recent and isn't cleaned
        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();

            assert_eq!(0, res.num_directory_search_cleaned);
            assert_eq!(0, res.num_cleaned);
        }

        diesel::update(schema::directory_search::table)
            .filter(schema::directory_search::id.eq(search.id))
            .set(schema::directory_search::retrieved_at.eq(Utc::now() - Duration::weeks(2)))
            .execute(&*bootstrap.conn)
            .unwrap();

        // Search is now expired and gets removed
        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();

            assert_eq!(1, res.num_directory_search_cleaned);

            // We don't check the total number cleaned here (like we do in other tests)
            // because there's a race condition: cleaning up the directory
            // search leaves the directory podcast dangling. If the thread
            // cleaning directory podcasts is a little behind it may also remove
            // that record, leaving us with two cleaned records in total.
        }
    }

    #[test]
    #[ignore]
    fn test_clean_key_cleans() {
        let mut bootstrap = TestBootstrap::new();

        // Insert a key that expired a week ago
        let _key = test_data::key::insert_args(
            &bootstrap.log,
            &*bootstrap.conn,
            test_data::key::Args {
                account:   None,
                expire_at: Some(Utc::now() - Duration::weeks(1)),
            },
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(1, res.num_key_cleaned);
        assert_eq!(1, res.num_cleaned);
    }

    #[test]
    #[ignore]
    fn test_clean_key_ignores() {
        let mut bootstrap = TestBootstrap::new();

        // Insert a key that doesn't expire
        let _key = test_data::key::insert(&bootstrap.log, &*bootstrap.conn);

        // Insert a key that doesn't expire for a while
        let _key = test_data::key::insert_args(
            &bootstrap.log,
            &*bootstrap.conn,
            test_data::key::Args {
                account:   None,
                expire_at: Some(Utc::now() + Duration::weeks(4)),
            },
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(0, res.num_key_cleaned);
        assert_eq!(0, res.num_cleaned);
    }

    #[test]
    #[ignore]
    fn test_clean_podcast_feed_content_cleans() {
        let mut bootstrap = TestBootstrap::new();

        let num_contents = 25;
        let podcast = test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);
        for _i in 0..num_contents {
            insert_podcast_feed_content(&bootstrap.log, &*bootstrap.conn, &podcast);
        }

        // This is here to ensure that a different podcast's records (one that only has
        // one content row) aren't affected by the run
        let _ = test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);

        assert_eq!(
            // +2: one inserted with the original podcast and one more for the other podcast
            // inserted above
            Ok(num_contents + 2),
            schema::podcast_feed_content::table
                .count()
                .first(&*bootstrap.conn)
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // Expect to have cleaned all except the limit number of rows
        let expected_num_cleaned = num_contents + 1 - PODCAST_FEED_CONTENT_LIMIT;
        assert_eq!(expected_num_cleaned, res.num_podcast_feed_content_cleaned);
        assert_eq!(expected_num_cleaned, res.num_cleaned);

        // Expect to have exactly the limit left in the database plus one more for the
        // other podcast
        assert_eq!(
            Ok(PODCAST_FEED_CONTENT_LIMIT + 1),
            schema::podcast_feed_content::table
                .count()
                .first(&*bootstrap.conn)
        );
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
        pool:    Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let conn = pool.get().map_err(Error::from).unwrap();
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                conn:    conn,
                log:     test_helpers::log_sync(),
                pool:    pool,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
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

    fn insert_directory_podcast_directory_search(
        _log: &Logger,
        conn: &PgConnection,
        dir_podcast: &model::DirectoryPodcast,
        search: &model::DirectorySearch,
    ) {
        let join_ins = insertable::DirectoryPodcastDirectorySearch {
            directory_podcast_id: dir_podcast.id,
            directory_search_id:  search.id,
            position:             0,
        };

        diesel::insert_into(schema::directory_podcast_directory_search::table)
            .values(&join_ins)
            .execute(conn)
            .unwrap();
    }

    fn insert_directory_podcast_exception(
        _log: &Logger,
        conn: &PgConnection,
        dir_podcast: &model::DirectoryPodcast,
    ) {
        let ex_ins = insertable::DirectoryPodcastException {
            directory_podcast_id: dir_podcast.id,
            errors:               vec!["error1".to_owned(), "error2".to_owned()],
            occurred_at:          Utc::now(),
        };

        diesel::insert_into(schema::directory_podcast_exception::table)
            .values(&ex_ins)
            .execute(conn)
            .unwrap();
    }

    fn insert_directory_search(log: &Logger, conn: &PgConnection) -> model::DirectorySearch {
        let mut rng = rand::thread_rng();

        let directory = model::Directory::itunes(log, &conn).unwrap();

        let search_ins = insertable::DirectorySearch {
            directory_id: directory.id,
            query:        iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .take(50)
                .collect(),
            retrieved_at: Utc::now(),
        };

        diesel::insert_into(schema::directory_search::table)
            .values(&search_ins)
            .get_result(conn)
            .unwrap()
    }

    fn insert_podcast_feed_content(_log: &Logger, conn: &PgConnection, podcast: &model::Podcast) {
        let body = "feed body".to_owned();
        let mut rng = rand::thread_rng();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write(body.as_bytes()).unwrap();

        let content_ins = insertable::PodcastFeedContent {
            content_gzip: encoder.finish().unwrap(),
            podcast_id:   podcast.id,
            retrieved_at: Utc::now(),

            // There's a length check on this field in Postgres, so generate a string that's
            // exactly 64 characters long.
            sha256_hash: iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .take(64)
                .collect(),
        };

        diesel::insert_into(schema::podcast_feed_content::table)
            .values(&content_ins)
            .execute(conn)
            .unwrap();
    }
}
