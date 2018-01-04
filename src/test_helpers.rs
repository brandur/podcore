use schema;

use diesel::pg::PgConnection;
use diesel::prelude::*;
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
    check_database(&conn);
    conn
}

pub fn log() -> Logger {
    if nocapture() {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        let async_drain = slog_async::Async::new(drain).build().fuse();
        slog::Logger::root(async_drain, o!("env" => "test"))
    } else {
        slog::Logger::root(slog::Discard, o!())
    }
}

//
// Private types/functions
//

fn check_database(conn: &PgConnection) {
    // Note that we only check one table's count as a proxy for the state of the entire database.
    // This isn't bullet proof, but will hopefully be enough to avoid most stupid problems.
    match schema::podcasts::table.count().first(conn) {
        Ok(0) => (),
        Ok(_) => panic!("Expected test database to be empty. Please reset it."),
        Err(e) => panic!("Error testing database connection: {}", e),
    }
}

fn nocapture() -> bool {
    match env::var("RUST_TEST_NOCAPTURE") {
        Ok(val) => &val != "0",
        Err(_) => false,
    }
}
