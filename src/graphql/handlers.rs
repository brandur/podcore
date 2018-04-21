use errors::*;
use graphql;
use middleware;
use model;
use server;
use server::Params as P;
use server::State;
use time_helpers;

use actix;
use actix_web::http::StatusCode;
use actix_web::AsyncResponder;
use actix_web::{HttpRequest, HttpResponse};
use bytes::Bytes;
use futures::future;
use futures::future::Future;
use juniper::http::GraphQLRequest;
use juniper::{InputValue, RootNode};
use serde_json;
use slog::Logger;

//
// Private structs
//

struct ExecutionResponse {
    json: String,
    ok:   bool,
}

struct Params {
    account:     model::Account,
    graphql_req: GraphQLRequest,
}

impl server::Params for Params {
    // Only exists as a symbolic target to let us implement `Params` because this
    // parameter type can be implemented in multiple ways. See `build_from_get`
    // and `build_from_post` instead.
    fn build<S: server::State>(
        _log: &Logger,
        req: &mut HttpRequest<S>,
        data: Option<&[u8]>,
    ) -> Result<Self> {
        let account = match server::account(req) {
            Some(account) => account,
            None => bail!(error::unauthorized()),
        };

        match data {
            // Build from `POST` request
            Some(data) => match serde_json::from_slice::<GraphQLRequest>(data) {
                Ok(graphql_req) => Ok(Params {
                    account,
                    graphql_req,
                }),
                Err(e) => bail!(error::bad_request(format!(
                    "Error deserializing request body: {}",
                    e
                ))),
            },

            // Build from `GET` request
            None => {
                let input_query = match req.query().get("query") {
                    Some(q) => q.to_owned(),
                    None => bail!(error::bad_request("No query provided")),
                };

                let operation_name = req.query().get("operationName").map(|n| n.to_owned());

                let variables: Option<InputValue> = match req.query().get("variables") {
                    Some(v) => match serde_json::from_str::<InputValue>(v) {
                        Ok(v) => Some(v),
                        Err(e) => bail!(error::bad_request(format!(
                            "Malformed variables JSON: {}",
                            e
                        ))),
                    },
                    None => None,
                };

                Ok(Self {
                    account,
                    graphql_req: GraphQLRequest::new(input_query, operation_name, variables),
                })
            }
        }
    }
}

//
// Web handlers
//

pub fn graphql_post(
    mut req: HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    use actix_web::HttpMessage;

    let log = middleware::log_initializer::log(&mut req);
    let log_clone = log.clone();
    let mut req_clone = req.clone();
    let sync_addr = req.state().get_sync_addr().clone();

    let fut = req.body()
        // `map_err` is used here instead of `chain_err` because `PayloadError` doesn't implement
        // the `Error` trait and I was unable to put it in the error chain.
        .map_err(|_e| Error::from("Error reading request body"))
        .and_then(move |bytes: Bytes| {
            time_helpers::log_timed(&log_clone.new(o!("step" => "build_params")), |log| {
                Params::build(log, &mut req_clone, Some(bytes.as_ref()))
            })
        })
        .from_err();

    execute(log, Box::new(fut), sync_addr)
}

pub fn graphql_get(
    mut req: HttpRequest<server::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let log = middleware::log_initializer::log(&mut req);

    let params_res = time_helpers::log_timed(&log.new(o!("step" => "build_params")), |log| {
        Params::build(log, &mut req, None)
    });

    execute(
        log,
        Box::new(future::result(params_res)),
        req.state().get_sync_addr().clone(),
    )
}

#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
pub fn graphiql_get(_req: HttpRequest<server::StateImpl>) -> HttpResponse {
    // This HTML file was carbon copied from `juniper`'s source, but modified to
    // support sending credentials so that we can authenticate via cookie.
    HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(include_str!("../static/html/graphiql.html"))
}

//
// Message handlers
//

impl ::actix::prelude::Handler<server::Message<Params>> for server::SyncExecutor {
    type Result = Result<ExecutionResponse>;

    fn handle(&mut self, message: server::Message<Params>, _: &mut Self::Context) -> Self::Result {
        let conn = self.pool.get()?;
        let root_node = RootNode::new(
            graphql::operations::Query::default(),
            graphql::operations::Mutation::default(),
        );
        time_helpers::log_timed(
            &message.log.new(o!("step" => "handle_message")),
            move |log| {
                let context = graphql::operations::Context {
                    account: message.params.account,
                    conn,
                    log: log.clone(),
                };
                info!(log, "Executing GraphQL query");
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
    // We need `log` clones because we have multiple `move` closures below (and only
    // one can take the original log).
    let log2 = log.clone();
    let log3 = log.clone();

    fut.and_then(move |params| {
        let message = server::Message::new(&log, params);
        sync_addr
            .send(message)
            .map_err(|_e| Error::from("Future canceled"))
    }).flatten()
        .and_then(move |response| {
            time_helpers::log_timed(&log2.new(o!("step" => "render_response")), |_log| {
                let code = if response.ok {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                };
                Ok(HttpResponse::build(code)
                    .content_type("application/json; charset=utf-8")
                    .body(response.json))
            })
        })
        .then(move |res| match res {
            Err(e) => Ok(server::render_error(
                &log3,
                e,
                graphql::errors::error_internal,
                graphql::errors::error_user,
            )),
            r => r,
        })
        .responder()
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use graphql::handlers::*;
    use test_helpers;
    use test_helpers::IntegrationTestBootstrap;

    use actix_web::http::Method;

    #[test]
    fn test_graphql_handlers_graphql_get_ok() {
        let bootstrap = IntegrationTestBootstrap::new();
        let middleware = bootstrap.authenticated_middleware();
        let mut server = bootstrap.server_builder.start(move |app| {
            app.middleware(middleware::log_initializer::Middleware)
                .middleware(middleware.clone())
                .handler(graphql_get)
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
    fn test_graphql_handlers_graphql_get_no_credentials() {
        let bootstrap = IntegrationTestBootstrap::new();
        let mut server = bootstrap.server_builder.start(|app| {
            app.middleware(middleware::log_initializer::Middleware)
                .handler(graphql_get)
        });

        let req = server.get().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::UNAUTHORIZED, resp.status());
        let value = test_helpers::read_body_json(resp);
        assert_eq!(
            json!({"errors": [{"message": format!("{}", error::unauthorized())}]}),
            value
        );
    }

    #[test]
    fn test_graphql_handlers_graphql_get_no_query() {
        let bootstrap = IntegrationTestBootstrap::new();
        let middleware = bootstrap.authenticated_middleware();
        let mut server = bootstrap.server_builder.start(move |app| {
            app.middleware(middleware::log_initializer::Middleware)
                .middleware(middleware.clone())
                .handler(graphql_get)
        });

        let req = server.get().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::BAD_REQUEST, resp.status());
        let e = error::bad_request("No query provided");
        let value = test_helpers::read_body_json(resp);
        assert_eq!(json!({"errors": [{"message": format!("{}", e)}]}), value);
    }

    #[test]
    fn test_graphql_handlers_graphql_post_ok() {
        let bootstrap = IntegrationTestBootstrap::new();
        let middleware = bootstrap.authenticated_middleware();
        let mut server = bootstrap.server_builder.start(move |app| {
            app.middleware(middleware::log_initializer::Middleware)
                .middleware(middleware.clone())
                .handler(graphql_post)
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
    fn test_graphql_handlers_graphql_post_no_credentials() {
        let bootstrap = IntegrationTestBootstrap::new();
        let mut server = bootstrap.server_builder.start(|app| {
            app.middleware(middleware::log_initializer::Middleware)
                .handler(graphql_post)
        });

        let req = server.post().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::UNAUTHORIZED, resp.status());
        let value = test_helpers::read_body_json(resp);
        assert_eq!(
            json!({"errors": [{"message": format!("{}", error::unauthorized())}]}),
            value
        );
    }

    #[test]
    fn test_graphql_handlers_graphql_post_no_query() {
        let bootstrap = IntegrationTestBootstrap::new();
        let middleware = bootstrap.authenticated_middleware();
        let mut server = bootstrap.server_builder.start(move |app| {
            app.middleware(middleware::log_initializer::Middleware)
                .middleware(middleware.clone())
                .handler(graphql_post)
        });

        let req = server.post().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();

        assert_eq!(StatusCode::BAD_REQUEST, resp.status());
        let error = error::bad_request(concat!(
            "Error deserializing request body: ",
            "EOF while parsing a value at line 1 column 0"
        ));
        let value = test_helpers::read_body_json(resp);
        assert_eq!(
            json!({"errors": [{"message": format!("{}", error)}]}),
            value
        );
    }

    #[test]
    fn test_graphql_handlers_graphiql_get_ok() {
        let bootstrap = IntegrationTestBootstrap::new();
        let mut server = bootstrap
            .server_builder
            .start(|app| app.handler(graphiql_get));

        let req = server.get().finish().unwrap();
        let resp = server.execute(req.send()).unwrap();
        assert_eq!(StatusCode::OK, resp.status());
    }
}
