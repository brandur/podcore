use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use chrono::Utc;
use diesel;
use diesel::pg::PgConnection;
use diesel::pg::upsert::excluded;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub account: &'a model::Account,
    pub conn:    &'a PgConnection,
    pub podcast: &'a model::Podcast,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account_podcast = self.upsert_account_podcast(log)?;
        Ok(RunResult { account_podcast })
    }

    //
    // Steps
    //

    fn upsert_account_podcast(&mut self, log: &Logger) -> Result<model::AccountPodcast> {
        let ins_account_podcast = insertable::AccountPodcast {
            account_id:      self.account.id,
            podcast_id:      self.podcast.id,
            subscribed_at:   Utc::now(),
            unsubscribed_at: None,
        };

        time_helpers::log_timed(&log.new(o!("step" => "upsert_account_podcast")), |_log| {
            diesel::insert_into(schema::account_podcast::table)
                .values(&ins_account_podcast)
                .on_conflict((
                    schema::account_podcast::account_id,
                    schema::account_podcast::podcast_id,
                ))
                .do_update()
                .set((
                    schema::account_podcast::subscribed_at
                        .eq(excluded(schema::account_podcast::subscribed_at)),
                    schema::account_podcast::unsubscribed_at
                        .eq(excluded(schema::account_podcast::unsubscribed_at)),
                ))
                .get_result(self.conn)
                .chain_err(|| "Error upserting account_podcast")
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
    use mediators::podcast_subscriber::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_podcast_subscribe() {
        let mut bootstrap = TestBootstrap::new();
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast.id);
    }

    #[test]
    fn test_podcast_subscribe_again() {
        let mut bootstrap = TestBootstrap::new();

        let id = {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_ne!(0, res.account_podcast.id);
            res.account_podcast.id
        };

        let next_id = {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_ne!(0, res.account_podcast.id);
            res.account_podcast.id
        };

        assert_eq!(id, next_id);
    }

    #[test]
    fn test_podcast_subscribe_again_after_unsubscribe() {
        let mut bootstrap = TestBootstrap::new();

        let id = {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_ne!(0, res.account_podcast.id);
            res.account_podcast.id
        };

        // Unsubscribe
        diesel::update(schema::account_podcast::table)
            .filter(schema::account_podcast::id.eq(id))
            .set(schema::account_podcast::unsubscribed_at.eq(Some(Utc::now())))
            .execute(&*bootstrap.conn)
            .unwrap();

        let next_id = {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_ne!(0, res.account_podcast.id);
            res.account_podcast.id
        };

        assert_eq!(id, next_id);
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        account: model::Account,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
        podcast: model::Podcast,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                account: test_data::account::insert(&log, &conn),
                podcast: test_data::podcast::insert(&log, &conn),

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
                    podcast: &self.podcast,
                },
                self.log.clone(),
            )
        }
    }
}
