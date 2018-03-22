use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub conn:    &'a PgConnection,
    pub last_ip: String,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account = self.insert_account(log)?;
        Ok(RunResult { account })
    }

    //
    // Steps
    //

    fn insert_account(&mut self, log: &Logger) -> Result<model::Account> {
        time_helpers::log_timed(&log.new(o!("step" => "insert_account")), |_log| {
            diesel::insert_into(schema::account::table)
                .values(&insertable::Account {
                    last_ip: self.last_ip.clone(),
                })
                .get_result(self.conn)
                .chain_err(|| "Error inserting account")
        })
    }
}

pub struct RunResult {
    pub account: model::Account,
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_creator::*;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_account_create() {
        let mut bootstrap = TestBootstrap::new();
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                conn:    test_helpers::connection(),
                log:     test_helpers::log(),
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:    &*self.conn,
                    last_ip: "1.2.3.4".to_owned(),
                },
                self.log.clone(),
            )
        }
    }
}
