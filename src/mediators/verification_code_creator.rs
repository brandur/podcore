use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use rand::distributions::Alphanumeric;
use rand::EntropyRng;
use slog::Logger;
use std::iter;

pub struct Mediator<'a> {
    pub account: &'a model::Account,
    pub conn:    &'a PgConnection,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let secret = generate_secret(log);

        // We don't want secrets in logs, so we rely on this statement being compiled
        // out in a release build because it's `debug!`
        debug!(log, "Generated secret"; "secret" => secret.as_str());

        let code = self.insert_verification_code(log, secret)?;
        Ok(RunResult { code })
    }

    //
    // Steps
    //

    fn insert_verification_code(
        &mut self,
        log: &Logger,
        secret: String,
    ) -> Result<model::VerificationCode> {
        time_helpers::log_timed(&log.new(o!("step" => "insert_verification_code")), |_log| {
            diesel::insert_into(schema::verification_code::table)
                .values(&insertable::VerificationCode {
                    account_id: self.account.id,
                    secret,
                })
                .get_result(self.conn)
                .chain_err(|| "Error inserting verification code")
        })
    }
}

pub struct RunResult {
    pub code: model::VerificationCode,
}

//
// Private constants
//

// Note that there's a database constraint in place to enforce this as well.
const SECRET_LENGTH: usize = 60;

//
// Private functions
//

fn generate_secret(_log: &Logger) -> String {
    use rand::Rng;

    // `EntropyRng` collects secure random data from the OS if available (it almost
    // always is), and falls back to the `JitterRng` entropy collector
    // otherwise. It panics if no secure source of entropy is available.
    let mut rng = EntropyRng::new();

    iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .take(SECRET_LENGTH)
        .collect()
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::verification_code_creator::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_verification_code_create() {
        let mut bootstrap = TestBootstrap::new();
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.code.id);
        assert_eq!(SECRET_LENGTH, res.code.secret.len());
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
