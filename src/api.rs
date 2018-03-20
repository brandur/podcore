use errors::*;
use graphql;
use middleware;
use server;
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
use futures::future::{Future, FutureResult};
use juniper::{InputValue, RootNode};
use juniper::graphiql;
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
                .resource("/graphiql", |r| {
                    r.method(Method::GET).a(handler_graphiql_get);
                })
                .resource("/graphql", |r| {
                    r.method(Method::GET).a(handler_graphql_get);
                    r.method(Method::POST).a(handler_graphql_post);
                })
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

//
// Private structs
//

struct ExecutionResponse {
    json: String,
    ok:   bool,
}

struct Params {
    graphql_req: GraphQLRequest,
}

impl Params {
    /// Builds `Params` from a `GET` request.
    fn build_from_get(log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<Self> {
        let input_query = match req.query().get("query") {
            Some(q) => q.to_owned(),
            None => {
                info!(log, "No query provided");
                return Err(Error::from("No query provided"));
            }
        };

        let operation_name = req.query().get("operationName").map(|n| n.to_owned());

        let variables: Option<InputValue> = match req.query().get("variables") {
            Some(v) => match serde_json::from_str::<InputValue>(v) {
                Ok(v) => Some(v),
                Err(e) => {
                    info!(log, "Variables JSON malformed");
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

    /// Builds `Params` from a `POST` request.
    fn build_from_post(_log: &Logger, data: &[u8]) -> Result<Self> {
        match serde_json::from_slice::<GraphQLRequest>(data) {
            Ok(graphql_req) => Ok(Params { graphql_req }),
            Err(_e) => Err(Error::from("Error deserializing request body")),
        }
    }
}

impl server::Params for Params {
    // Only exists as a symbolic target to let us implement `Params` because this
    // parameter type can be implemented in multiple ways. See `build_from_get`
    // and `build_from_post` instead.
    fn build(_log: &Logger, _req: &HttpRequest<server::StateImpl>) -> Result<Self> {
        unimplemented!()
    }
}

//
// Web handlers
//

fn handler_graphql_post(
    mut req: HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    use actix_web::HttpMessage;

    let log = middleware::log_initializer::log(&mut req);
    let log_clone = log.clone();

    let sync_addr = req.state().sync_addr.clone();
    let fut = req.body()
        // `map_err` is used here instead of `chain_err` because `PayloadError` doesn't implement
        // the `Error` trait and I was unable to put it in the error chain.
        .map_err(|_e| Error::from("Error reading request body"))
        .and_then(move |bytes: Bytes| {
            time_helpers::log_timed(&log_clone.new(o!("step" => "build_params")), |log| {
                Params::build_from_post(log, bytes.as_ref())
            })
        })
        .from_err();

    execute(log, Box::new(fut), sync_addr)
}

fn handler_graphql_get(
    mut req: HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let log = middleware::log_initializer::log(&mut req);

    let params_res = time_helpers::log_timed(&log.new(o!("step" => "build_params")), |log| {
        Params::build_from_get(log, &req)
    });
    let params = match params_res {
        Ok(params) => params,
        Err(e) => {
            return Box::new(future::ok(
                actix_web::error::ErrorBadRequest(e.description().to_owned()).error_response(),
            ));
        }
    };

    execute(
        log,
        Box::new(future::ok(params)),
        req.state().sync_addr.clone(),
    )
}

#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn handler_graphiql_get(_req: HttpRequest<server::StateImpl>) -> FutureResult<HttpResponse, Error> {
    future::ok(
        HttpResponse::build(StatusCode::OK)
            .content_type("text/html; charset=utf-8")
            .body(graphiql::graphiql_source("/graphql"))
            .unwrap(),
    )
}

//
// Message handlers
//

impl ::actix::prelude::Handler<server::Message<Params>> for server::SyncExecutor {
    type Result = Result<ExecutionResponse>;

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

impl ::actix::prelude::Message for server::Message<Params> {
    type Result = Result<ExecutionResponse>;
}

//
// Private functions
//

fn execute<F>(
    log: Logger,
    fut: Box<F>,
    sync_addr: actix::prelude::Addr<actix::prelude::Syn, server::SyncExecutor>,
) -> Box<Future<Item = HttpResponse, Error = Error>>
where
    F: Future<Item = Params, Error = Error> + 'static,
{
    // We need one `log` clone because we have two `move` closures below (and only
    // one can take the log).
    let log_clone = log.clone();

    fut.and_then(move |params| {
        let message = server::Message::new(&log_clone, params);
        sync_addr
            .send(message)
            .map_err(|_e| Error::from("Future canceled"))
    }).from_err()
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
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use api::*;
    use server;
    use test_helpers;

    use actix;
    use actix_web::{HttpMessage, Method};
    use actix_web::test::{TestRequest, TestServer};
    use percent_encoding::{percent_encode, DEFAULT_ENCODE_SET};
    use serde_json;

    #[test]
    fn test_handler_graphql_get() {
        /*
        let bootstrap = TestBootstrap::new();
        */

        let mut srv = TestServer::with_state(
            || {
                let pool = test_helpers::pool();
                let pool_clone = pool.clone();

                server::StateImpl {
                    assets_version: "".to_owned(),
                    log:            test_helpers::log(),
                    sync_addr:      actix::SyncArbiter::start(1, move || server::SyncExecutor {
                        pool: pool_clone.clone(),
                    }),
                }
            },
            |app| {
                app.middleware(middleware::log_initializer::Middleware)
                    .handler(handler_graphql_get)
            },
        );

        let req = srv.client(
            Method::GET,
            format!(
                "/?query={}",
                percent_encode(b"{podcast{id}}", DEFAULT_ENCODE_SET)
            ).as_str(),
        ).finish()
            .unwrap();
        let resp = srv.execute(req.send()).unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let bytes: Bytes = resp.body().wait().unwrap();
        let value: serde_json::Value = serde_json::from_slice(bytes.as_ref()).unwrap();

        assert_eq!(json!({"data": {"podcast": []}}), value);
    }

    #[test]
    fn test_handler_graphiql_get() {
        let bootstrap = TestBootstrap::new();
        let resp = TestRequest::with_state(bootstrap.state)
            .run_async(|r| handler_graphiql_get(r).map_err(|e| e.into()))
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        _system: actix::SystemRunner,
        state:   server::StateImpl,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let pool_clone = pool.clone();
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                _system: actix::System::new("podcore-api-test"),
                state:   server::StateImpl {
                    assets_version: "".to_owned(),
                    log:            test_helpers::log(),
                    sync_addr:      actix::SyncArbiter::start(1, move || server::SyncExecutor {
                        pool: pool_clone.clone(),
                    }),
                },
            }
        }
    }
}
