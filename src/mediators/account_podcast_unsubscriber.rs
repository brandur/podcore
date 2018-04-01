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
    pub conn:            &'a PgConnection,
    pub account_podcast: &'a model::AccountPodcast,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account_podcast = self.update_account_podcast(log)?;
        Ok(RunResult { account_podcast })
    }

    //
    // Steps
    //

    fn update_account_podcast(&mut self, log: &Logger) -> Result<model::AccountPodcast> {
        time_helpers::log_timed(&log.new(o!("step" => "update_account_podcast")), |_log| {
            diesel::update(schema::account_podcast::table)
                .filter(schema::account_podcast::id.eq(self.account_podcast.id))
                .set(schema::account_podcast::unsubscribed_at.eq(Some(Utc::now())))
                .get_result(self.conn)
                .chain_err(|| "Error updating account_podcast")
        })
    }
}

pub struct RunResult {
    pub account_podcast: model::AccountPodcast,
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_podcast_unsubscriber::*;
    use model;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_podcast_unsubscribe() {
        let mut bootstrap = TestBootstrap::new();
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(None, res.account_podcast.unsubscribed_at);
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common:         test_helpers::CommonTestBootstrap,
        account_podcast: model::AccountPodcast,
        conn:            PooledConnection<ConnectionManager<PgConnection>>,
        log:             Logger,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            TestBootstrap {
                _common:         test_helpers::CommonTestBootstrap::new(),
                account_podcast: test_data::account_podcast::insert(&log, &conn),

                // Only move these after filling the above
                conn: conn,
                log:  log,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    account_podcast: &self.account_podcast,
                    conn:            &*self.conn,
                },
                self.log.clone(),
            )
        }
    }
}
