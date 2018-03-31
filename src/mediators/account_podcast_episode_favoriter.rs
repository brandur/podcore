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
    pub account_podcast: &'a model::AccountPodcast,
    pub conn:            &'a PgConnection,
    pub episode:         &'a model::Episode,
    pub favorite:        bool,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account_podcast_episode = self.upsert_account_podcast_episode(log)?;
        Ok(RunResult {
            account_podcast_episode,
        })
    }

    //
    // Steps
    //

    fn upsert_account_podcast_episode(
        &mut self,
        log: &Logger,
    ) -> Result<model::AccountPodcastEpisode> {
        // This is a variant of the standard `insertable` designed to specifically
        // handle the case of updating `favorite`.
        let ins_episode = insertable::AccountPodcastEpisodeFavorite {
            account_podcast_id: self.account_podcast.id,
            episode_id:         self.episode.id,
            favorite:           self.favorite,
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
                        schema::account_podcast_episode::favorite
                            .eq(excluded(schema::account_podcast_episode::favorite)),
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
    fn test_account_podcast_episode_favorite() {
        let mut bootstrap = TestBootstrap::new(Args { favorite: true });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast_episode.id);
        assert!(res.account_podcast_episode.favorite);
    }

    #[test]
    fn test_account_podcast_episode_unfavorite() {
        let mut bootstrap = TestBootstrap::new(Args { favorite: false });
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_ne!(0, res.account_podcast_episode.id);
        assert_eq!(false, res.account_podcast_episode.favorite);
    }

    //
    // Private types/functions
    //

    struct Args {
        favorite: bool,
    }

    struct TestBootstrap {
        _common:         test_helpers::CommonTestBootstrap,
        account_podcast: model::AccountPodcast,
        args:            Args,
        episode:         model::Episode,
        conn:            PooledConnection<ConnectionManager<PgConnection>>,
        log:             Logger,
    }

    impl TestBootstrap {
        fn new(args: Args) -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();

            let account_podcast = test_data::account_podcast::insert(&log, &*conn);
            let episode: model::Episode = schema::episode::table
                .filter(schema::episode::podcast_id.eq(account_podcast.podcast_id))
                .limit(1)
                .get_result(&*conn)
                .unwrap();

            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                account_podcast,
                args,
                conn,
                episode,
                log,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    account_podcast: &self.account_podcast,
                    conn:            &*self.conn,
                    episode:         &self.episode,
                    favorite:        self.args.favorite,
                },
                self.log.clone(),
            )
        }
    }
}
