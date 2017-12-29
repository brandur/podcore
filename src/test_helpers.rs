use diesel::prelude::*;
use diesel::pg::PgConnection;
use slog;
use slog::{Drain, Logger};
use slog_async;
use slog_term;
use std;
use std::env;

pub fn connection() -> PgConnection {
    let database_url =
        env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set in order to run tests");
    let conn = PgConnection::establish(&database_url).unwrap();
    conn.begin_test_transaction().unwrap();
    conn
}

pub fn log() -> Logger {
    let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
    let drain = slog_term::CompactFormat::new(decorator).build().fuse();
    let async_drain = slog_async::Async::new(drain).build().fuse();
    slog::Logger::root(async_drain, o!("env" => "test"))
}
