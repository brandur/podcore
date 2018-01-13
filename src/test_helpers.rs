use schema;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use r2d2::{HandleError, Pool, PooledConnection};
use r2d2_diesel::ConnectionManager;
use slog;
use slog::{Drain, Logger};
use slog_async;
use slog_term;
use std;
use std::env;
use std::time::Duration;

/// Acquires a single connection from a connection pool and starts a test transaction on it. This
/// is suitable for use a shortcut by subcommands that only need to run one single-threaded task.
pub fn connection() -> PooledConnection<ConnectionManager<PgConnection>> {
    let conn = pool()
        .get()
        .expect("Error acquiring connection from connection pool");
    conn.begin_test_transaction().unwrap();
    conn
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

    let conn = pool.get()
        .expect("Error acquiring connection from connection pool");
    check_database(&*conn);

    pool
}

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
        error!(log_sync(), "{}", error);
    }
}

pub static NUM_CONNECTIONS: u32 = 10;

fn check_database(conn: &PgConnection) {
    // Note that we only check one table's count as a proxy for the state of the entire database.
    // This isn't bullet proof, but will hopefully be enough to avoid most stupid problems.
    match schema::podcasts::table.count().first(conn) {
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
