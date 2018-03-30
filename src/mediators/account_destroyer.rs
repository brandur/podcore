use errors::*;
use model;
use schema;
use time_helpers;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub account: &'a model::Account,
    pub conn:    &'a PgConnection,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let num_account_podcast_deleted = self.delete_account_podcast(log)?;
        let num_account_deleted = self.delete_account(log)?;

        // TODO: account_podcast_episode
        // TODO: key

        Ok(RunResult {
            num_account_deleted,
            num_account_podcast_deleted,
        })
    }

    //
    // Steps
    //

    fn delete_account(&mut self, log: &Logger) -> Result<usize> {
        time_helpers::log_timed(&log.new(o!("step" => "delete_account")), |_log| {
            diesel::delete(schema::account::table)
                .filter(schema::account::id.eq(self.account.id))
                .execute(self.conn)
                .chain_err(|| "Error deleting account")
        })
    }

    fn delete_account_podcast(&mut self, log: &Logger) -> Result<usize> {
        time_helpers::log_timed(&log.new(o!("step" => "delete_account_podcast")), |_log| {
            diesel::delete(schema::account_podcast::table)
                .filter(schema::account_podcast::account_id.eq(self.account.id))
                .execute(self.conn)
                .chain_err(|| "Error deleting account podcasts")
        })
    }
}

pub struct RunResult {
    pub num_account_deleted:         usize,
    pub num_account_podcast_deleted: usize,
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_destroyer::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_account_destroy() {
        let mut bootstrap = TestBootstrap::new();

        test_data::account_podcast::insert_args(
            &bootstrap.log,
            &bootstrap.conn,
            test_data::account_podcast::Args {
                account: Some(&bootstrap.account),
            },
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(1, res.num_account_deleted);
        assert_eq!(1, res.num_account_podcast_deleted);
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
