pub mod log_initializer {
    use server;

    use actix_web;
    use actix_web::HttpRequest;
    use actix_web::middleware::Started;
    use slog::Logger;

    pub struct Middleware;

    pub struct Extension(pub Logger);

    impl<S: server::State> actix_web::middleware::Middleware<S> for Middleware {
        fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
            let log = req.state().log().clone();
            req.extensions().insert(Extension(log));
            Ok(Started::Done)
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
    use actix_web::HttpRequest;
    use actix_web::middleware::Started;

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
        use mediators;
        use middleware::log_initializer;
        use model;
        use server;
        use server::Params as P;
        use time_helpers;

        use actix_web;
        use actix_web::http::StatusCode;
        use actix_web::middleware::RequestSession;
        use actix_web::middleware::Started;
        use actix_web::{HttpRequest, HttpResponse};
        use diesel::pg::PgConnection;
        use futures::future;
        use slog::Logger;

        pub struct Middleware;

        struct Extension {
            account: Option<model::Account>,
        }

        impl<S: 'static + server::State> actix_web::middleware::Middleware<S> for Middleware {
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
                let mut req = req.clone();

                let fut = req.state()
                    .sync_addr()
                    .send(message)
                    .map_err(|_e| Error::from("Error from SyncExecutor"))
                    .flatten()
                    .then(move |res| match res {
                        Ok(view_model) => {
                            match view_model {
                                ViewModel::Bot => {
                                    req.extensions().insert(Extension { account: None });
                                }
                                ViewModel::ExistingAccount(account) => {
                                    req.extensions().insert(Extension {
                                        account: Some(account),
                                    });
                                }
                                ViewModel::NewAccount(account, key) => {
                                    req.extensions().insert(Extension {
                                        account: Some(account),
                                    });
                                    req.session()
                                        .set(COOKIE_KEY_SECRET, key.secret)
                                        .unwrap_or_else(|e| {
                                            error!(log, "Error setting session: {}", e)
                                        });
                                }
                            };
                            future::ok(None)
                        }
                        Err(e) => {
                            error!(log, "Middleware error: {}", e);
                            future::ok(None)
                        }
                    });
                Ok(Started::Future(Box::new(fut)))
            }
        }

        //
        // Public functions
        //

        /// Shorthand for getting the active authenticated account.
        ///
        /// An account is usually available because even if a user failed to
        /// authenticate, we'll create an account for them and store it to
        /// their session so that they can use the app in an unauthenticated
        /// way. However, `None` must still be handled because we don't
        /// bother creating an account if the `User-Agent` looks like a
        /// bot.
        pub fn account<S: server::State>(req: &mut HttpRequest<S>) -> Option<&model::Account> {
            req.extensions()
                .get::<Extension>()
                .unwrap()
                .account
                .as_ref()
        }

        //
        // Params
        //

        struct Params {
            last_ip:    String,
            secret:     Option<String>,
            user_agent: Option<String>,
        }

        impl server::Params for Params {
            fn build<S: server::State>(log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
                use actix_web::HttpMessage;
                Ok(Params {
                    last_ip:    req.connection_info().host().to_owned(),
                    secret:     req.session()
                        .get::<String>(COOKIE_KEY_SECRET)
                        .map_err(|_| Error::from("Error reading from session"))?,
                    user_agent: match req.headers().get("User-Agent") {
                        Some(s) => match s.to_str() {
                            Ok(s) => Some(s.to_owned()),
                            Err(e) => {
                                error!(log, "Error parsing `User-Agent`: {}", e);
                                None
                            }
                        },
                        None => None,
                    },
                })
            }
        }

        //
        // ViewModel
        //

        enum ViewModel {
            Bot,
            ExistingAccount(model::Account),
            NewAccount(model::Account, model::Key),
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

        // We're going to start with some pretty dumb heuristics to detect bots so that
        // we don't end up creating new user accounts for them as they crawl
        // endlessly. For now, we just examine user agents and if they contain
        // any of these strings, we skip account creation.
        const BOT_USER_AGENTS: &[&str] = &[
            "APIs-Google",
            "AdsBot-Google",
            "Googlebot/2.1",
            "Googlebot-Image/1.0",
            "Googlebot-News",
            "Googlebot-Video/1.0",
            "Mediapartners-Google",
        ];

        const COOKIE_KEY_SECRET: &str = "secret";

        //
        // Private functions
        //

        fn handle_inner(log: &Logger, conn: &PgConnection, params: &Params) -> Result<ViewModel> {
            if params.secret.is_some() {
                let account = mediators::account_authenticator::Mediator {
                    conn:    conn,
                    last_ip: params.last_ip.as_str(),
                    secret:  params.secret.as_ref().unwrap().as_str(),
                }.run(log)?
                    .account;

                // Only has a value if the authenticator passed successfully. We fall through in
                // the case of an invalid secret being presented.
                if account.is_some() {
                    return Ok(ViewModel::ExistingAccount(account.unwrap()));
                }
            }

            // Assume that an empty `User-Agent` is a bot
            if params.user_agent.is_none() {
                return Ok(ViewModel::Bot);
            }

            // Also run through a list of known bot `User-Agent` values
            let user_agent = params.user_agent.as_ref().unwrap();
            for &bot_user_agent in BOT_USER_AGENTS {
                if user_agent.contains(bot_user_agent) {
                    return Ok(ViewModel::Bot);
                }
            }

            let account = mediators::account_creator::Mediator {
                conn:      conn,
                email:     None,
                ephemeral: true,
                last_ip:   params.last_ip.as_str(),
            }.run(log)?
                .account;
            let key = mediators::key_creator::Mediator {
                account: &account,
                conn,
                expire_at: None,
            }.run(log)?
                .key;

            return Ok(ViewModel::NewAccount(account, key));
        }
    }
}
