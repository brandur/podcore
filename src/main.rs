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
pub struct Podcast {
    pub id:    i64,
    pub title: String,
    pub url:   String,
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
struct PodcastObject {
    #[graphql(description = "The podcast's title.")]
    pub title: String,

    #[graphql(description = "The podcast's RSS feed URL.")]
    pub url: String,
}

impl<'a> From<&'a Podcast> for PodcastObject {
    fn from(p: &Podcast) -> Self {
        PodcastObject {
            title: p.title.to_owned(),
            url:   p.url.to_owned(),
        }
    }
}

struct Query;

graphql_object!(Query: Context |&self| {
    description: "The root query object of the schema."

    field apiVersion() -> &str {
        "1.0"
    }

    field podcasts(&executor) -> FieldResult<Vec<PodcastObject>> as "A collection podcasts." {
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
