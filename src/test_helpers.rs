use errors::*;
use schema;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::types::Text;
use r2d2::{HandleError, Pool, PooledConnection};
use r2d2_diesel::ConnectionManager;
use slog;
use slog::{Drain, Logger};
use slog_async;
use slog_term;
use std;
use std::env;
use std::time::Duration;

//
// Public constants
//

pub const IDEAL_FEED: &[u8] = br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <language>en-US</language>
    <link>https://example.com/podcast</link>
    <media:thumbnail url="https://example.com/podcast-image-url.jpg"/>
    <title>Title</title>
    <item>
      <description><![CDATA[Item 1 description]]></description>
      <guid>1</guid>
      <itunes:explicit>yes</itunes:explicit>
      <media:content url="https://example.com/item-1" type="audio/mpeg"/>
      <pubDate>Sun, 24 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 1 Title</title>
    </item>
  </channel>
</rss>"#;

pub const MINIMAL_FEED: &[u8] = br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <title>Title</title>
    <item>
      <guid>1</guid>
      <media:content url="https://example.com/item-1" type="audio/mpeg"/>
      <pubDate>Sun, 24 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 1 Title</title>
    </item>
  </channel>
</rss>"#;

//
// Public functions
//

/// Acquires a single connection from a connection pool and starts a test transaction on it. This
/// is suitable for use a shortcut by subcommands that only need to run one single-threaded task.
pub fn connection() -> PooledConnection<ConnectionManager<PgConnection>> {
    let conn = pool().get().map_err(Error::from).unwrap();
    conn.begin_test_transaction().unwrap();
    conn
}

// Resets database state. This is useful for tests that can't use a test
// transaction like the ones that require multiple connections across multiple
// threads.
//
// Note that this is currently a really janky way of doing this. Please
// continue to add whatever statements here that are necessary to get things
// back to zero.
pub fn clean_database(log: &Logger, conn: &PgConnection) {
    debug!(log, "Cleaning database on bootstrap drop");
    conn.execute("TRUNCATE TABLE podcast CASCADE").unwrap();
}

pub fn log() -> Logger {
    if nocapture() {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain).build().fuse();
        slog::Logger::root(drain, o!("env" => "test"))
    } else {
        slog::Logger::root(slog::Discard, o!())
    }
}

/// Gets a synchronous Logger. This is more suitable in some situations like where threading is
/// involved.
pub fn log_sync() -> Logger {
    if nocapture() {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        slog::Logger::root(drain, o!("env" => "test"))
    } else {
        slog::Logger::root(slog::Discard, o!())
    }
}

/// Initializes and returns a connection pool suitable for use across threads.
pub fn pool() -> Pool<ConnectionManager<PgConnection>> {
    let database_url = env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    let pool = Pool::builder()
        // Basically don't allow a thread pool that doesn't have slack -- all tests should be
        // optimized so that they aren't requesting connections that can't be had, and if they are
        // it's indicative of a bug.
        .connection_timeout(Duration::from_secs(1))
        .error_handler(Box::new(LoggingErrorHandler {}))
        .max_size(NUM_CONNECTIONS)
        .build(manager)
        .expect("Failed to create pool.");

    let conn = pool.get().map_err(Error::from).unwrap();
    check_database(&*conn);

    pool
}

//
// Private types/functions
//

/// An `r2d2::HandleError` implementation which logs at the error level.
#[derive(Copy, Clone, Debug)]
pub struct LoggingErrorHandler;

#[derive(Clone, Debug, QueryableByName)]
struct Snapshot {
    #[sql_type = "Text"]
    id: String,
}

impl<E> HandleError<E> for LoggingErrorHandler
where
    E: std::error::Error,
{
    fn handle_error(&self, error: E) {
        error!(log_sync(), "{}", error);
    }
}

pub static NUM_CONNECTIONS: u32 = 10;

fn check_database(conn: &PgConnection) {
    // Note that we only check one table's count as a proxy for the state of the
    // entire database. This isn't bullet proof, but will hopefully be enough
    // to avoid most stupid problems.
    match schema::podcast::table.count().first(conn) {
        Ok(0) => (),
        Ok(n) => panic!(
            "Expected test database to be empty, but found {} podcast(s). Please reset it.",
            n
        ),
        Err(e) => panic!("Error testing database connection: {}", e),
    }
}

fn nocapture() -> bool {
    match env::var("RUST_TEST_NOCAPTURE") {
        Ok(val) => &val != "0",
        Err(_) => false,
    }
}
