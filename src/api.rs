use errors::*;
use graphql;
use middleware;
use server;
use server::Params as P;
use time_helpers;

use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use actix_web::AsyncResponder;
use actix_web::Method;
use actix_web::ResponseError;
use bytes::Bytes;
use diesel::pg::PgConnection;
use futures::future;
use futures::future::Future;
use juniper::{InputValue, RootNode};
use juniper::http::GraphQLRequest;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use serde_json;
use slog::Logger;

pub struct Server {
    pub log:                Logger,
    pub num_sync_executors: u32,
    pub pool:               Pool<ConnectionManager<PgConnection>>,
    pub port:               String,
}

impl Server {
    pub fn run(&self) -> Result<()> {
        let log = self.log.clone();
        let pool = self.pool.clone();

        // Must appear up here because we're going to move `log` into server closure.
        let host = format!("0.0.0.0:{}", self.port.as_str());
        info!(log, "API server starting"; "host" => host.as_str());

        // Although not referenced in the server definition, a `System` must be defined
        // or the server will crash on `start()`.
        let system = actix::System::new("podcore-api");

        let sync_addr = actix::SyncArbiter::start(self.num_sync_executors as usize, move || {
            server::SyncExecutor { pool: pool.clone() }
        });

        let server = actix_web::HttpServer::new(move || {
            actix_web::Application::with_state(server::StateImpl {
                assets_version: "".to_owned(),
                log:            log.clone(),
                sync_addr:      sync_addr.clone(),
            }).middleware(middleware::log_initializer::Middleware)
                .middleware(middleware::request_id::Middleware)
                .middleware(middleware::request_response_logger::Middleware)
                .resource("/", |r| {
                    r.method(Method::GET).f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/graphql", |r| r.method(Method::GET).a(get_handler))
                .resource("/health", |r| {
                    r.method(Method::GET).f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .default_resource(|r| r.h(actix_web::NormalizePath::default()))
        });

        let _addr = server.bind(host)?.start();
        let _ = system.run();

        Ok(())
    }
}

struct Params {
    graphql_req: GraphQLRequest,
}

impl Params {
    fn build_from_post(_log: &Logger, data: &[u8]) -> Result<Self> {
        match serde_json::from_slice::<GraphQLRequest>(data) {
            Ok(graphql_req) => Ok(Params { graphql_req }),
            Err(e) => Err(Error::from("Error deserializing request body")),
        }
    }
}

impl server::Params for Params {
    // TODO: convert this to build_from_get
    fn build(_log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<Self> {
        let input_query = match req.query().get("query") {
            Some(q) => q.to_owned(),
            None => {
                return Err(Error::from("No query provided"));
            }
        };

        let operation_name = req.query().get("operationName").map(|n| n.to_owned());

        let variables: Option<InputValue> = match req.query().get("variables") {
            Some(v) => match serde_json::from_str::<InputValue>(v) {
                Ok(v) => Some(v),
                Err(e) => {
                    return Err(Error::from(format!(
                        "Malformed variables JSON. Error: {}",
                        e
                    )));
                }
            },
            None => None,
        };

        Ok(Self {
            graphql_req: GraphQLRequest::new(input_query, operation_name, variables),
        })
    }
}

/*
pub fn post_handler(
    mut req: HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let log = middleware::log_initializer::log(&mut req);
    req.body()
        .from_err()
        .and_then(|bytes: Bytes| {
            // TODO: Timing
            Params::build_from_post(&log, bytes.as_ref())
        })
        .from_err()
        .and_then(|params: Params| execute(&log, &req, params))
}
*/

pub fn get_handler(
    mut req: HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let log = middleware::log_initializer::log(&mut req);

    let params_res = time_helpers::log_timed(&log.new(o!("step" => "build_params")), |log| {
        Params::build(log, &req)
    });
    let params = match params_res {
        Ok(params) => params,
        Err(e) => {
            return Box::new(future::ok(
                actix_web::error::ErrorBadRequest(e.description().to_owned()).error_response(),
            ));
        }
    };

    execute(&log, future::ok(params), &req)
}

fn execute<F>(
    log: &Logger,
    fut: F,
    req: &HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>>
where
    F: Future<Item = Params, Error = Error> + 'static,
{
    fut.and_then(|_params| {
        Ok(HttpResponse::build(StatusCode::OK)
            .content_type("application/json; charset=utf-8")
            .body("hello".to_owned())
            .unwrap())
    }).responder()
    /*
    fut.and_then(|params| {
        let message = server::Message::new(&log, params);
        req.state().sync_addr.call_fut(message)
    }).chain_err(|| "Error from SyncExecutor")
        .from_err()
        .and_then(move |res| {
            let response = res?;
            time_helpers::log_timed(&log.new(o!("step" => "render_response")), |_log| {
                let code = if response.ok {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                };
                Ok(HttpResponse::build(code)
                    .content_type("application/json; charset=utf-8")
                    .body(response.json)
                    .unwrap())
            })
        })
        .responder()
    */
}

type MessageResult = ::actix::prelude::MessageResult<server::Message<Params>>;

impl ::actix::prelude::Handler<server::Message<Params>> for server::SyncExecutor {
    type Result = MessageResult;

    fn handle(&mut self, message: server::Message<Params>, _: &mut Self::Context) -> Self::Result {
        let conn = self.pool.get()?;
        let root_node = RootNode::new(graphql::Query::default(), graphql::Mutation::default());
        time_helpers::log_timed(
            &message.log.new(o!("step" => "handle_message")),
            move |log| {
                let context = graphql::Context {
                    conn,
                    log: log.clone(),
                };
                let graphql_response = message.params.graphql_req.execute(&root_node, &context);
                Ok(ExecutionResponse {
                    json: serde_json::to_string_pretty(&graphql_response)?,
                    ok:   graphql_response.is_ok(),
                })
            },
        )
    }
}

struct ExecutionResponse {
    json: String,
    ok:   bool,
}

// TODO: `ResponseType` will change to `Message`
impl ::actix::prelude::ResponseType for server::Message<Params> {
    type Item = ExecutionResponse;
    type Error = Error;
}
