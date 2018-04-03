use errors::*;
use mediators;
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

        // `juniper` does some parameter name mangling -- this is invoked for example as:
        //
        // ``` graphql
        // mutation {
        //   subscribe(podcastId: "1") {
        //     id
        //   }
        // }
        // ```
        field subscribe(&executor, podcast_id: String as "The podcast's ID.") -> FieldResult<resource::AccountPodcast> as "The object representing the subscription." {
            let podcast_id = i64::from_str(podcast_id.as_str()).
                chain_err(|| "Error parsing podcast ID")?;

            let podcast: Option<model::Podcast> = schema::podcast::table
                .filter(schema::podcast::id.eq(podcast_id))
                .first(executor.context().conn())
                .optional()?;

            if podcast.is_none() {
                bail!(ErrorKind::NotFound("podcast".to_owned(), podcast_id));
            }
            let podcast = podcast.unwrap();

            let account_podcast = mediators::account_podcast_subscriber::Mediator {
                account: &executor.context().account,
                conn: executor.context().conn(),
                podcast: &podcast,
            }.run(&executor.context().log)?.account_podcast;

            Ok(resource::AccountPodcast::from(&account_podcast))
        }
    }
);

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
    }

    impl<'a> From<&'a model::AccountPodcast> for AccountPodcast {
        fn from(e: &model::AccountPodcast) -> Self {
            AccountPodcast {
                id: e.id.to_string(),
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
