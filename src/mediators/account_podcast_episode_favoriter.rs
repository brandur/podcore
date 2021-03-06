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
    pub account:   &'a model::Account,
    pub conn:      &'a PgConnection,
    pub episode:   &'a model::Episode,
    pub favorited: bool,
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
        // This is a variant of the standard `insertable` designed to specifically
        // handle the case of updating `favorite`.
        let ins_episode = insertable::AccountPodcastEpisodeFavorite {
            account_podcast_id: account_podcast.id,
            episode_id:         self.episode.id,
            favorited:          self.favorited,
            updated_at:         Utc::now(),

            // We insert a listened seconds value of `0`, but we only expect this to take affect if
            // there was no previously existing row. If there was, we don't set it in the `DO
            // UPDATE` below.
            listened_seconds: Some(0),
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
                        schema::account_podcast_episode::favorited
                            .eq(excluded(schema::account_podcast_episode::favorited)),
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
    use mediators::account_podcast_episode_favoriter::*;
    use test_data;
    use test_helpers;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_account_podcast_episode_favorite_new() {
        let mut bootstrap = TestBootstrap::new(Args { favorited: true });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast_episode.id);
        assert!(res.account_podcast_episode.favorited);
    }

    #[test]
    fn test_account_podcast_episode_favorite_existing() {
        let mut bootstrap = TestBootstrap::new(Args { favorited: true });

        // Insert a new record so that upsert operates on that
        let existing = insert_account_podcast_episode(&bootstrap);
        assert_eq!(false, existing.favorited);

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // All these fields should be unchanged
        assert_eq!(existing.id, res.account_podcast_episode.id);
        assert_eq!(
            existing.listened_seconds,
            res.account_podcast_episode.listened_seconds
        );
        assert_eq!(existing.played, res.account_podcast_episode.played);

        // However, it's now a favorite
        assert!(res.account_podcast_episode.favorited);
    }

    #[test]
    fn test_account_podcast_episode_favorite_unfavorite_new() {
        let mut bootstrap = TestBootstrap::new(Args { favorited: false });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast_episode.id);
        assert_eq!(false, res.account_podcast_episode.favorited);
    }

    #[test]
    fn test_account_podcast_episode_favorite_unfavorite_existing() {
        let mut bootstrap = TestBootstrap::new(Args { favorited: true });

        // Insert a new record so that upsert operates on that
        let existing = insert_account_podcast_episode(&bootstrap);
        assert_eq!(false, existing.favorited);

        // Do a targeted `UPDATE` to set `favorite` on this existing row
        diesel::update(schema::account_podcast_episode::table)
            .set(schema::account_podcast_episode::favorited.eq(true))
            .execute(&*bootstrap.conn)
            .unwrap();

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // All these fields should be unchanged
        assert_eq!(existing.id, res.account_podcast_episode.id);
        assert_eq!(
            existing.listened_seconds,
            res.account_podcast_episode.listened_seconds
        );
        assert_eq!(existing.played, res.account_podcast_episode.played);

        // However, it's now a favorite
        assert!(res.account_podcast_episode.favorited);
    }

    //
    // Private types/functions
    //

    struct Args {
        favorited: bool,
    }

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        account: model::Account,
        args:    Args,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        episode: model::Episode,
        log:     Logger,
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
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    account:   &self.account,
                    conn:      &*self.conn,
                    episode:   &self.episode,
                    favorited: self.args.favorited,
                },
                self.log.clone(),
            )
        }
    }

    fn insert_account_podcast_episode(bootstrap: &TestBootstrap) -> model::AccountPodcastEpisode {
        test_data::account_podcast_episode::insert_args(
            &bootstrap.log,
            &bootstrap.conn,
            test_data::account_podcast_episode::Args {
                account: Some(&bootstrap.account),
                episode: Some(&bootstrap.episode),
            },
        )
    }
}
