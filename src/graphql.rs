use errors::*;
use model;
use schema;

use chrono::{DateTime, Utc};
use diesel::pg::PgConnection;
use diesel::prelude::*;
use juniper;
use juniper::FieldResult;
use r2d2::{Pool, PooledConnection};
use r2d2_diesel::ConnectionManager;
use std::str::FromStr;

type DieselConnection = PooledConnection<ConnectionManager<PgConnection>>;
type DieselPool = Pool<ConnectionManager<PgConnection>>;

pub struct Context {
    pool: DieselPool,
}

impl Context {
    pub fn new(pool: DieselPool) -> Self {
        Context { pool: pool }
    }

    fn get_conn(&self) -> Result<DieselConnection> {
        self.pool.get().map_err(Error::from)
    }
}

impl juniper::Context for Context {}

pub struct Mutation;

impl Mutation {
    pub fn new() -> Self {
        Self {}
    }
}

graphql_object!(
    Mutation: Context | &self | {

    description: "The root mutation object of the schema."

// field createHuman(&executor, new_human: NewHuman) -> FieldResult<Human> {
//    let db = executor.context().pool.get_connection()?;
//    let human: Human = db.insert_human(&new_human)?;
//    Ok(human)
// }
    }
);

#[derive(GraphQLObject)]
struct EpisodeObject {
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

impl<'a> From<&'a model::Episode> for EpisodeObject {
    fn from(e: &model::Episode) -> Self {
        EpisodeObject {
            id:           e.id.to_string(),
            description:  e.description.clone(),
            explicit:     e.explicit.clone(),
            link_url:     e.link_url.clone(),
            media_url:    e.media_url.to_owned(),
            podcast_id:   e.podcast_id.to_string(),
            published_at: e.published_at.clone(),
            title:        e.title.to_owned(),
        }
    }
}

#[derive(GraphQLObject)]
struct PodcastObject {
    // IDs are exposed as strings because JS cannot store a fully 64-bit integer. This should be
    // okay because clients should be treating them as opaque tokens anyway.
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

impl<'a> From<&'a model::Podcast> for PodcastObject {
    fn from(p: &model::Podcast) -> Self {
        PodcastObject {
            id:        p.id.to_string(),
            image_url: p.image_url.clone(),
            language:  p.language.clone(),
            link_url:  p.link_url.clone(),
            title:     p.title.to_owned(),
        }
    }
}

pub struct Query;

impl Query {
    pub fn new() -> Self {
        Self {}
    }
}

graphql_object!(Query: Context |&self| {
    description: "The root query object of the schema."

    field apiVersion() -> &str {
        "1.0"
    }

    field episodes(&executor, podcast_id: String as "The podcast's ID.") ->
            FieldResult<Vec<EpisodeObject>> as "A collection episodes for a podcast." {
        let id = i64::from_str(podcast_id.as_str()).
            chain_err(|| "Error parsing podcast ID")?;

        let context = executor.context();
        let results = schema::episode::table
            .filter(schema::episode::podcast_id.eq(id))
            .order(schema::episode::published_at.desc())
            .limit(20)
            .load::<model::Episode>(&*context.get_conn()?)
            .chain_err(|| "Error loading episodes from the database")?
            .iter()
            .map(|p| EpisodeObject::from(p) )
            .collect::<Vec<_>>();
        Ok(results)
    }

    field podcasts(&executor) -> FieldResult<Vec<PodcastObject>> as "A collection of podcasts." {
        let context = executor.context();
        let results = schema::podcasts::table
            .order(schema::podcasts::title.asc())
            .limit(5)
            .load::<model::Podcast>(&*context.get_conn()?)
            .chain_err(|| "Error loading podcasts from the database")?
            .iter()
            .map(|p| PodcastObject::from(p) )
            .collect::<Vec<_>>();
        Ok(results)
    }
});
