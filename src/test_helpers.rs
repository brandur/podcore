use error_helpers;
use errors::*;
use middleware;
use schema;
use server;
use test_data;

use actix;
use actix_web;
use bytes::Bytes;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use percent_encoding::{percent_encode, PercentEncode, DEFAULT_ENCODE_SET};
use r2d2::{HandleError, Pool, PooledConnection};
use r2d2_diesel::ConnectionManager;
use serde_json;
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

// An "ideal" feed that has proper formatting and the standard set of expected
// fields.
//
// I've previously run into bugs where we were only processing one episode, so
// try to keep at least two items in this list.
pub const IDEAL_FEED: &[u8] = br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <description>Description</description>
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
    <item>
      <description><![CDATA[Item 2 description]]></description>
      <guid>2</guid>
      <itunes:explicit>yes</itunes:explicit>
      <media:content url="https://example.com/item-2" type="audio/mpeg"/>
      <pubDate>Sat, 23 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 2 Title</title>
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

// The password that test accounts (non-ephemeral ones) are created with.
pub const PASSWORD: &str = "password123";

// This is a trival work factor that should never be used in real life.
// However, to keep things in tests fast, we inject a low work factor that
// allows the CPU to produce these very quickly.
//
// This blog post on the subject is good: https://blog.filippo.io/the-scrypt-parameters/
pub const SCRYPT_LOG_N: u8 = 4;

// This is the IP that we expect test requests to originate from. It's a
// constant just in case Actix changes its default implementation.
pub const REQUEST_IP: &str = "<no IP>";

//
// Public types
//

// Gives us a place for adding code that can be run before and after any test.
//
// This struct should be a member on *every* `TestBootstrap` throughout the
// suite.
pub struct CommonTestBootstrap {}

impl CommonTestBootstrap {
    pub fn new() -> CommonTestBootstrap {
        // Make sure that nothing from the test suite is ever capable of reporting to
        // Sentry, even if the key is set in our environment.
        env::remove_var("SENTRY_URL");

        CommonTestBootstrap {}
    }
}

// A bootstrap specifically for use with HTTP API/web integration tests.
// Encloses a `TestServerBuilder` which produces a lot of flexibility in how to
// construct the app that's going to be tested.
pub struct IntegrationTestBootstrap {
    _common:            CommonTestBootstrap,
    pub log:            Logger,
    pub pool:           Pool<ConnectionManager<PgConnection>>,
    pub server_builder: actix_web::test::TestServerBuilder<server::StateImpl>,
}

impl IntegrationTestBootstrap {
    pub fn new() -> IntegrationTestBootstrap {
        let log = log();
        let log_clone = log.clone();
        let pool = pool();
        let pool_clone = pool.clone();

        let server_builder = actix_web::test::TestServer::build_with_state(move || {
            server_state_with_sync_executor(&log_clone, Some(pool_clone.clone()))
        });

        IntegrationTestBootstrap {
            _common:        CommonTestBootstrap::new(),
            log:            log,
            pool:           pool,
            server_builder: server_builder,
        }
    }

    /// Creates an `Account` and produces a test authenticator middleware that
    /// will guarantee that an account looks authenticated to handlers.
    ///
    /// Note that the one problem with this approach is that because we insert
    /// the account on a test transaction (in order to not leave garbage
    /// left over in the database), it won't be visible to any other
    /// connections that try to load it. We may need to change this behavior
    /// in the future, but if we do, we'll also have to make sure to clean the
    /// database afterwards.
    pub fn authenticated_middleware(&self) -> middleware::test::authenticator::Middleware {
        let conn = self.pool.get().unwrap();
        conn.begin_test_transaction().unwrap();
        let account = test_data::account::insert(&self.log, &*conn);
        middleware::test::authenticator::Middleware { account }
    }
}

//
// Public functions
//

/// Acquires a single connection from a connection pool and starts a test
/// transaction on it. This is suitable for use a shortcut by subcommands that
/// only need to run one single-threaded task.
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

    // Try to identify the "leaf" relations and drop them. All other relations
    // should cascade from them. This list might need to be added to
    // occasionally.
    //
    // I've left out `directory` even thought it's a leaf because there's no point
    // in deleting it over and over when it can be reused unchanged.
    conn.execute("TRUNCATE TABLE account CASCADE").unwrap();
    conn.execute("TRUNCATE TABLE job CASCADE").unwrap();
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

/// Gets a synchronous Logger. This is more suitable in some situations like
/// where threading is involved.
pub fn log_sync() -> Logger {
    if nocapture() {
        log_sync_no_capture()
    } else {
        slog::Logger::root(slog::Discard, o!())
    }
}

/// Same as `log_sync`, but never discards output. This is useful for printing
/// errors from the test suite (that might otherwise get swallowed).
pub fn log_sync_no_capture() -> Logger {
    let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    slog::Logger::root(drain, o!("env" => "test"))
}

/// Initializes and returns a connection pool suitable for use across threads.
pub fn pool() -> Pool<ConnectionManager<PgConnection>> {
    match pool_inner() {
        Ok(pool) => pool,
        Err(e) => {
            error_helpers::print_error(&log_sync_no_capture(), &e);
            panic!("{}", e);
        }
    }
}

pub fn read_body_json(resp: actix_web::client::ClientResponse) -> serde_json::Value {
    use actix_web::HttpMessage;
    use futures::Future;

    let bytes: Bytes = resp.body().wait().unwrap();
    serde_json::from_slice(bytes.as_ref()).unwrap()
}

pub fn server_state(log: &Logger) -> server::StateImpl {
    server_state_with_sync_executor(log, None)
}

// TODO: Maybe make this a reference to a pool and do the cloning ourselves.
pub fn server_state_with_sync_executor(
    log: &Logger,
    pool: Option<Pool<ConnectionManager<PgConnection>>>,
) -> server::StateImpl {
    let sync_addr = match pool {
        Some(pool) => Some(actix::SyncArbiter::start(1, move || server::SyncExecutor {
            pool: pool.clone(),
        })),
        None => None,
    };

    server::StateImpl {
        assets_version: "".to_owned(),
        log:            log.clone(),
        scrypt_log_n:   SCRYPT_LOG_N,
        sync_addr:      sync_addr,
    }
}

pub fn url_encode(bytes: &[u8]) -> PercentEncode<DEFAULT_ENCODE_SET> {
    percent_encode(bytes, DEFAULT_ENCODE_SET)
}

//
// Tests
//

// A no-op test that will clean the database. This is useful to run before a
// test suite that's not using test transactions just in case a failed test
// previously left some state in there.
//
// When running through the ignored test suite we can expect this test to run a
// second time, but it's okay, it should be a no-op by then.
#[test]
#[ignore]
fn test_clean_database() {
    clean_database(&log(), &*connection());
}

//
// Private types/functions
//

/// An `r2d2::HandleError` implementation which logs at the error level.
#[derive(Copy, Clone, Debug)]
pub struct LoggingErrorHandler;

impl<E> HandleError<E> for LoggingErrorHandler
where
    E: std::error::Error,
{
    fn handle_error(&self, error: E) {
        error!(log_sync_no_capture(), "{}", error);
    }
}

pub static MAX_NUM_CONNECTIONS: u32 = 10;

fn check_database(conn: &PgConnection) -> Result<()> {
    // Note that we only check one table's count as a proxy for the state of the
    // entire database. This isn't bullet proof, but will hopefully be enough
    // to avoid most stupid problems.
    match schema::podcast::table
        .count()
        .first(conn)
        .chain_err(|| "Error testing database connection")?
    {
        0 => Ok(()),
        n => Err(Error::from(format!(
            "Expected test database to be empty, but found {} podcast(s). Please reset it.",
            n
        ))),
    }
}

fn nocapture() -> bool {
    match env::var("RUST_TEST_NOCAPTURE") {
        Ok(val) => &val != "0",
        Err(_) => false,
    }
}

fn pool_inner() -> Result<Pool<ConnectionManager<PgConnection>>> {
    let database_url = env::var("TEST_DATABASE_URL").chain_err(|| "TEST_DATABASE_URL must be set")?;
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    let pool = Pool::builder()
        // Basically don't allow a thread pool that doesn't have slack -- all tests should be
        // optimized so that they aren't requesting connections that can't be had, and if they are
        // it's indicative of a bug.
        .connection_timeout(Duration::from_secs(5))
        .error_handler(Box::new(LoggingErrorHandler {}))
        .max_size(MAX_NUM_CONNECTIONS)
        // If `min_idle` is not set, then `r2d2` will open a number of connections equal to
        // `max_size` on startup, which as you can probably imagine isn't something that works well
        // for test concurrency.
        .min_idle(Some(0))
        .build(manager)
        .chain_err(|| "Error creating thread pool")?;

    let conn = pool.get().map_err(Error::from)?;
    check_database(&*conn)?;
    Ok(pool)
}
