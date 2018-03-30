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
    pub conn:    &'a PgConnection,
    pub last_ip: &'a str,
    pub secret:  &'a str,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        // We don't want secrets in logs, so we rely on this statement being compiled
        // out in a release build because it's `debug!`
        debug!(log, "Authenticating key"; "secret" => self.secret);

        let key = self.select_key(log, self.secret)?;
        if key.is_none() {
            info!(log, "No valid key with matching secret");
            return Ok(RunResult { account: None });
        }

        let key = key.unwrap();
        info!(log, "Found matching key"; "key" => key.id);

        let account = self.touch_and_select_account(log, &key)?;
        info!(log, "Found account"; "account" => account.id);

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
                .set((
                    schema::account::last_ip.eq(self.last_ip),
                    schema::account::last_seen_at.eq(Utc::now()),
                ))
                .get_result(self.conn)
                .chain_err(|| "Error touching account")
        })
    }

    fn select_key(&mut self, log: &Logger, secret: &str) -> Result<Option<model::Key>> {
        time_helpers::log_timed(&log.new(o!("step" => "select_key")), |_log| {
            schema::key::table
                .filter(schema::key::secret.eq(secret))
                .filter(
                    schema::key::expire_at
                        .is_null()
                        .or(schema::key::expire_at.ge(Utc::now())),
                )
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

#[cfg(test)]
mod tests {
    use mediators::account_authenticator::*;
    use test_data;
    use test_helpers;

    use chrono::{DateTime, Utc};
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use time::Duration;

    #[test]
    fn test_account_authenticate_no_expiry() {
        let mut bootstrap = TestBootstrap::new(Args {
            key_expire_at: None,
        });

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert!(res.account.is_some());

        let account = res.account.unwrap();
        assert_eq!(TEST_NEW_IP, account.last_ip);
    }

    #[test]
    fn test_account_authenticate_with_expiry() {
        let mut bootstrap = TestBootstrap::new(Args {
            key_expire_at: Some(Utc::now() + Duration::days(1)),
        });

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert!(res.account.is_some());

        let account = res.account.unwrap();
        assert_eq!(TEST_NEW_IP, account.last_ip);
    }

    #[test]
    fn test_account_authenticate_invalid_deleted() {
        let mut bootstrap = TestBootstrap::new(Args {
            key_expire_at: None,
        });

        // Delete the key completely
        diesel::delete(schema::key::table)
            .filter(schema::key::id.eq(bootstrap.key.id))
            .execute(&*bootstrap.conn)
            .unwrap();

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert!(res.account.is_none());
    }

    #[test]
    fn test_account_authenticate_invalid_expired() {
        let mut bootstrap = TestBootstrap::new(Args {
            key_expire_at: Some(Utc::now() - Duration::days(1)),
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();
        assert!(res.account.is_none());
    }

    //
    // Private types/functions
    //

    static TEST_NEW_IP: &'static str = "4.5.6.7";

    struct Args {
        key_expire_at: Option<DateTime<Utc>>,
    }

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        key:     model::Key,
        log:     Logger,
    }

    impl TestBootstrap {
        fn new(args: Args) -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            let key = test_data::key::insert_args(
                &log,
                &conn,
                test_data::key::Args {
                    expire_at: args.key_expire_at,
                },
            );

            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                key,

                // Only move these after filling the above
                conn: conn,
                log: log,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:    &*self.conn,
                    last_ip: TEST_NEW_IP,
                    secret:  &self.key.secret,
                },
                self.log.clone(),
            )
        }
    }
}
