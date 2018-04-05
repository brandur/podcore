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
    pub conn:      &'a PgConnection,
    pub email:     Option<String>,
    pub ephemeral: bool,
    pub mobile:    bool,
    pub last_ip:   &'a str,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
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
                    email:     self.email.clone(),
                    ephemeral: self.ephemeral,
                    last_ip:   self.last_ip.to_owned(),
                    mobile:    self.mobile,
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
    fn test_account_create_ephemeral() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     None,
            ephemeral: true,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
    }

    #[test]
    fn test_account_create_permanent() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     Some("foo@example.com".to_owned()),
            ephemeral: false,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
    }

    #[test]
    fn test_account_create_invalid_ephemeral_with_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     Some("foo@example.com".to_owned()),
            ephemeral: true,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!("Error inserting account", e.description());
    }

    #[test]
    fn test_account_create_invalid_permanent_without_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     None,
            ephemeral: false,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!("Error inserting account", e.description());
    }

    //
    // Private types/functions
    //

    struct Args {
        email:     Option<String>,
        ephemeral: bool,
    }

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        args:    Args,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
    }

    impl TestBootstrap {
        fn new(args: Args) -> TestBootstrap {
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                args:    args,
                conn:    test_helpers::connection(),
                log:     test_helpers::log(),
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:      &*self.conn,
                    email:     self.args.email.clone(),
                    ephemeral: self.args.ephemeral,
                    last_ip:   "1.2.3.4",
                    mobile:    false,
                },
                self.log.clone(),
            )
        }
    }
}
