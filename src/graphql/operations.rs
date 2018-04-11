use errors::*;
use model;
use schema;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use juniper;
use juniper::FieldResult;
use r2d2::PooledConnection;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::str::FromStr;

//
// Context
//

pub struct Context {
    pub account: model::Account,
    pub conn:    PooledConnection<ConnectionManager<PgConnection>>,
    pub log:     Logger,
}

impl Context {
    /// Convenience function for accessing a `Context`'s internal connection as
    /// a `PgConnection` reference (which is what most mediators take).
    #[inline]
    fn conn(&self) -> &PgConnection {
        &*self.conn
    }
}

impl juniper::Context for Context {}

//
// Mutations
//

#[derive(Default)]
pub struct Mutation;

impl Mutation {}

graphql_object!(
    Mutation: Context | &self | {

        description: "The root mutation object of the schema."

        field episode_favorited_update(&executor,
            episode_id: String as "The episode's ID.",
            favorited: bool as "True to set as favorited or false to set as not favorited."
        ) -> FieldResult<resource::AccountPodcastEpisode> as "An object representing the podcast episode for this user." {
            Ok(mutation::episode_favorited_update::execute(
                &executor.context().log,
                &executor.context().conn(),
                &mutation::episode_favorited_update::RawParams {
                    account:    &executor.context().account,
                    episode_id: &episode_id,
                    favorited,
                }
            )?)
        }

        field episode_played_update(&executor,
            episode_id: String as "The episode's ID.",
            played: bool as "True to set as played or false to set as not played."
        ) -> FieldResult<resource::AccountPodcastEpisode> as "An object representing the podcast episode for this user." {
            Ok(mutation::episode_played_update::execute(
                &executor.context().log,
                &executor.context().conn(),
                &mutation::episode_played_update::RawParams {
                    account:    &executor.context().account,
                    episode_id: &episode_id,
                    played,
                }
            )?)
        }

        // `juniper` does some function/parameter name mangling -- this is invoked for example as:
        //
        // ``` graphql
        // mutation {
        //   podcastSubscribedUpdate(podcastId: "1", subscribed: true) {
        //     id
        //   }
        // }
        // ```
        field podcast_subscribed_update(&executor,
            podcast_id: String as "The podcast's ID.",
            subscribed: bool as "True to subscribe or false to unsubscribe."
        ) -> FieldResult<Option<resource::AccountPodcast>> as "An object representing the added or removed subscribed, or null if unsubscribing and the account wasn't subscribed." {
            Ok(mutation::podcast_subscribed_update::execute(
                &executor.context().log,
                &mutation::podcast_subscribed_update::Params {
                    account:    &executor.context().account,
                    conn:       &executor.context().conn(),
                    podcast_id: &podcast_id,
                    subscribed: subscribed,
                }
            )?)
        }
    }
);

mod mutation {
    use errors::*;
    use graphql::operations::resource;
    use mediators;
    use model;
    use schema;

    use diesel::pg::PgConnection;
    use slog::Logger;

    pub mod episode_favorited_update {
        use graphql::operations::mutation::*;
        use time_helpers;

        use diesel::prelude::*;

        //
        // Raw parameters
        //

        pub struct RawParams<'a> {
            pub account:    &'a model::Account,
            pub episode_id: &'a str,
            pub favorited:  bool,
        }

        //
        // Coerced parameters
        //

        pub struct CoercedParams<'a> {
            pub account:    &'a model::Account,
            pub episode_id: i64,
            pub favorited:  bool,
        }

        impl<'a> CoercedParams<'a> {
            fn coerce(_log: &Logger, params: &RawParams<'a>) -> Result<CoercedParams<'a>> {
                use std::str::FromStr;
                Ok(CoercedParams {
                    account:    params.account,
                    episode_id: i64::from_str(params.episode_id)
                        .map_err(|e| error::bad_parameter("episode_id", &e))?,
                    favorited:  params.favorited,
                })
            }
        }

        //
        // Fetch
        //

        pub struct Fetches {
            account_podcast: model::AccountPodcast,
            episode:         model::Episode,
        }

        impl Fetches {
            fn fetch(log: &Logger, conn: &PgConnection, params: &CoercedParams) -> Result<Self> {
                time_helpers::log_timed(&log.new(o!("step" => "fetch")), |_log| {
                    let episode: model::Episode = schema::episode::table
                        .filter(schema::episode::id.eq(params.episode_id))
                        .first(conn)
                        .optional()?
                        .ok_or_else(|| error::not_found("episode", params.episode_id))?;

                    let account_podcast: model::AccountPodcast = schema::account_podcast::table
                        .filter(schema::account_podcast::account_id.eq(params.account.id))
                        .filter(schema::account_podcast::podcast_id.eq(episode.podcast_id))
                        .first(conn)
                        .optional()?
                        .ok_or_else(|| {
                            error::not_found_general(format!(
                                "Subscription for account {} on podcast {}",
                                params.account.id, episode.podcast_id
                            ))
                        })?;

                    Ok(Fetches {
                        account_podcast,
                        episode,
                    })
                })
            }
        }

        //
        // Execution
        //

        pub fn execute<'a>(
            log: &Logger,
            conn: &PgConnection,
            params: &RawParams<'a>,
        ) -> Result<resource::AccountPodcastEpisode> {
            let coerced = CoercedParams::coerce(&log, &params)?;
            let fetches = Fetches::fetch(&log, conn, &coerced)?;

            let res = mediators::account_podcast_episode_favoriter::Mediator {
                account_podcast: &fetches.account_podcast,
                conn:            conn,
                episode:         &fetches.episode,
                favorited:       params.favorited,
            }.run(log)?;

            Ok(resource::AccountPodcastEpisode::from(
                &res.account_podcast_episode,
            ))
        }

        //
        // Tests
        //

        #[cfg(test)]
        mod tests {
            use graphql::operations::mutation::episode_favorited_update::*;
            use model;
            use schema;
            use test_data;
            use test_helpers;

            use diesel;
            use r2d2::PooledConnection;
            use r2d2_diesel::ConnectionManager;

            #[test]
            fn test_mutation_episode_favorited_update_favorited() {
                let bootstrap = TestBootstrap::new();

                let episode = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: &bootstrap.episode.id.to_string(),
                        favorited:  true,
                    },
                ).unwrap();
                assert_ne!("0", episode.id);
                assert_eq!(bootstrap.episode.id.to_string(), episode.episode_id);
                assert!(episode.favorited);
            }

            #[test]
            fn test_mutation_episode_favorited_update_not_favorited() {
                let bootstrap = TestBootstrap::new();

                let episode = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: &bootstrap.episode.id.to_string(),
                        favorited:  false,
                    },
                ).unwrap();
                assert_ne!("0", episode.id);
                assert_eq!(bootstrap.episode.id.to_string(), episode.episode_id);
                assert_eq!(false, episode.favorited);
            }

            #[test]
            fn test_mutation_episode_favorited_update_no_episode() {
                let bootstrap = TestBootstrap::new();

                let err = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: "0",
                        favorited:  false,
                    },
                ).err()
                    .unwrap();
                assert_eq!(
                    format!("{}", error::not_found("episode", 0)),
                    format!("{}", err)
                );
            }

            #[test]
            fn test_mutation_episode_favorited_update_no_account_podcast() {
                let bootstrap = TestBootstrap::new();

                // Delete the `account_podcast_episode` and `account_podcast` (subscription)
                let num_deleted = diesel::delete(
                    schema::account_podcast_episode::table.filter(
                        schema::account_podcast_episode::account_podcast_id
                            .eq(bootstrap.account_podcast.id),
                    ),
                ).execute(&*bootstrap.conn)
                    .unwrap();
                assert_eq!(1, num_deleted);
                let num_deleted = diesel::delete(
                    schema::account_podcast::table
                        .filter(schema::account_podcast::id.eq(bootstrap.account_podcast.id)),
                ).execute(&*bootstrap.conn)
                    .unwrap();
                assert_eq!(1, num_deleted);

                let err = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: "0",
                        favorited:  false,
                    },
                ).err()
                    .unwrap();
                assert_eq!(
                    format!("{}", error::not_found("episode", 0)),
                    format!("{}", err)
                );
            }

            //
            // Private types/functions
            //

            struct TestBootstrap {
                _common:         test_helpers::CommonTestBootstrap,
                account:         model::Account,
                account_podcast: model::AccountPodcast,
                episode:         model::Episode,
                conn:            PooledConnection<ConnectionManager<PgConnection>>,
                log:             Logger,
            }

            impl TestBootstrap {
                fn new() -> TestBootstrap {
                    let conn = test_helpers::connection();
                    let log = test_helpers::log();

                    let account = test_data::account::insert(&log, &conn);
                    let account_podcast = test_data::account_podcast::insert_args(
                        &log,
                        &*conn,
                        test_data::account_podcast::Args {
                            account: Some(&account),
                        },
                    );
                    let episode: model::Episode = schema::episode::table
                        .filter(schema::episode::podcast_id.eq(account_podcast.podcast_id))
                        .first(&*conn)
                        .unwrap();
                    let _account_podcast_episode = test_data::account_podcast_episode::insert_args(
                        &log,
                        &*conn,
                        test_data::account_podcast_episode::Args {
                            account_podcast: Some(&account_podcast),
                            episode:         Some(&episode),
                        },
                    );

                    TestBootstrap {
                        _common:         test_helpers::CommonTestBootstrap::new(),
                        account:         account,
                        account_podcast: account_podcast,
                        episode:         episode,

                        // Only move these after filling the above
                        conn: conn,
                        log:  log,
                    }
                }
            }
        }
    }

    pub mod episode_played_update {
        use graphql::operations::mutation::*;
        use time_helpers;

        use diesel::prelude::*;

        //
        // Raw parameters
        //

        pub struct RawParams<'a> {
            pub account:    &'a model::Account,
            pub episode_id: &'a str,
            pub played:     bool,
        }

        //
        // Coerced parameters
        //

        pub struct CoercedParams<'a> {
            pub account:    &'a model::Account,
            pub episode_id: i64,
            pub played:     bool,
        }

        impl<'a> CoercedParams<'a> {
            fn coerce(_log: &Logger, params: &RawParams<'a>) -> Result<CoercedParams<'a>> {
                use std::str::FromStr;
                Ok(CoercedParams {
                    account:    params.account,
                    episode_id: i64::from_str(params.episode_id)
                        .map_err(|e| error::bad_parameter("episode_id", &e))?,
                    played:     params.played,
                })
            }
        }

        //
        // Fetch
        //

        pub struct Fetches {
            account_podcast: model::AccountPodcast,
            episode:         model::Episode,
        }

        impl Fetches {
            fn fetch(log: &Logger, conn: &PgConnection, params: &CoercedParams) -> Result<Self> {
                time_helpers::log_timed(&log.new(o!("step" => "fetch")), |_log| {
                    let episode: model::Episode = schema::episode::table
                        .filter(schema::episode::id.eq(params.episode_id))
                        .first(conn)
                        .optional()?
                        .ok_or_else(|| error::not_found("episode", params.episode_id))?;

                    let account_podcast: model::AccountPodcast = schema::account_podcast::table
                        .filter(schema::account_podcast::account_id.eq(params.account.id))
                        .filter(schema::account_podcast::podcast_id.eq(episode.podcast_id))
                        .first(conn)
                        .optional()?
                        .ok_or_else(|| {
                            error::not_found_general(format!(
                                "Subscription for account {} on podcast {}",
                                params.account.id, episode.podcast_id
                            ))
                        })?;

                    Ok(Fetches {
                        account_podcast,
                        episode,
                    })
                })
            }
        }

        //
        // Execution
        //

        pub fn execute<'a>(
            log: &Logger,
            conn: &PgConnection,
            params: &RawParams<'a>,
        ) -> Result<resource::AccountPodcastEpisode> {
            let coerced = CoercedParams::coerce(&log, &params)?;
            let fetches = Fetches::fetch(&log, conn, &coerced)?;

            // `listened_seconds` must be set to something (not `NULL`) if we're marking the
            // episode unplayed
            let listened_seconds = if params.played { None } else { Some(0) };

            let res = mediators::account_podcast_episode_upserter::Mediator {
                account_podcast:  &fetches.account_podcast,
                conn:             conn,
                episode:          &fetches.episode,
                listened_seconds: listened_seconds,
                played:           params.played,
            }.run(log)?;

            Ok(resource::AccountPodcastEpisode::from(
                &res.account_podcast_episode,
            ))
        }

        //
        // Tests
        //

        #[cfg(test)]
        mod tests {
            use graphql::operations::mutation::episode_played_update::*;
            use model;
            use schema;
            use test_data;
            use test_helpers;

            use diesel;
            use r2d2::PooledConnection;
            use r2d2_diesel::ConnectionManager;

            #[test]
            fn test_mutation_episode_played_update_played() {
                let bootstrap = TestBootstrap::new();

                let episode = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: &bootstrap.episode.id.to_string(),
                        played:     true,
                    },
                ).unwrap();
                assert_ne!("0", episode.id);
                assert_eq!(bootstrap.episode.id.to_string(), episode.episode_id);
                assert!(episode.played);
            }

            #[test]
            fn test_mutation_episode_played_update_unplayed() {
                let bootstrap = TestBootstrap::new();

                let episode = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: &bootstrap.episode.id.to_string(),
                        played:     false,
                    },
                ).unwrap();
                assert_ne!("0", episode.id);
                assert_eq!(bootstrap.episode.id.to_string(), episode.episode_id);
                assert_eq!(false, episode.played);
            }

            #[test]
            fn test_mutation_episode_played_update_no_episode() {
                let bootstrap = TestBootstrap::new();

                let err = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: "0",
                        played:     false,
                    },
                ).err()
                    .unwrap();
                assert_eq!(
                    format!("{}", error::not_found("episode", 0)),
                    format!("{}", err)
                );
            }

            #[test]
            fn test_mutation_episode_played_update_no_account_podcast() {
                let bootstrap = TestBootstrap::new();

                // Delete the `account_podcast_episode` and `account_podcast` (subscription)
                let num_deleted = diesel::delete(
                    schema::account_podcast_episode::table.filter(
                        schema::account_podcast_episode::account_podcast_id
                            .eq(bootstrap.account_podcast.id),
                    ),
                ).execute(&*bootstrap.conn)
                    .unwrap();
                assert_eq!(1, num_deleted);
                let num_deleted = diesel::delete(
                    schema::account_podcast::table
                        .filter(schema::account_podcast::id.eq(bootstrap.account_podcast.id)),
                ).execute(&*bootstrap.conn)
                    .unwrap();
                assert_eq!(1, num_deleted);

                let err = execute(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    &RawParams {
                        account:    &bootstrap.account,
                        episode_id: "0",
                        played:     false,
                    },
                ).err()
                    .unwrap();
                assert_eq!(
                    format!("{}", error::not_found("episode", 0)),
                    format!("{}", err)
                );
            }

            //
            // Private types/functions
            //

            struct TestBootstrap {
                _common:         test_helpers::CommonTestBootstrap,
                account:         model::Account,
                account_podcast: model::AccountPodcast,
                episode:         model::Episode,
                conn:            PooledConnection<ConnectionManager<PgConnection>>,
                log:             Logger,
            }

            impl TestBootstrap {
                fn new() -> TestBootstrap {
                    let conn = test_helpers::connection();
                    let log = test_helpers::log();

                    let account = test_data::account::insert(&log, &conn);
                    let account_podcast = test_data::account_podcast::insert_args(
                        &log,
                        &*conn,
                        test_data::account_podcast::Args {
                            account: Some(&account),
                        },
                    );
                    let episode: model::Episode = schema::episode::table
                        .filter(schema::episode::podcast_id.eq(account_podcast.podcast_id))
                        .first(&*conn)
                        .unwrap();
                    let _account_podcast_episode = test_data::account_podcast_episode::insert_args(
                        &log,
                        &*conn,
                        test_data::account_podcast_episode::Args {
                            account_podcast: Some(&account_podcast),
                            episode:         Some(&episode),
                        },
                    );

                    TestBootstrap {
                        _common:         test_helpers::CommonTestBootstrap::new(),
                        account:         account,
                        account_podcast: account_podcast,
                        episode:         episode,

                        // Only move these after filling the above
                        conn: conn,
                        log:  log,
                    }
                }
            }
        }
    }

    pub mod podcast_subscribed_update {
        use graphql::operations::mutation::*;

        use diesel::prelude::*;
        use std::str::FromStr;

        pub struct Params<'a> {
            pub account:    &'a model::Account,
            pub conn:       &'a PgConnection,
            pub podcast_id: &'a str,
            pub subscribed: bool,
        }

        pub fn execute<'a>(
            log: &Logger,
            params: &Params<'a>,
        ) -> Result<Option<resource::AccountPodcast>> {
            let podcast_id = i64::from_str(params.podcast_id)
                .map_err(|e| error::bad_parameter("podcast_id", &e))?;

            let account_podcast = if params.subscribed {
                // Subscription always produces an `AccountPodcast`
                Some(subscribe(log, params, podcast_id)?)
            } else {
                unsubscribe(log, params, podcast_id)?
            };
            Ok(account_podcast.map(|ref ap| resource::AccountPodcast::from(ap)))
        }

        //
        // Private functions
        //

        pub fn subscribe<'a>(
            log: &Logger,
            params: &Params<'a>,
            podcast_id: i64,
        ) -> Result<model::AccountPodcast> {
            let podcast: model::Podcast = schema::podcast::table
                .filter(schema::podcast::id.eq(podcast_id))
                .first(params.conn)
                .optional()?
                .ok_or_else(|| error::not_found("podcast", podcast_id))?;

            let res = mediators::account_podcast_subscriber::Mediator {
                account: params.account,
                conn:    params.conn,
                podcast: &podcast,
            }.run(log)?;

            Ok(res.account_podcast)
        }

        pub fn unsubscribe<'a>(
            log: &Logger,
            params: &Params<'a>,
            podcast_id: i64,
        ) -> Result<Option<model::AccountPodcast>> {
            let account_podcast: model::AccountPodcast = match schema::account_podcast::table
                .filter(schema::account_podcast::account_id.eq(params.account.id))
                .filter(schema::account_podcast::podcast_id.eq(podcast_id))
                .first(params.conn)
                .optional()?
            {
                Some(account_podcast) => account_podcast,
                None => return Ok(None),
            };

            let res = mediators::account_podcast_unsubscriber::Mediator {
                conn:            params.conn,
                account_podcast: &account_podcast,
            }.run(log)?;

            Ok(Some(res.account_podcast))
        }

        //
        // Tests
        //

        #[cfg(test)]
        mod tests {
            use graphql::operations::mutation::podcast_subscribed_update::*;
            use test_data;
            use test_helpers;

            use r2d2::PooledConnection;
            use r2d2_diesel::ConnectionManager;

            #[test]
            fn test_mutation_podcast_subscribed_update_subscribe() {
                let bootstrap = TestBootstrap::new();

                // Two `unwrap`s: once to verify successful execution, and once to verify that
                // we were indeed handed a `model::AccountPodcast` which is
                // always expected when subscribing.
                let account_podcast = execute(
                    &bootstrap.log,
                    &Params {
                        account:    &bootstrap.account,
                        conn:       &*bootstrap.conn,
                        podcast_id: &bootstrap.podcast.id.to_string(),
                        subscribed: true,
                    },
                ).unwrap()
                    .unwrap();
                assert_ne!("0", account_podcast.id);
                assert_eq!(bootstrap.account.id.to_string(), account_podcast.account_id);
                assert_eq!(bootstrap.podcast.id.to_string(), account_podcast.podcast_id);
            }

            #[test]
            fn test_mutation_podcast_subscribed_update_unsubscribe_subscribed() {
                let bootstrap = TestBootstrap::new();

                let account_podcast = test_data::account_podcast::insert_args(
                    &bootstrap.log,
                    &*bootstrap.conn,
                    test_data::account_podcast::Args {
                        account: Some(&bootstrap.account),
                    },
                );

                let account_podcast_resource = execute(
                    &bootstrap.log,
                    &Params {
                        account:    &bootstrap.account,
                        conn:       &*bootstrap.conn,
                        podcast_id: &account_podcast.podcast_id.to_string(),
                        subscribed: false,
                    },
                ).unwrap()
                    .unwrap();
                assert_eq!(account_podcast.id.to_string(), account_podcast_resource.id);
            }

            // Unsubscribing when not subscribed is a no-op, but returns a successful
            // response.
            #[test]
            fn test_mutation_podcast_subscribed_update_unsubscribed_not_subscribed() {
                let bootstrap = TestBootstrap::new();

                let account_podcast = execute(
                    &bootstrap.log,
                    &Params {
                        account:    &bootstrap.account,
                        conn:       &*bootstrap.conn,
                        podcast_id: &"0",
                        subscribed: false,
                    },
                ).unwrap();
                assert!(account_podcast.is_none());
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
            }
        }
    }
}

//
// Queries
//

#[derive(Default)]
pub struct Query;

impl Query {}

graphql_object!(Query: Context |&self| {
    description: "The root query object of the schema."

    field apiVersion() -> &str {
        "1.0"
    }

    field episode(&executor, podcast_id: String as "The podcast's ID.") ->
            FieldResult<Vec<resource::Episode>> as "A collection episodes for a podcast." {
        let id = i64::from_str(podcast_id.as_str()).
            chain_err(|| "Error parsing podcast ID")?;

        let context = executor.context();
        let results = schema::episode::table
            .filter(schema::episode::podcast_id.eq(id))
            .order(schema::episode::published_at.desc())
            .limit(50)
            .load::<model::Episode>(&*context.conn)
            .chain_err(|| "Error loading episodes from the database")?
            .iter()
            .map(resource::Episode::from)
            .collect::<Vec<_>>();
        Ok(results)
    }

    field podcast(&executor) -> FieldResult<Vec<resource::Podcast>> as "A collection of podcasts." {
        let context = executor.context();
        let results = schema::podcast::table
            .order(schema::podcast::title.asc())
            .limit(5)
            .load::<model::Podcast>(&*context.conn)
            .chain_err(|| "Error loading podcasts from the database")?
            .iter()
            .map(resource::Podcast::from)
            .collect::<Vec<_>>();
        Ok(results)
    }
});

//
// GraphQL resources
//

mod resource {
    use model;

    use chrono::{DateTime, Utc};

    #[derive(GraphQLObject)]
    pub struct AccountPodcast {
        #[graphql(description = "The account podcast's ID.")]
        pub id: String,

        #[graphql(description = "The account's ID.")]
        pub account_id: String,

        #[graphql(description = "The podcast's ID.")]
        pub podcast_id: String,
    }

    impl<'a> From<&'a model::AccountPodcast> for AccountPodcast {
        fn from(e: &model::AccountPodcast) -> Self {
            AccountPodcast {
                id:         e.id.to_string(),
                account_id: e.account_id.to_string(),
                podcast_id: e.podcast_id.to_string(),
            }
        }
    }

    #[derive(GraphQLObject)]
    pub struct AccountPodcastEpisode {
        #[graphql(description = "The account podcast episode's ID.")]
        pub id: String,

        #[graphql(description = "The episode's ID.")]
        pub episode_id: String,

        #[graphql(description = "Whether the episode has been favorited.")]
        pub favorited: bool,

        #[graphql(description = "Whether the episode has been fully played.")]
        pub played: bool,
    }

    impl<'a> From<&'a model::AccountPodcastEpisode> for AccountPodcastEpisode {
        fn from(e: &model::AccountPodcastEpisode) -> Self {
            AccountPodcastEpisode {
                id:         e.id.to_string(),
                episode_id: e.episode_id.to_string(),
                favorited:  e.favorited,
                played:     e.played,
            }
        }
    }

    #[derive(GraphQLObject)]
    pub struct Episode {
        #[graphql(description = "The episode's ID.")]
        pub id: String,

        #[graphql(description = "The episode's description.")]
        pub description: Option<String>,

        #[graphql(description = "Whether the episode is considered explicit.")]
        pub explicit: Option<bool>,

        #[graphql(description = "The episode's web link.")]
        pub link_url: Option<String>,

        #[graphql(description = "The episode's media link (i.e. where the audio can be found).")]
        pub media_url: String,

        #[graphql(description = "The episode's podcast's ID.")]
        pub podcast_id: String,

        #[graphql(description = "The episode's publishing date and time.")]
        pub published_at: DateTime<Utc>,

        #[graphql(description = "The episode's title.")]
        pub title: String,
    }

    impl<'a> From<&'a model::Episode> for Episode {
        fn from(e: &model::Episode) -> Self {
            Episode {
                id:           e.id.to_string(),
                description:  e.description.clone(),
                explicit:     e.explicit,
                link_url:     e.link_url.clone(),
                media_url:    e.media_url.to_owned(),
                podcast_id:   e.podcast_id.to_string(),
                published_at: e.published_at,
                title:        e.title.to_owned(),
            }
        }
    }

    #[derive(GraphQLObject)]
    pub struct Podcast {
        // IDs are exposed as strings because JS cannot store a fully 64-bit integer. This should
        // be okay because clients should be treating them as opaque tokens anyway.
        #[graphql(description = "The podcast's ID.")]
        pub id: String,

        #[graphql(description = "The podcast's image URL.")]
        pub image_url: Option<String>,

        #[graphql(description = "The podcast's language.")]
        pub language: Option<String>,

        #[graphql(description = "The podcast's RSS link URL.")]
        pub link_url: Option<String>,

        #[graphql(description = "The podcast's title.")]
        pub title: String,
    }

    impl<'a> From<&'a model::Podcast> for Podcast {
        fn from(p: &model::Podcast) -> Self {
            Podcast {
                id:        p.id.to_string(),
                image_url: p.image_url.clone(),
                language:  p.language.clone(),
                link_url:  p.link_url.clone(),
                title:     p.title.to_owned(),
            }
        }
    }
}
