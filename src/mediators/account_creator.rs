use errors::*;
use mediators;
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
    pub create_key:   bool,
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
        self.params_check()?;
        self.params_validate()?;
        self.check_existing_account(log)?;
        let password_scrypt = self.password.map(|p| self.scrypt_password(log, p));
        let account = self.insert_account(log, password_scrypt)?;
        let key = self.create_key(log, &account)?;
        Ok(RunResult { account, key })
    }

    //
    // Steps
    //

    fn create_key(&mut self, log: &Logger, account: &model::Account) -> Result<Option<model::Key>> {
        if !self.create_key {
            return Ok(None);
        }

        let res = mediators::key_creator::Mediator {
            account,
            conn: self.conn,
            expire_at: None,
        }.run(log)?;
        Ok(Some(res.key))
    }

    fn insert_account(
        &mut self,
        log: &Logger,
        password_scrypt: Option<String>,
    ) -> Result<model::Account> {
        time_helpers::log_timed(&log.new(o!("step" => "insert_account")), |_log| {
            diesel::insert_into(schema::account::table)
                .values(&insertable::Account {
                    email: self.email.map(|e| e.to_owned()),
                    ephemeral: self.ephemeral,
                    last_ip: self.last_ip.to_owned(),
                    mobile: self.mobile,
                    password_scrypt,
                    verified: if self.ephemeral { None } else { Some(false) },
                })
                .get_result(self.conn)
                .chain_err(|| "Error inserting account")
        })
    }

    //
    // Private functions
    //

    /// Checks whether an account with the given email address already exists.
    ///
    /// This isn't strictly necessary because our `UNIQUE` constraint will
    /// protect us regardless, but this gives the user a much better error.
    fn check_existing_account(&self, log: &Logger) -> Result<()> {
        if self.email.is_none() {
            return Ok(());
        }

        let email = self.email.unwrap();

        let email_exists =
            time_helpers::log_timed(&log.new(o!("step" => "select_existing_account")), |_log| {
                diesel::select(diesel::dsl::exists(
                    schema::account::table.filter(schema::account::email.eq(email)),
                )).get_result(self.conn)
                    .chain_err(|| "Error checking account existence")
            })?;

        if email_exists {
            bail!(user_errors::validation(
                "An account with that email already exists."
            ));
        }

        Ok(())
    }

    /// Scrypts the account's password (only called if one was supplied).
    ///
    /// Written as a separate step because scrypting can be a very expensive
    /// operation (easily on the order of full seconds with a high enough
    /// `log_n` value, and this gives us some timing insight into an scrypt
    /// that might be taking a long time).
    fn scrypt_password(&self, log: &Logger, password: &str) -> String {
        time_helpers::log_timed(&log.new(o!("step" => "scrypt_password")), |log| {
            let log_n = self.scrypt_log_n.unwrap();
            debug!(log, "Scrypting password"; "log_n" => log_n);

            // We use some unwraps here with the logic that if something is wrong with our
            // scrypt generation, let's just blow up and find out about it.
            scrypt::scrypt_simple(password, &scrypt::ScryptParams::new(log_n, 8, 1)).unwrap()
        })
    }

    /// Performs general checks on parameters. Not intended to be user-facing.
    fn params_check(&mut self) -> Result<()> {
        if self.ephemeral {
            return Ok(());
        }

        if self.password.is_none() {
            bail!("`password` is required to create non-ephemeral accounts.");
        }

        if self.scrypt_log_n.is_none() {
            bail!("`scrypt_log_n` is required to create non-ephemeral accounts.");
        }

        Ok(())
    }

    /// Performs validations on parameters. These are user facing.
    fn params_validate(&self) -> Result<()> {
        if self.ephemeral {
            return Ok(());
        }

        lazy_static! {
            // See: https://www.w3.org/TR/html51/sec-forms.html#valid-e-mail-address
            static ref EMAIL_REGEX: Regex = Regex::new("^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$").unwrap();
        }

        if let Some(email) = self.email {
            if email.is_empty() {
                bail!(user_errors::validation("Please specify an email address."))
            }

            if !EMAIL_REGEX.is_match(email) {
                bail!(user_errors::validation(
                    "Please specify a valid email address."
                ))
            }
        }

        if let Some(password) = self.password {
            if password.is_empty() {
                bail!(user_errors::validation("Please specify a password."))
            }

            // Obviously we want to put in more sophisticated rules around password
            // complexity ...
            if password.len() < 8 {
                bail!(user_errors::validation(
                    "Password must be at least 8 characters long."
                ))
            }
        }

        Ok(())
    }
}

pub struct RunResult {
    pub account: model::Account,

    /// A newly minted key for the account. A key is only created if the
    /// `create_key` parameter was set to `true`.
    pub key: Option<model::Key>,
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_creator::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_account_create_ephemeral() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      None,
            ephemeral:  true,
            password:   None,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
        assert!(res.key.is_none());
    }

    #[test]
    fn test_account_create_permanent() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some("foo@example.com"),
            ephemeral:  false,
            password:   Some("my-password"),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
        assert!(res.key.is_none());
    }

    #[test]
    fn test_account_create_with_key() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: true,
            email:      None,
            ephemeral:  true,
            password:   None,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account.id);
        assert!(res.key.is_some());
        assert_ne!(0, res.key.unwrap().id);
    }

    #[test]
    fn test_account_create_invalid_ephemeral_with_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some("foo@example.com"),
            ephemeral:  true,
            password:   None,
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
            create_key: false,
            email:      None,
            ephemeral:  false,
            password:   None,
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
    fn test_account_create_invalid_permanent_empty_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some(""),
            ephemeral:  false,
            password:   Some("my-password"),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: Please specify an email address.",
            format!("{}", e).as_str()
        );
    }

    #[test]
    fn test_account_create_invalid_permanent_bad_email() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some("foo"),
            ephemeral:  false,
            password:   Some("my-password"),
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
    fn test_account_create_invalid_permanent_empty_password() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some("foo@example.com"),
            ephemeral:  false,
            password:   Some(""),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: Please specify a password.",
            format!("{}", e).as_str()
        );
    }

    #[test]
    fn test_account_create_invalid_permanent_short_password() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some("foo@example.com"),
            ephemeral:  false,
            password:   Some("123"),
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

    #[test]
    fn test_account_create_invalid_email_exists() {
        let mut bootstrap = TestBootstrap::new(Args {
            create_key: false,
            email:      Some("foo@example.com"),
            ephemeral:  false,
            password:   Some("my-password"),
        });

        let _account = test_data::account::insert_args(
            &bootstrap.log,
            &bootstrap.conn,
            test_data::account::Args {
                email:     Some("foo@example.com"),
                ephemeral: false,
                mobile:    false,
            },
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!(
            "Validation failed: An account with that email already exists.",
            format!("{}", e).as_str()
        );
    }

    //
    // Private types/functions
    //

    struct Args<'a> {
        create_key: bool,
        email:      Option<&'a str>,
        ephemeral:  bool,
        password:   Option<&'a str>,
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
                    create_key:   self.args.create_key,
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
