use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use chrono::Utc;
use diesel;
use diesel::pg::upsert::excluded;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub account:          &'a model::Account,
    pub conn:             &'a PgConnection,
    pub episode:          &'a model::Episode,
    pub listened_seconds: Option<i64>,
    pub played:           bool,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account_podcast = self.upsert_account_podcast(log)?;
        let account_podcast_episode = self.upsert_account_podcast_episode(log, &account_podcast)?;
        Ok(RunResult {
            account_podcast_episode,
        })
    }

    //
    // Steps
    //

    fn upsert_account_podcast(&mut self, log: &Logger) -> Result<model::AccountPodcast> {
        let ins_account_podcast = insertable::AccountPodcast {
            account_id:      self.account.id,
            podcast_id:      self.episode.podcast_id,
            subscribed_at:   None,
            unsubscribed_at: None,
        };

        let account_podcast: Option<model::AccountPodcast> =
            time_helpers::log_timed(&log.new(o!("step" => "upsert_account_podcast")), |_log| {
                diesel::insert_into(schema::account_podcast::table)
                    .values(&ins_account_podcast)
                    .on_conflict((
                        schema::account_podcast::account_id,
                        schema::account_podcast::podcast_id,
                    ))
                    .do_nothing()
                    .get_result(self.conn)
                    .optional()
                    .chain_err(|| "Error upserting account podcast")
            })?;

        // One slightly unfortunate quirk of upsert is that on a conflict, `DO NOTHING`
        // won't return you the row that's already in there, even if you tack
        // on a `RETURNING *`, so we have to select the existing one in another
        // query. Ask Peter about this later.
        if account_podcast.is_some() {
            return Ok(account_podcast.unwrap());
        }

        time_helpers::log_timed(&log.new(o!("step" => "select_account_podcast")), |_log| {
            schema::account_podcast::table
                .filter(schema::account_podcast::account_id.eq(self.account.id))
                .filter(schema::account_podcast::podcast_id.eq(self.episode.podcast_id))
                .get_result(self.conn)
                .chain_err(|| "Error selecting account podcast")
        })
    }

    fn upsert_account_podcast_episode(
        &mut self,
        log: &Logger,
        account_podcast: &model::AccountPodcast,
    ) -> Result<model::AccountPodcastEpisode> {
        let ins_episode = insertable::AccountPodcastEpisode {
            account_podcast_id: account_podcast.id,
            episode_id:         self.episode.id,
            listened_seconds:   self.listened_seconds,
            played:             self.played,
            updated_at:         Utc::now(),
        };

        time_helpers::log_timed(
            &log.new(o!("step" => "upsert_account_podcast_episode")),
            |_log| {
                diesel::insert_into(schema::account_podcast_episode::table)
                    .values(&ins_episode)
                    .on_conflict((
                        schema::account_podcast_episode::account_podcast_id,
                        schema::account_podcast_episode::episode_id,
                    ))
                    .do_update()
                    .set((
                        schema::account_podcast_episode::listened_seconds
                            .eq(excluded(schema::account_podcast_episode::listened_seconds)),
                        schema::account_podcast_episode::played
                            .eq(excluded(schema::account_podcast_episode::played)),
                        schema::account_podcast_episode::updated_at
                            .eq(excluded(schema::account_podcast_episode::updated_at)),
                    ))
                    .get_result(self.conn)
                    .chain_err(|| "Error upserting account podcast episode")
            },
        )
    }
}

pub struct RunResult {
    pub account_podcast_episode: model::AccountPodcastEpisode,
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::account_podcast_episode_upserter::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_account_podcast_episode_upsert_partially_played() {
        let mut bootstrap = TestBootstrap::new(Args {
            listened_seconds: Some(10),
            played:           false,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast_episode.id);
    }

    #[test]
    fn test_account_podcast_episode_upsert_played() {
        let mut bootstrap = TestBootstrap::new(Args {
            listened_seconds: None,
            played:           true,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast_episode.id);
    }

    #[test]
    fn test_account_podcast_episode_upsert_invalid_played_with_seconds() {
        let mut bootstrap = TestBootstrap::new(Args {
            listened_seconds: Some(10),
            played:           true,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!("Error upserting account podcast episode", e.description());
    }

    #[test]
    fn test_account_podcast_episode_upsert_invalid_unplayed_without_seconds() {
        let mut bootstrap = TestBootstrap::new(Args {
            listened_seconds: None,
            played:           false,
        });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);
        assert!(res.is_err());
        let e = res.err().unwrap();
        assert_eq!("Error upserting account podcast episode", e.description());
    }

    // The mediator will insert an `account_podcast` row automatically if one was
    // missing. Here we check the result if there was one already.
    #[test]
    fn test_account_podcast_episode_upsert_existing_account_podcast() {
        let mut bootstrap = TestBootstrap::new(Args {
            listened_seconds: None,
            played:           true,
        });

        let account_podcast = test_data::account_podcast::insert_args(
            &bootstrap.log,
            &*bootstrap.conn,
            test_data::account_podcast::Args {
                account: Some(&bootstrap.account),
                podcast: Some(&bootstrap.podcast),
            },
        );

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(
            account_podcast.id,
            res.account_podcast_episode.account_podcast_id
        );
    }

    //
    // Private types/functions
    //

    struct Args {
        listened_seconds: Option<i64>,
        played:           bool,
    }

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        account: model::Account,
        args:    Args,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        episode: model::Episode,
        log:     Logger,
        podcast: model::Podcast,
    }

    impl TestBootstrap {
        fn new(args: Args) -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            let account = test_data::account::insert(&log, &*conn);
            let podcast = test_data::podcast::insert(&log, &*conn);
            let episode = test_data::episode::first(&log, &*conn, &podcast);

            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                account,
                args,
                conn,
                episode,
                log,
                podcast,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    account:          &self.account,
                    conn:             &*self.conn,
                    episode:          &self.episode,
                    listened_seconds: self.args.listened_seconds.clone(),
                    played:           self.args.played,
                },
                self.log.clone(),
            )
        }
    }
}
