extern crate chrono;
#[macro_use]
extern crate diesel;
#[macro_use]
extern crate error_chain;
extern crate iron;
#[macro_use]
extern crate juniper;
extern crate juniper_iron;
extern crate mount;
extern crate r2d2;
extern crate r2d2_diesel;
extern crate serde;
extern crate time;

mod schema;

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::pg::PgConnection;
use iron::prelude::*;
use iron::{typemap, AfterMiddleware, BeforeMiddleware};
use juniper::FieldResult;
use juniper_iron::{GraphQLHandler, GraphiQLHandler};
use mount::Mount;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use std::env;
use std::str::FromStr;
use time::precise_time_ns;

//
// Init
//

// Create the Error, ErrorKind, ResultExt, and Result types
error_chain!{}

//
// Model
//

type DieselConnection = r2d2::PooledConnection<ConnectionManager<PgConnection>>;

#[derive(Queryable)]
pub struct Directory {
    pub id:   i64,
    pub name: String,
}

#[derive(Queryable)]
pub struct DirectoryPodcast {
    pub id:           i64,
    pub directory_id: i64,
    pub feed_url:     String,
    pub podcast_id:   i64,
    pub vendor_id:    String,
}

#[derive(Queryable)]
pub struct DirectorySearch {
    pub id:           i64,
    pub directory_id: i64,
    pub query:        String,
    pub retrieved_at: DateTime<Utc>,
}

#[derive(Queryable)]
pub struct Episode {
    pub id:           i64,
    pub description:  String,
    pub explicit:     bool,
    pub media_type:   String,
    pub media_url:    String,
    pub guid:         String,
    pub link_url:     String,
    pub podcast_id:   i64,
    pub published_at: DateTime<Utc>,
    pub title:        String,
}

#[derive(Queryable)]
pub struct Podcast {
    pub id:        i64,
    pub feed_url:  String,
    pub image_url: String,
    pub language:  String,
    pub link_url:  String,
    pub title:     String,
}

#[derive(Queryable)]
pub struct PodcastFeedContent {
    pub id:           i64,
    pub content:      String,
    pub podcast_id:   i64,
    pub retrieved_at: DateTime<Utc>,
}

struct Context {
    pool: Pool<ConnectionManager<PgConnection>>,
}

impl Context {
    fn get_conn(&self) -> Result<DieselConnection> {
        self.pool
            .get()
            .chain_err(|| "Error acquiring connection from database pool")
    }
}

impl juniper::Context for Context {}

//
// GraphQL
//

struct Mutation;

graphql_object!(
    Mutation: Context | &self | {

    description: "The root mutation object of the schema."

        //field createHuman(&executor, new_human: NewHuman) -> FieldResult<Human> {
        //    let db = executor.context().pool.get_connection()?;
        //    let human: Human = db.insert_human(&new_human)?;
        //    Ok(human)
        //}
    }
);

#[derive(GraphQLObject)]
struct EpisodeObject {
    #[graphql(description = "The episode's ID.")]
    pub id: String,

    #[graphql(description = "The episode's description.")]
    pub description: String,

    #[graphql(description = "Whether the episode is considered explicit.")]
    pub explicit: bool,

    #[graphql(description = "The episode's web link.")]
    pub link_url: String,

    #[graphql(description = "The episode's media link (i.e. where the audio can be found).")]
    pub media_url: String,

    #[graphql(description = "The episode's podcast's ID.")]
    pub podcast_id: String,

    #[graphql(description = "The episode's publishing date and time.")]
    pub published_at: DateTime<Utc>,

    #[graphql(description = "The episode's title.")]
    pub title: String,
}

impl<'a> From<&'a Episode> for EpisodeObject {
    fn from(e: &Episode) -> Self {
        EpisodeObject {
            id:           e.id.to_string(),
            description:  e.description.to_string(),
            explicit:     e.explicit,
            link_url:     e.link_url.to_owned(),
            media_url:    e.media_url.to_owned(),
            podcast_id:   e.podcast_id.to_string(),
            published_at: e.published_at,
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

    #[graphql(description = "The podcast's RSS feed URL.")]
    pub feed_url: String,

    #[graphql(description = "The podcast's image URL.")]
    pub image_url: String,

    #[graphql(description = "The podcast's language.")]
    pub language: String,

    #[graphql(description = "The podcast's RSS link URL.")]
    pub link_url: String,

    #[graphql(description = "The podcast's title.")]
    pub title: String,
}

impl<'a> From<&'a Podcast> for PodcastObject {
    fn from(p: &Podcast) -> Self {
        PodcastObject {
            id:        p.id.to_string(),
            feed_url:  p.feed_url.to_owned(),
            image_url: p.image_url.to_owned(),
            language:  p.language.to_owned(),
            link_url:  p.link_url.to_owned(),
            title:     p.title.to_owned(),
        }
    }
}

struct Query;

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
        let results = schema::episodes::table
            .filter(schema::episodes::podcast_id.eq(id))
            .order(schema::episodes::published_at.desc())
            .limit(20)
            .load::<Episode>(&*context.get_conn()?)
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
            .load::<Podcast>(&*context.get_conn()?)
            .chain_err(|| "Error loading podcasts from the database")?
            .iter()
            .map(|p| PodcastObject::from(p) )
            .collect::<Vec<_>>();
        Ok(results)
    }
});

//
// HTTP abstractions
//

struct ResponseTime;

impl typemap::Key for ResponseTime {
    type Value = u64;
}

impl BeforeMiddleware for ResponseTime {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<ResponseTime>(precise_time_ns());
        Ok(())
    }
}

impl AfterMiddleware for ResponseTime {
    fn after(&self, req: &mut Request, res: Response) -> IronResult<Response> {
        let delta = precise_time_ns() - *req.extensions.get::<ResponseTime>().unwrap();
        println!("Request took: {} ms", (delta as f64) / 1000000.0);
        Ok(res)
    }
}

//
// Main
//

fn main() {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    let pool = Pool::builder()
        .build(manager)
        .expect("Failed to create pool.");

    let graphql_endpoint = GraphQLHandler::new(
        move |_: &mut Request| -> Context { Context { pool: pool.clone() } },
        Query {},
        Mutation {},
    );
    let graphiql_endpoint = GraphiQLHandler::new("/graphql");

    let mut mount = Mount::new();
    mount.mount("/", graphiql_endpoint);
    mount.mount("/graphql", graphql_endpoint);

    let mut chain = Chain::new(mount);
    chain.link_before(ResponseTime);
    chain.link_after(ResponseTime);

    let port = env::var("PORT").unwrap_or("8080".to_owned());
    let host = format!("0.0.0.0:{}", port);
    println!("GraphQL server started on {}", host);
    Iron::new(chain).http(host.as_str()).unwrap();
}
