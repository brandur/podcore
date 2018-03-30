use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use rand::EntropyRng;
use rand::distributions::Alphanumeric;
use slog::Logger;
use std::iter;

pub struct Mediator<'a> {
    pub account: &'a model::Account,
    pub conn:    &'a PgConnection,
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
                    expire_at: None,
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

/*
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
                    last_ip:   "1.2.3.4".to_owned(),
                },
                self.log.clone(),
            )
        }
    }
}
*/
