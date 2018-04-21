use errors::*;
use model;
use schema;
use time_helpers;

use chrono::Utc;
use crypto::scrypt;
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub conn:     &'a PgConnection,
    pub email:    &'a str,
    pub last_ip:  &'a str,
    pub password: &'a str,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        self.params_validate()?;

        // We don't want secrets in logs, so we rely on this statement being compiled
        // out in a release build because it's `debug!`
        debug!(log, "Authenticating password"; "email" => self.email, "password" => self.password);

        let account = self.select_account(log, self.email)?;
        if account.is_none() {
            info!(log, "No account with that email");
            bail!(error::validation("No account matched that email address."));
        };

        let account = account.unwrap();
        info!(log, "Found account"; "id" => account.id);

        if !scrypt_check(log, &account, self.password) {
            info!(log, "Password did not match scrypt hash");
            bail!(error::validation(
                "That password doesn't match the account's."
            ));
        }

        info!(log, "Password matched scrypt hash");
        let account = self.touch_account(log, &account)?;
        let key = self.select_key(log, &account)?;

        Ok(RunResult { account, key })
    }

    //
    // Steps
    //

    fn select_account(&mut self, log: &Logger, email: &str) -> Result<Option<model::Account>> {
        time_helpers::log_timed(&log.new(o!("step" => "select_account")), |_log| {
            schema::account::table
                .filter(schema::account::email.eq(email))
                .first(self.conn)
                .optional()
                .chain_err(|| "Error selecting account")
        })
    }

    fn select_key(&self, log: &Logger, account: &model::Account) -> Result<model::Key> {
        time_helpers::log_timed(&log.new(o!("step" => "select_key")), |_log| {
            schema::key::table
                .filter(schema::key::account_id.eq(account.id))
                .filter(schema::key::expire_at.is_null())
                .first(self.conn)
                .chain_err(|| "Error selecting key")
        })
    }

    fn touch_account(&mut self, log: &Logger, account: &model::Account) -> Result<model::Account> {
        time_helpers::log_timed(&log.new(o!("step" => "touch_account")), |_log| {
            diesel::update(schema::account::table)
                .filter(schema::account::id.eq(account.id))
                .set((
                    schema::account::last_ip.eq(self.last_ip),
                    schema::account::last_seen_at.eq(Utc::now()),
                ))
                .get_result(self.conn)
                .chain_err(|| "Error touching account")
        })
    }

    //
    // Private functions
    //

    /// Performs validations on parameters. These are user facing.
    fn params_validate(&mut self) -> Result<()> {
        if self.email.is_empty() {
            bail!(error::validation("Please specify an email address."))
        }
        if self.password.is_empty() {
            bail!(error::validation("Please specify a password."))
        }

        Ok(())
    }
}

pub struct RunResult {
    pub account: model::Account,
    pub key:     model::Key,
}

//
// Private functions
//

/// Checks a password against an Scrypted hash. Returns `true` if the
/// password matched successfully.
fn scrypt_check(log: &Logger, account: &model::Account, password: &str) -> bool {
    time_helpers::log_timed(&log.new(o!("step" => "scrypt_check")), |_log| {
        // We use some unwraps here with the logic that if something is wrong with our
        // scrypt verification, let's just blow up and find out about it.
        scrypt::scrypt_check(password, account.password_scrypt.as_ref().unwrap()).unwrap()
    })
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_password_authenticator::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_account_password_authenticator_ok() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:    TEST_EMAIL,
            password: test_helpers::PASSWORD,
        });

        let res = {
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap()
        };

        assert_eq!(bootstrap.account.id, res.account.id);
        assert_eq!(TEST_NEW_IP, res.account.last_ip);
        assert_eq!(bootstrap.key.id, res.key.id);
        assert_eq!(bootstrap.account.id, res.key.account_id);
    }

    #[test]
    fn test_account_password_authenticator_empty_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:    "",
            password: test_helpers::PASSWORD,
        });

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);

        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: Please specify an email address.",
            format!("{}", e).as_str()
        );
    }

    #[test]
    fn test_account_password_authenticator_empty_password() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:    TEST_EMAIL,
            password: "",
        });

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);

        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: Please specify a password.",
            format!("{}", e).as_str()
        );
    }

    #[test]
    fn test_account_password_authenticator_bad_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:    "no-one@example.com",
            password: test_helpers::PASSWORD,
        });

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);

        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: No account matched that email address.",
            format!("{}", e).as_str()
        );
    }

    #[test]
    fn test_account_password_authenticator_bad_password() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:    TEST_EMAIL,
            password: "bad-password",
        });

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);

        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: That password doesn't match the account's.",
            format!("{}", e).as_str()
        );
    }

    //
    // Private types/functions
    //

    static TEST_EMAIL: &str = "foo@example.com";
    static TEST_NEW_IP: &str = "4.5.6.7";

    struct Args<'a> {
        email:    &'a str,
        password: &'a str,
    }

    struct TestBootstrap<'a> {
        _common: test_helpers::CommonTestBootstrap,
        account: model::Account,
        args:    Args<'a>,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        key:     model::Key,
        log:     Logger,
    }

    impl<'a> TestBootstrap<'a> {
        fn new(args: Args) -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            let account = test_data::account::insert_args(
                &log,
                &*conn,
                test_data::account::Args {
                    email:     Some(TEST_EMAIL),
                    ephemeral: false,
                    mobile:    false,
                },
            );
            let key = test_data::key::insert_args(
                &log,
                &*conn,
                test_data::key::Args {
                    account:   Some(&account),
                    expire_at: None,
                },
            );

            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                account,
                args,
                conn,
                key,
                log,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:     &*self.conn,
                    email:    self.args.email,
                    last_ip:  TEST_NEW_IP,
                    password: self.args.password,
                },
                self.log.clone(),
            )
        }
    }
}
