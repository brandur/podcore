extern crate chrono;
extern crate diesel;
extern crate futures;
extern crate iron;
extern crate juniper_iron;
extern crate mount;
extern crate pod_core;
extern crate r2d2;
extern crate r2d2_diesel;
extern crate time;

// We'll need this soon enough
//use errors::*;

use pod_core::graphql;

use diesel::pg::PgConnection;
use iron::prelude::*;
use iron::{typemap, AfterMiddleware, BeforeMiddleware};
use juniper_iron::{GraphQLHandler, GraphiQLHandler};
use mount::Mount;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use std::env;
use time::precise_time_ns;

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
        move |_: &mut Request| -> graphql::Context { graphql::Context::new(pool.clone()) },
        graphql::Query::new(),
        graphql::Mutation::new(),
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

#[test]
fn test_nothing() {}
