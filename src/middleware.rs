pub mod log_initializer {
    use server;

    use actix_web;
    use actix_web::middleware::{Response, Started};
    use actix_web::{HttpRequest, HttpResponse};
    use slog::Logger;

    pub struct Middleware;

    pub struct Extension(pub Logger);

    impl<S: server::State> actix_web::middleware::Middleware<S> for Middleware {
        fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
            let log = req.state().log().clone();
            req.extensions().insert(Extension(log));
            Ok(Started::Done)
        }

        fn response(
            &self,
            _req: &mut HttpRequest<S>,
            resp: HttpResponse,
        ) -> actix_web::Result<Response> {
            Ok(Response::Done(resp))
        }
    }

    /// Shorthand for getting a usable `Logger` out of a request. It's also
    /// possible to access the request's extensions directly.
    pub fn log<S: server::State>(req: &mut HttpRequest<S>) -> Logger {
        req.extensions().get::<Extension>().unwrap().0.clone()
    }
}

pub mod request_id {
    use middleware::log_initializer;
    use server;

    use actix_web;
    use actix_web::middleware::{Response, Started};
    use actix_web::{HttpRequest, HttpResponse};

    use uuid::Uuid;

    pub struct Middleware;

    impl<S: server::State> actix_web::middleware::Middleware<S> for Middleware {
        fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
            let log = req.extensions()
                .remove::<log_initializer::Extension>()
                .unwrap()
                .0;

            let request_id = Uuid::new_v4().simple().to_string();
            debug!(&log, "Generated request ID"; "request_id" => request_id.as_str());

            req.extensions().insert(log_initializer::Extension(log.new(
                o!("request_id" => request_id),
            )));

            Ok(Started::Done)
        }

        fn response(
            &self,
            _req: &mut HttpRequest<S>,
            resp: HttpResponse,
        ) -> actix_web::Result<Response> {
            Ok(Response::Done(resp))
        }
    }
}

pub mod request_response_logger {
    use middleware::log_initializer;
    use server;
    use time_helpers;

    use actix_web;
    use actix_web::middleware::{Response, Started};
    use actix_web::{HttpRequest, HttpResponse};

    use time;

    pub struct Middleware;

    struct Extension {
        start_time: u64,
    }

    impl<S: server::State> actix_web::middleware::Middleware<S> for Middleware {
        fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
            req.extensions().insert(Extension {
                start_time: time::precise_time_ns(),
            });
            Ok(Started::Done)
        }

        fn response(
            &self,
            req: &mut HttpRequest<S>,
            resp: HttpResponse,
        ) -> actix_web::Result<Response> {
            let log = log_initializer::log(req);
            let elapsed =
                time::precise_time_ns() - req.extensions().get::<Extension>().unwrap().start_time;
            info!(log, "Request finished";
                    "elapsed" => time_helpers::unit_str(elapsed),
                    "method"  => req.method().as_str(),
                    "path"    => req.path(),
                    "status"  => resp.status().as_u16(),
                );
            Ok(Response::Done(resp))
        }
    }
}

/// Holds web-specific (as opposed to for the API) middleware.
pub mod web {
    pub mod authenticator {
        use errors::*;
        use mediators::account_authenticator;
        use mediators::account_creator;
        use middleware::log_initializer;
        use model;
        use server;
        use server::Params as P;
        use time_helpers;

        use actix_web;
        use actix_web::http::StatusCode;
        use actix_web::middleware::{Response, Started};
        use actix_web::{HttpRequest, HttpResponse};
        use diesel::pg::PgConnection;
        use slog::Logger;

        pub struct Middleware;

        struct Extension {
            account: Option<model::Account>,
        }

        impl<S: server::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                use futures::Future;

                let log = log_initializer::log(req);
                debug!(log, "Authenticating");

                let params_res = time_helpers::log_timed(
                    &log.new(o!("step" => "build_params")),
                    |log| Params::build(log, req),
                );
                let params = match params_res {
                    Ok(params) => params,
                    Err(e) => {
                        // TODO: More cohesive error handling strategy
                        error!(log, "Middleware error: {}", e);
                        return Ok(Started::Response(
                            HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR)
                                .content_type("text/html; charset=utf-8")
                                .body("Error getting session information"),
                        ));
                    }
                };

                let message = server::Message::new(&log, params);

                let fut = req.state()
                    .sync_addr()
                    .send(message)
                    .map_err(|_e| Error::from("Error from SyncExecutor"))
                    .flatten()
                    .and_then(move |view_model: ViewModel| {
                        req.extensions().insert(Extension {
                            account: view_model.account,
                        });
                        (None as Option<HttpResponse>)
                    });

                Ok(Started::Future(Box::new(fut)))
            }

            // No-op
            //
            // TODO: See if we can still compile without this?
            fn response(
                &self,
                _req: &mut HttpRequest<S>,
                resp: HttpResponse,
            ) -> actix_web::Result<Response> {
                Ok(Response::Done(resp))
            }
        }

        //
        // Params
        //

        struct Params {
            last_ip: String,
            secret:  Option<String>,
        }

        impl server::Params for Params {
            fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
                use actix_web::middleware::RequestSession;
                Ok(Params {
                    last_ip: req.connection_info().host().to_owned(),
                    secret:  req.session()
                        .get::<String>(COOKIE_KEY_SECRET)
                        .map_err(|_| Error::from("Error reading from session"))?,
                })
            }
        }

        //
        // ViewModel
        //

        struct ViewModel {
            account: Option<model::Account>,
        }

        //
        // Sync handler
        //

        // TODO: Maybe combine this with the `handler!()` macro available in
        // `endpoints`? It's the same right now, so try not to change it.
        impl ::actix::prelude::Handler<server::Message<Params>> for server::SyncExecutor {
            type Result = Result<ViewModel>;

            fn handle(
                &mut self,
                message: server::Message<Params>,
                _: &mut Self::Context,
            ) -> Self::Result {
                let conn = self.pool.get()?;
                let log = message.log.clone();
                time_helpers::log_timed(&log.new(o!("step" => "handle_message")), |log| {
                    handle_inner(log, &*conn, &message.params)
                })
            }
        }

        impl ::actix::prelude::Message for server::Message<Params> {
            type Result = Result<ViewModel>;
        }

        //
        // Private constants
        //

        const COOKIE_KEY_SECRET: &str = "secret";

        //
        // Private functions
        //

        fn handle_inner(log: &Logger, conn: &PgConnection, params: &Params) -> Result<ViewModel> {
            let account = match params.secret {
                Some(ref secret) => {
                    account_authenticator::Mediator {
                        conn:    conn,
                        last_ip: params.last_ip.as_str(),
                        secret:  secret.as_str(),
                    }.run(log)?
                        .account
                }
                None => {
                    // TODO: Don't bother for Google bots, etc.
                    Some(
                        account_creator::Mediator {
                            conn:      conn,
                            email:     None,
                            ephemeral: true,
                            last_ip:   params.last_ip.as_str(),
                        }.run(log)?
                            .account,
                    )
                }
            };
            Ok(ViewModel { account })
        }
    }
}
