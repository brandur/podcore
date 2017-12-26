#[macro_use] extern crate diesel;
extern crate iron;
#[macro_use] extern crate juniper;
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
use iron::{BeforeMiddleware, AfterMiddleware, typemap};
use juniper::{EmptyMutation, FieldResult};
use juniper_iron::{GraphQLHandler, GraphiQLHandler};
use mount::Mount;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use std::env;
use time::precise_time_ns;

//
// Model
//

type DieselConnection = r2d2::PooledConnection<ConnectionManager<PgConnection>>;

#[derive(Queryable)]
pub struct Podcast {
    pub id: i64,
    pub title: String,
    pub url: String,
}

struct Context {
     pool: Pool<ConnectionManager<PgConnection>>,
}

impl Context {
}

impl juniper::Context for Context {}

//
// GraphQL
//

#[derive(GraphQLObject)]
struct PodcastObject {
    pub title: String,
    pub url: String,
}

struct Query;

graphql_object!(Query: Context |&self| {
    description: "The root query object of the schema"

    field apiVersion() -> &str {
        "1.0"
    }

    field podcasts(&executor) -> FieldResult<Vec<PodcastObject>> {
        let context = executor.context();
        let conn: DieselConnection = context.pool.get()?;

        let results = schema::podcasts::table
            .order(schema::podcasts::title.asc())
            .limit(5)
            .load::<Podcast>(&*conn)?
            .iter()
            .map(|p| PodcastObject {
                    title: p.title.to_owned(),
                    url: p.url.to_owned(),
            })
            .collect::<Vec<_>>();

        Ok(results)
    }
});

/*
#[derive(GraphQLObject)]
#[graphql(description="Information about a person")]
struct Person {
    #[graphql(description="The person's full name, including both first and last names")]
    name: String,

    #[graphql(description="The person's age in years, rounded down")]
    age: i32,
}

#[derive(GraphQLObject)]
struct House {
    address: Option<String>, // Converted into String (nullable)
    inhabitants: Vec<Person>, // Converted into [Person!]!
}
*/

//
// HTTP abstractions
//

struct ResponseTime;

impl typemap::Key for ResponseTime { type Value = u64; }

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
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    let pool = Pool::builder().build(manager).expect("Failed to create pool.");

    let graphql_endpoint = GraphQLHandler::new(
        move |_: &mut Request| -> Context {
            Context {
                pool: pool.clone(),
            }
        },
        Query {},
        EmptyMutation::<Context>::new(),
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
