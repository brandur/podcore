use diesel::prelude::*;
use diesel::pg::PgConnection;
use std::env;

pub fn connection() -> PgConnection {
    let database_url =
        env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set in order to run tests");
    let conn = PgConnection::establish(&database_url).unwrap();
    conn.begin_test_transaction().unwrap();
    conn
}
