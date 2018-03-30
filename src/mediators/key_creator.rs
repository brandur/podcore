use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use chrono::{DateTime, Utc};
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use rand::EntropyRng;
use rand::distributions::Alphanumeric;
use slog::Logger;
use std::iter;

pub struct Mediator<'a> {
    pub account:   &'a model::Account,
    pub conn:      &'a PgConnection,
    pub expire_at: Option<DateTime<Utc>>,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let secret = self.generate_secret(log);
        let key = self.insert_key(log, secret)?;
        Ok(RunResult { key })
    }

    //
    // Steps
    //

    fn generate_secret(&mut self, _log: &Logger) -> String {
        use rand::Rng;

        // `EntropyRng` collects secure random data from the OS if available (it almost
        // always is), and falls back to the `JitterRng` entropy collector
        // otherwise. It panics if no secure source of entropy is available.
        let mut rng = EntropyRng::new();

        iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .take(KEY_LENGTH)
            .collect()
    }

    fn insert_key(&mut self, log: &Logger, secret: String) -> Result<model::Key> {
        time_helpers::log_timed(&log.new(o!("step" => "insert_key")), |_log| {
            diesel::insert_into(schema::key::table)
                .values(&insertable::Key {
                    account_id: self.account.id,
                    expire_at: self.expire_at,
                    secret,
                })
                .get_result(self.conn)
                .chain_err(|| "Error inserting key")
        })
    }
}

pub struct RunResult {
    pub key: model::Key,
}

//
// Private constants
//

// Note that there's a database constraint in place to enforce this as well.
const KEY_LENGTH: usize = 60;

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::key_creator::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_key_create() {
        let mut bootstrap = TestBootstrap::new();
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.key.id);
        assert_eq!(KEY_LENGTH, res.key.secret.len());
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        account: model::Account,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                account: test_data::account::insert(&log, &conn),

                // Only move these after filling the above
                conn: conn,
                log:  log,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    account:   &self.account,
                    conn:      &*self.conn,
                    expire_at: None,
                },
                self.log.clone(),
            )
        }
    }
}
