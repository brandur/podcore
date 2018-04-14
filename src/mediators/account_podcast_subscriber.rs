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
    pub account:    &'a model::Account,
    pub conn:       &'a PgConnection,
    pub podcast:    &'a model::Podcast,
    pub subscribed: bool,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        // Shotcut so that we can skip inserting a row in the case where an unsubscribe
        // was requested, but the account was not subscribed in the first place.
        if !self.subscribed && !self.account_podcast_exists(log)? {
            return Ok(RunResult {
                account_podcast: None,
            });
        }

        let account_podcast = if self.subscribed {
            self.upsert_account_podcast_subscribed(log)?
        } else {
            self.upsert_account_podcast_unsubscribed(log)?
        };

        Ok(RunResult {
            account_podcast: Some(account_podcast),
        })
    }

    //
    // Steps
    //

    fn account_podcast_exists(&mut self, log: &Logger) -> Result<bool> {
        // TODO: time_helpers
        time_helpers::log_timed(&log.new(o!("step" => "check_account_podcast")), |_log| {
            diesel::select(diesel::dsl::exists(
                schema::account_podcast::table
                    .filter(schema::account_podcast::account_id.eq(self.account.id))
                    .filter(schema::account_podcast::podcast_id.eq(self.podcast.id)),
            )).get_result(self.conn)
                .chain_err(|| "Error checking account podcast existence")
        })
    }

    fn upsert_account_podcast_subscribed(&mut self, log: &Logger) -> Result<model::AccountPodcast> {
        let ins_account_podcast = insertable::AccountPodcast {
            account_id:      self.account.id,
            podcast_id:      self.podcast.id,
            subscribed_at:   Some(Utc::now()),
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
                .chain_err(|| "Error upserting account podcast")
        })
    }

    fn upsert_account_podcast_unsubscribed(
        &mut self,
        log: &Logger,
    ) -> Result<model::AccountPodcast> {
        let ins_account_podcast = insertable::AccountPodcast {
            account_id:      self.account.id,
            podcast_id:      self.podcast.id,
            subscribed_at:   None,
            unsubscribed_at: Some(Utc::now()),
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
                    // Don't set `subscribed_at` here -- we want to leave its existing value
                    schema::account_podcast::unsubscribed_at
                        .eq(excluded(schema::account_podcast::unsubscribed_at)),
                ))
                .get_result(self.conn)
                .chain_err(|| "Error upserting account podcast")
        })
    }
}

pub struct RunResult {
    pub account_podcast: Option<model::AccountPodcast>,
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_podcast_subscriber::*;
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
