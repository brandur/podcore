use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use crypto::scrypt;
use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use regex::Regex;
use slog::Logger;

pub struct Mediator<'a> {
    pub conn:         &'a PgConnection,
    pub email:        Option<&'a str>,
    pub ephemeral:    bool,
    pub mobile:       bool,
    pub last_ip:      &'a str,
    pub password:     Option<&'a str>,
    pub scrypt_log_n: Option<u8>,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        self.validate_request()?;
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
                    email:           self.email.map(|e| e.to_owned()),
                    ephemeral:       self.ephemeral,
                    last_ip:         self.last_ip.to_owned(),
                    mobile:          self.mobile,
                    password_scrypt: self.password.map(|p| self.scrypt_password(p)),
                    verified:        if self.ephemeral { None } else { Some(false) },
                })
                .get_result(self.conn)
                .chain_err(|| "Error inserting account")
        })
    }

    //
    // Private functions
    //

    fn scrypt_password(&self, password: &str) -> String {
        // We use some unwraps here with the logic that if something is wrong with our
        // scrypt generation, let's just blow up and find out about it.
        scrypt::scrypt_simple(
            password,
            &scrypt::ScryptParams::new(self.scrypt_log_n.clone().unwrap(), 8, 1),
        ).unwrap()
    }

    fn validate_request(&mut self) -> Result<()> {
        if self.ephemeral {
            return Ok(());
        }

        //
        // General checks (not intended to be user-facing)
        //

        if self.password.is_none() {
            bail!("`password` is required to create non-ephemeral accounts.");
        }

        if self.scrypt_log_n.is_none() {
            bail!("`scrypt_log_n` is required to create non-ephemeral accounts.");
        }

        //
        // User-facing validations
        //

        lazy_static! {
            // See: https://www.w3.org/TR/html51/sec-forms.html#valid-e-mail-address
            static ref EMAIL_REGEX: Regex = Regex::new("^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$").unwrap();
        }

        if let Some(email) = self.email {
            if !EMAIL_REGEX.is_match(email) {
                bail!(error::validation("Please specify a valid email address."))
            }
        }

        if let Some(password) = self.password {
            // Obviously we want to put in more sophisticated rules around password
            // complexity ...
            if password.len() < 8 {
                bail!(error::validation(
                    "Password must be at least 8 characters long."
                ))
            }
        }

        return Ok(());
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
            password:  None,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
    }

    #[test]
    fn test_account_create_permanent() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     Some("foo@example.com"),
            ephemeral: false,
            password:  Some("my-password"),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
    }

    #[test]
    fn test_account_create_invalid_ephemeral_with_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     Some("foo@example.com"),
            ephemeral: true,
            password:  None,
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
            password:  None,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!(
            "`password` is required to create non-ephemeral accounts.",
            e.description()
        );
    }

    #[test]
    fn test_account_create_invalid_permanent_bad_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     Some("foo"),
            ephemeral: false,
            password:  Some("my-password"),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: Please specify a valid email address.",
            format!("{}", e).as_str()
        );
    }

    #[test]
    fn test_account_create_invalid_permanent_short_password() {
        let mut bootstrap = TestBootstrap::new(Args {
            email:     Some("foo@example.com"),
            ephemeral: false,
            password:  Some("123"),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: Password must be at least 8 characters long.",
            format!("{}", e).as_str()
        );
    }

    //
    // Private types/functions
    //

    struct Args<'a> {
        email:     Option<&'a str>,
        ephemeral: bool,
        password:  Option<&'a str>,
    }

    struct TestBootstrap<'a> {
        _common: test_helpers::CommonTestBootstrap,
        args:    Args<'a>,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
    }

    impl<'a> TestBootstrap<'a> {
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
                    conn:         &*self.conn,
                    email:        self.args.email,
                    ephemeral:    self.args.ephemeral,
                    last_ip:      "1.2.3.4",
                    mobile:       false,
                    password:     self.args.password,
                    scrypt_log_n: Some(test_helpers::SCRYPT_LOG_N),
                },
                self.log.clone(),
            )
        }
    }
}
