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
use bytes::Bytes;
use diesel::pg::PgConnection;
use futures::future;
use futures::future::Future;
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
                    r.method(Method::GET).f(handler_graphiql_get);
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

/// A struct to serialize a set of GraphQL errors back to a client (errors are always sent back as
/// an array).
#[derive(Debug, Clone, Deserialize, Serialize)]
struct GraphQLErrors {
    errors: Vec<GraphQLError>,
}

/// A struct to serialize a GraphQL error back to the client. Should be nested within
/// `GraphQLErrors`.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct GraphQLError {
    message: String,
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
                Params::build_from_post(log, bytes.as_ref()).map_err(|e| ErrorKind::BadRequest(e.to_string()).into())
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
            return Box::new(future::result(handle_error(
                StatusCode::BAD_REQUEST,
                e.description().to_owned(),
            )));
        }
    };

    execute(
        log,
        Box::new(future::ok(params)),
        req.state().sync_addr.clone(),
    )
}

#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn handler_graphiql_get(_req: HttpRequest<server::StateImpl>) -> HttpResponse {
    HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(graphiql::graphiql_source("/graphql"))
        .unwrap()
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
        // TODO: Why is this res?! -- I guess we're returning Result, see if we can pass along okay
        // instead
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
        .then(|res| {
            match res {
                Err(e @ Error(ErrorKind::BadRequest(_), _)) => {
                    // `format!` activates the `Display` traits and shows our `display` definition
                    handle_error(StatusCode::BAD_REQUEST, format!("{}", e))
                }
                r => r,
            }
        })
        .responder()
}

pub fn handle_error(code: StatusCode, message: String) -> Result<HttpResponse> {
    let body = serde_json::to_string_pretty(&GraphQLErrors {
        errors: vec![GraphQLError { message }],
    })?;
    Ok(HttpResponse::build(code)
        .content_type("application/json; charset=utf-8")
        .body(body)?)
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
    use actix_web;
    use actix_web::Method;
    use diesel::pg::PgConnection;
    use r2d2::Pool;
    use r2d2_diesel::ConnectionManager;

    #[test]
    fn test_handler_graphql_get() {
        let bootstrap = TestBootstrap::new();
        let mut server = bootstrap.server_builder.start(|app| {
            app.middleware(middleware::log_initializer::Middleware)
                .handler(handler_graphql_get)
        });

        let req = server
            .client(
                Method::GET,
                format!("/?query={}", test_helpers::url_encode(b"{podcast{id}}")).as_str(),
            )
            .finish()
            .unwrap();

        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::OK, resp.status());
        let value = test_helpers::read_body_json(resp);
        assert_eq!(json!({"data": {"podcast": []}}), value);
    }

    #[test]
    fn test_handler_graphql_get_no_query() {
        let bootstrap = TestBootstrap::new();
        let mut server = bootstrap.server_builder.start(|app| {
            app.middleware(middleware::log_initializer::Middleware)
                .handler(handler_graphql_get)
        });

        let req = server.get().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::BAD_REQUEST, resp.status());
        let value = test_helpers::read_body_json(resp);
        assert_eq!(json!({"errors": [{"message": "No query provided"}]}), value);
    }

    #[test]
    fn test_handler_graphql_post() {
        let bootstrap = TestBootstrap::new();
        let mut server = bootstrap.server_builder.start(|app| {
            app.middleware(middleware::log_initializer::Middleware)
                .handler(handler_graphql_post)
        });

        let graphql_req = GraphQLRequest::new("{podcast{id}}".to_owned(), None, None);
        let body = serde_json::to_string(&graphql_req).unwrap();
        let req = server.post().body(body).unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::OK, resp.status());
        let value = test_helpers::read_body_json(resp);
        assert_eq!(json!({"data": {"podcast": []}}), value);
    }

    #[test]
    fn test_handler_graphql_post_no_query() {
        let bootstrap = TestBootstrap::new();
        let mut server = bootstrap.server_builder.start(|app| {
            app.middleware(middleware::log_initializer::Middleware)
                .handler(handler_graphql_post)
        });

        let req = server.post().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::BAD_REQUEST, resp.status());
        let value = test_helpers::read_body_json(resp);
        assert_eq!(
            json!({"errors": [{"message": "Bad request: Error deserializing request body"}]}),
            value
        );
    }

    #[test]
    fn test_handler_graphiql_get() {
        let bootstrap = TestBootstrap::new();
        let mut server = bootstrap
            .server_builder
            .start(|app| app.handler(handler_graphiql_get));

        let req = server.get().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();
        assert_eq!(StatusCode::OK, resp.status());
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common:        test_helpers::CommonTestBootstrap,
        _pool:          Pool<ConnectionManager<PgConnection>>,
        server_builder: actix_web::test::TestServerBuilder<server::StateImpl>,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            let pool = test_helpers::pool();
            let pool_clone = pool.clone();

            let server_builder = actix_web::test::TestServer::build_with_state(move || {
                // This is fucking disgusting. Ladies and gentlemen, I give you Rust.
                let pool_clone = pool_clone.clone();

                server::StateImpl {
                    assets_version: "".to_owned(),
                    log:            test_helpers::log(),
                    sync_addr:      actix::SyncArbiter::start(1, move || server::SyncExecutor {
                        pool: pool_clone.clone(),
                    }),
                }
            });

            TestBootstrap {
                _common:        test_helpers::CommonTestBootstrap::new(),
                _pool:          pool,
                server_builder: server_builder,
            }
        }
    }
}
