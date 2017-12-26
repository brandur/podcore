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

struct Root;

impl Root {
    fn new() -> Root {
        Root {}
    }
}

impl Context for Root {}

graphql_object!(Root: Root as "Root" |&self| {
    description: "The root query object of the schema"

    field foo() -> String {
        "Bar".to_owned()
    }
});

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

struct ResponseTime;

impl typemap::Key for ResponseTime { type Value = u64; }

fn context_factory(_: &mut Request) -> Root {
    Root::new()
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
        Root::new(),
        EmptyMutation::<Root>::new(),
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
