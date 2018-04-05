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

        // `juniper` does some function/parameter name mangling -- this is invoked for example as:
        //
        // ``` graphql
        // mutation {
        //   accountPodcastSubscribe(podcastId: "1") {
        //     id
        //   }
        // }
        // ```
        field account_podcast_subscribe(&executor,
            podcast_id: String as "The podcast's ID."
        ) -> FieldResult<resource::AccountPodcast> as "An object representing the subscription." {
            Ok(mutation::account_podcast_subscribe::execute(
                &executor.context().log,
                &mutation::account_podcast_subscribe::Params {
                    account:    &executor.context().account,
                    conn:       &executor.context().conn(),
                    podcast_id: &podcast_id,
                }
            )?)
        }

        field account_podcast_unsubscribe(&executor,
            podcast_id: String as "The podcast's ID."
        ) -> FieldResult<Option<resource::AccountPodcast>> as "An object representing the removed subscription, or null if the account wasn't subscribed." {
            Ok(mutation::account_podcast_unsubscribe::execute(
                &executor.context().log,
                &mutation::account_podcast_unsubscribe::Params {
                    account:    &executor.context().account,
                    conn:       &executor.context().conn(),
                    podcast_id: &podcast_id,
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

    pub mod account_podcast_subscribe {
        use graphql::operations::mutation::*;

        use diesel::prelude::*;
        use std::str::FromStr;

        pub struct Params<'a> {
            pub account:    &'a model::Account,
            pub conn:       &'a PgConnection,
            pub podcast_id: &'a str,
        }

        pub fn execute<'a>(log: &Logger, params: &Params<'a>) -> Result<resource::AccountPodcast> {
            let podcast_id =
                i64::from_str(params.podcast_id).map_err(|e| bad_parameter("podcast_id", &e))?;

            let podcast: model::Podcast = schema::podcast::table
                .filter(schema::podcast::id.eq(podcast_id))
                .first(params.conn)
                .optional()?
                .ok_or_else(|| ErrorKind::NotFound("podcast".to_owned(), podcast_id))?;

            let account_podcast = mediators::account_podcast_subscriber::Mediator {
                account: params.account,
                conn:    params.conn,
                podcast: &podcast,
            }.run(log)?
                .account_podcast;

            Ok(resource::AccountPodcast::from(&account_podcast))
        }

        #[cfg(test)]
        mod tests {
            use graphql::operations::mutation::account_podcast_subscribe::*;
            use test_data;
            use test_helpers;

            use r2d2::PooledConnection;
            use r2d2_diesel::ConnectionManager;

            #[test]
            fn test_mutation_account_podcast_subscribe() {
                let bootstrap = TestBootstrap::new();

                let account_podcast = execute(
                    &bootstrap.log,
                    &Params {
                        account:    &bootstrap.account,
                        conn:       &*bootstrap.conn,
                        podcast_id: &bootstrap.podcast.id.to_string(),
                    },
                ).unwrap();
                assert_ne!("0", account_podcast.id);
                assert_eq!(bootstrap.account.id.to_string(), account_podcast.account_id);
                assert_eq!(bootstrap.podcast.id.to_string(), account_podcast.podcast_id);
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

    pub mod account_podcast_unsubscribe {
        use graphql::operations::mutation::*;

        use diesel::prelude::*;
        use std::str::FromStr;

        pub struct Params<'a> {
            pub account:    &'a model::Account,
            pub conn:       &'a PgConnection,
            pub podcast_id: &'a str,
        }

        pub fn execute<'a>(
            log: &Logger,
            params: &Params<'a>,
        ) -> Result<Option<resource::AccountPodcast>> {
            let podcast_id =
                i64::from_str(params.podcast_id).map_err(|e| bad_parameter("podcast_id", &e))?;

            let account_podcast: model::AccountPodcast = match schema::account_podcast::table
                .filter(schema::account_podcast::account_id.eq(params.account.id))
                .filter(schema::account_podcast::podcast_id.eq(podcast_id))
                .first(params.conn)
                .optional()?
            {
                Some(account_podcast) => account_podcast,
                None => return Ok(None),
            };

            let account_podcast = mediators::account_podcast_unsubscriber::Mediator {
                conn:            params.conn,
                account_podcast: &account_podcast,
            }.run(log)?
                .account_podcast;

            Ok(Some(resource::AccountPodcast::from(&account_podcast)))
        }

        #[cfg(test)]
        mod tests {
            use graphql::operations::mutation::account_podcast_unsubscribe::*;
            use test_data;
            use test_helpers;

            use r2d2::PooledConnection;
            use r2d2_diesel::ConnectionManager;

            #[test]
            fn test_mutation_account_podcast_unsubscribe_subscribed() {
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
                    },
                ).unwrap()
                    .unwrap();
                assert_eq!(account_podcast.id.to_string(), account_podcast_resource.id);
            }

            // Unsubscribing when not subscribed is a no-op, but returns a successful
            // response.
            #[test]
            fn test_mutation_account_podcast_unsubscribe_not_subscribed() {
                let bootstrap = TestBootstrap::new();

                let account_podcast = execute(
                    &bootstrap.log,
                    &Params {
                        account:    &bootstrap.account,
                        conn:       &*bootstrap.conn,
                        podcast_id: &"0",
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
            }
        }
    }

    //
    // Functions
    //

    #[inline]
    pub fn bad_parameter<E: ::std::error::Error>(name: &str, e: &E) -> Error {
        // `format!` invokes the error's `Display` trait implementation
        ErrorKind::BadParameter(name.to_owned(), format!("{}", e).to_owned()).into()
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
