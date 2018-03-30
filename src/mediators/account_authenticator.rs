use errors::*;
use model;
use schema;
use time_helpers;

use chrono::Utc;
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub conn:   &'a PgConnection,
    pub secret: &'a str,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let key = self.select_key(log, self.secret)?;
        if key.is_none() {
            return Ok(RunResult { account: None });
        }

        let account = self.touch_and_select_account(log, &key.unwrap())?;
        Ok(RunResult {
            account: Some(account),
        })
    }

    //
    // Steps
    //

    fn touch_and_select_account(
        &mut self,
        log: &Logger,
        key: &model::Key,
    ) -> Result<model::Account> {
        time_helpers::log_timed(&log.new(o!("step" => "touch_and_select_account")), |_log| {
            diesel::update(schema::account::table)
                .filter(schema::account::id.eq(key.account_id))
                .set(schema::account::last_seen_at.eq(Utc::now()))
                .get_result(self.conn)
                .chain_err(|| "Error touching account")
        })
    }

    fn select_key(&mut self, log: &Logger, secret: &str) -> Result<Option<model::Key>> {
        time_helpers::log_timed(&log.new(o!("step" => "select_key")), |_log| {
            schema::key::table
                .filter(schema::key::secret.eq(secret))
                .filter(schema::key::expire_at.lt(Utc::now()))
                .first(self.conn)
                .optional()
                .chain_err(|| "Error selecting key")
        })
    }
}

pub struct RunResult {
    pub account: Option<model::Account>,
}

//
// Tests
//

/*
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
                    account: &self.account,
                    conn:    &*self.conn,
                },
                self.log.clone(),
            )
        }
    }
}
*/
