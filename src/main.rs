extern crate iron;
#[macro_use] extern crate juniper;
extern crate juniper_iron;
extern crate mount;
extern crate serde;
extern crate time;

use std::env;

use iron::prelude::*;
use iron::{BeforeMiddleware, AfterMiddleware, typemap};
use juniper::{Context, EmptyMutation};
use juniper_iron::{GraphQLHandler, GraphiQLHandler};
use mount::Mount;
use time::precise_time_ns;

struct Database;

impl Database {
    fn new() -> Database {
        Database {}
    }
}

impl Context for Database {}

graphql_object!(Database: Database as "Query" |&self| {
    description: "The root query object of the schema"
});

struct ResponseTime;

impl typemap::Key for ResponseTime { type Value = u64; }

fn context_factory(_: &mut Request) -> Database {
    Database::new()
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

fn main() {
    let graphql_endpoint = GraphQLHandler::new(
        context_factory,
        Database::new(),
        EmptyMutation::<Database>::new(),
    );
    let graphiql_endpoint = GraphiQLHandler::new("/graphql");

    let mut mount = Mount::new();
    mount.mount("/", graphiql_endpoint);
    mount.mount("/graphql", graphql_endpoint);

    let mut chain = Chain::new(mount);
    chain.link_before(ResponseTime);
    chain.link_after(ResponseTime);

    let host = env::var("LISTEN").unwrap_or("0.0.0.0:8080".to_owned());
    println!("GraphQL server started on {}", host);
    Iron::new(chain).http(host.as_str()).unwrap();
}
