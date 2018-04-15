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
    #[inline]
    pub fn log<S: server::State>(req: &mut HttpRequest<S>) -> Logger {
        req.extensions().get::<Extension>().unwrap().0.clone()
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use middleware::log_initializer::*;
        use test_helpers::IntegrationTestBootstrap;

        use actix_web::HttpResponse;
        use actix_web::http::{Method, StatusCode};

        #[test]
        fn test_middleware_log_initializer_integration() {
            let bootstrap = IntegrationTestBootstrap::new();
            let mut server = bootstrap.server_builder.start(|app| {
                app.middleware(Middleware)
                    .handler(|_req| HttpResponse::Ok())
            });

            let req = server.client(Method::GET, "/").finish().unwrap();
            let resp = server.execute(req.send()).unwrap();
            assert_eq!(StatusCode::OK, resp.status());
        }
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

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use middleware;
        use middleware::request_id::*;
        use test_helpers::IntegrationTestBootstrap;

        use actix_web::HttpResponse;
        use actix_web::http::{Method, StatusCode};

        #[test]
        fn test_middleware_request_id_integration() {
            let bootstrap = IntegrationTestBootstrap::new();
            let mut server = bootstrap.server_builder.start(|app| {
                app.middleware(middleware::log_initializer::Middleware)
                    .middleware(Middleware)
                    .handler(|_req| HttpResponse::Ok())
            });

            let req = server.client(Method::GET, "/").finish().unwrap();
            let resp = server.execute(req.send()).unwrap();
            assert_eq!(StatusCode::OK, resp.status());
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

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use middleware;
        use middleware::request_response_logger::*;
        use test_helpers::IntegrationTestBootstrap;

        use actix_web::HttpResponse;
        use actix_web::http::{Method, StatusCode};

        #[test]
        fn test_middleware_request_response_logger_integration() {
            let bootstrap = IntegrationTestBootstrap::new();
            let mut server = bootstrap.server_builder.start(|app| {
                app.middleware(middleware::log_initializer::Middleware)
                    .middleware(Middleware)
                    .handler(|_req| HttpResponse::Ok())
            });

            let req = server.client(Method::GET, "/").finish().unwrap();
            let resp = server.execute(req.send()).unwrap();
            assert_eq!(StatusCode::OK, resp.status());
        }
    }
}

pub mod api {
    #[allow(dead_code)]
    pub mod authenticator {
        use model;
        use server;

        use actix_web;
        use actix_web::HttpRequest;
        use actix_web::middleware::Started;

        pub struct Middleware;

        struct Extension {
            account: model::Account,
        }

        // This is in place to demonstrate what the `graphql` module using either type
        // of authenticator would look like, but is not implemented or used yet.
        impl<S: 'static + server::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, _req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                Ok(Started::Done)
            }
        }

        //
        // Public functions
        //

        #[inline]
        pub fn account<S: server::State>(req: &mut HttpRequest<S>) -> Option<&model::Account> {
            req.extensions()
                .get::<Extension>()
                .and_then(|e| Some(&e.account))
        }
    }
}

/// Holds middleware that are useful for testing.
pub mod test {
    pub mod authenticator {
        use middleware;
        use model;
        use server;

        use actix_web;
        use actix_web::HttpRequest;
        use actix_web::middleware::Started;

        /// The test authentication middleware.
        ///
        /// This needs to allow `dead_code` because it's only ever created from
        /// the test suite.
        ///
        /// This needs to be `Clone` because we need to be able to clone it
        /// into an `Fn` when building a test server.
        #[allow(dead_code)]
        #[derive(Clone)]
        pub struct Middleware {
            pub account: model::Account,
        }

        struct Extension {
            account: model::Account,
        }

        impl<S: 'static + server::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                let log = middleware::log_initializer::log(req);
                debug!(log, "Test authenticator setting account"; "id" => self.account.id);
                req.extensions().insert(Extension {
                    account: self.account.clone(),
                });
                Ok(Started::Done)
            }
        }

        //
        // Public functions
        //

        #[inline]
        pub fn account<S: server::State>(req: &mut HttpRequest<S>) -> Option<&model::Account> {
            req.extensions()
                .get::<Extension>()
                .and_then(|e| Some(&e.account))
        }
    }
}

/// Holds web-specific (as opposed to for the API) middleware.
///
/// This probably makes more sense as part of the actual `web` module. That
/// way, `web` wouldn't have to export `errors` so that this module could use
/// the functions therein.
pub mod web {
    pub mod authenticator {
        use errors::*;
        use mediators;
        use middleware;
        use model;
        use server;
        use server::Params as P;
        use time_helpers;
        use web;

        use actix_web;
        use actix_web::HttpRequest;
        use actix_web::http::Method;
        use actix_web::middleware::RequestSession;
        use actix_web::middleware::Started;
        use diesel::pg::PgConnection;
        use futures::future;
        use slog::Logger;

        // Gives us a `SyncExecutor` handler that trades `Params` for `ViewModel`. Runs
        // `handle_inner`.
        message_handler!();

        pub struct Middleware;

        struct Extension {
            account: Option<model::Account>,
        }

        impl<S: 'static + server::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                use futures::Future;

                let log =
                    middleware::log_initializer::log(req).new(o!("middleware" => "authenticator"));

                // Let anyone have our static assets
                if req.path().starts_with("/assets/") {
                    debug!(log, "Static asset; skipping authentication");
                    return Ok(Started::Done);
                }

                debug!(log, "Authenticating");

                let params_res = time_helpers::log_timed(
                    &log.new(o!("step" => "build_params")),
                    |log| Params::build(log, req),
                );
                let params = match params_res {
                    Ok(params) => params,
                    Err(e) => {
                        return Ok(Started::Response(server::render_error(
                            &log,
                            e,
                            web::errors::error_internal,
                            web::errors::error_user,
                        )));
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
                                ViewModel::NoAccount => {
                                    set_request_account(&log, &mut req, None);
                                }
                                ViewModel::ExistingAccount(account) => {
                                    set_request_account(&log, &mut req, Some(account));
                                }
                                ViewModel::NewAccount(account, key) => {
                                    set_request_account(&log, &mut req, Some(account));
                                    set_session_secret(&log, &mut req, &key.secret);
                                }
                            };
                            future::ok(None)
                        }
                        Err(e) => {
                            let response = server::render_error(
                                &log,
                                e,
                                web::errors::error_internal,
                                web::errors::error_user,
                            );
                            future::ok(Some(response))
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
        ///
        /// Returns `None` even this authenticator middleware was not active.
        #[inline]
        pub fn account<S: server::State>(req: &mut HttpRequest<S>) -> Option<&model::Account> {
            req.extensions()
                .get::<Extension>()
                .and_then(|e| e.account.as_ref())
        }

        //
        // Params
        //

        struct Params {
            is_get:     bool,
            last_ip:    String,
            secret:     Option<String>,
            user_agent: Option<String>,
        }

        impl server::Params for Params {
            fn build<S: server::State>(log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
                use actix_web::HttpMessage;
                Ok(Params {
                    is_get:     *req.method() == Method::GET,
                    last_ip:    req.connection_info().host().to_owned(),
                    secret:     req.session()
                        .get::<String>(COOKIE_KEY_SECRET)
                        .map_err(|_| Error::from("Error reading from session"))
                        .map(|s| {
                            // Don't actually log secrets. We rely on this being a `debug!`
                            // statement and being compiled out on any release build.
                            debug!(log, "Reading session secret"; "secret" => format!("{:?}", s));
                            s
                        })?,
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

        #[derive(Debug)]
        enum ViewModel {
            ExistingAccount(model::Account),
            NewAccount(model::Account, model::Key),
            NoAccount,
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
            "Applebot",
            "ArchiveBot",
            "Baiduspider",
            "BIGLOTRON",
            "BingPreview",
            "bingbot",
            "convera",
            "DuckDuckBot",
            "exabot",
            "FAST Enterprise Crawler",
            "FAST-WebCrawler",
            "Facebot",
            "Gigablast",
            "Gigabot",
            "GingerCrawler",
            "Googlebot/2.1",
            "Googlebot-Image/1.0",
            "Googlebot-News",
            "Googlebot-Video/1.0",
            "ia_archiver",
            "java",
            "jyxobot",
            "Mediapartners-Google",
            "msnbot",
            "nutch",
            "OrangeBot",
            "PhantomJS",
            "Pingdom",
            "phpcrawl",
            "pinterest",
            "redditbot",
            "SimpleCrawler",
            "Slackbot",
            "seekbot",
            "slurp",
            "spbot",
            "Teoma",
            "TinEye",
            "Twitterbot",
            "UptimeRobot",
            "voilabot",
            "WhatsApp",
            "Yahoo Link Preview",
            "YandexBot",
            "yandex.com",
            "yanga",
        ];

        const COOKIE_KEY_SECRET: &str = "secret";

        //
        // Private functions
        //

        fn handle_inner(log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
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

            // Don't create an account if a user is performing only `GET` (readonly)
            // requests. Instead, we wait until they perform some kind of
            // mutation (identified by a `POST`), then lazily create an account
            // for them.
            if params.is_get {
                debug!(log, "Request method is GET -- not creating account");
                return Ok(ViewModel::NoAccount);
            }

            if is_bot(&params) {
                debug!(log, "User-Agent is bot -- not creating account");
                return Ok(ViewModel::NoAccount);
            }

            let account = mediators::account_creator::Mediator {
                conn:      conn,
                email:     None,
                ephemeral: true,
                last_ip:   params.last_ip.as_str(),

                // This is very much a middleware only for use on the web, so `mobile` is false.
                // Mobile clients will create an account explicitly instead of automatically like
                // we're doing here.
                mobile: false,

                password:     None,
                scrypt_log_n: None,
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

        fn is_bot(params: &Params) -> bool {
            // Assume that an empty `User-Agent` is a bot
            if params.user_agent.is_none() {
                return true;
            }

            // Also run through a list of known bot `User-Agent` values
            let user_agent = params.user_agent.as_ref().unwrap();
            for &bot_user_agent in BOT_USER_AGENTS {
                if user_agent.contains(bot_user_agent) {
                    return true;
                }
            }

            return false;
        }

        fn set_request_account<S: server::State>(
            log: &Logger,
            req: &mut HttpRequest<S>,
            account: Option<model::Account>,
        ) {
            if account.is_none() {
                debug!(log, "Setting request account to none");
            } else {
                debug!(log, "Setting request account"; "id" => account.as_ref().unwrap().id);
            }

            req.extensions().insert(Extension { account });
        }

        /// Sets a secret to a client's session/cookie. Logs an error if there
        /// was a problem doing so.
        fn set_session_secret<S: server::State>(
            log: &Logger,
            req: &mut HttpRequest<S>,
            secret: &str,
        ) {
            // Don't actually log secrets. We rely on this being a `debug!` statement and
            // being compiled out on any release build.
            debug!(log, "Setting session secret"; "secret" => secret);

            req.session()
                .set(COOKIE_KEY_SECRET, secret)
                .unwrap_or_else(|e| error!(log, "Error setting session: {}", e));
        }

        //
        // Tests
        //

        #[cfg(test)]
        mod tests {
            use middleware;
            use middleware::web::authenticator::*;
            use test_data;
            use test_helpers;
            use test_helpers::IntegrationTestBootstrap;

            use actix_web::HttpResponse;
            use actix_web::http::{Method, StatusCode};
            use r2d2::PooledConnection;
            use r2d2_diesel::ConnectionManager;

            #[test]
            fn test_middleware_web_authenticator_integration() {
                let bootstrap = IntegrationTestBootstrap::new();
                let mut server = bootstrap.server_builder.start(|app| {
                    app.middleware(middleware::log_initializer::Middleware)
                        .middleware(Middleware)
                        .handler(|_req| HttpResponse::Ok())
                });

                let req = server.client(Method::POST, "/").finish().unwrap();
                let resp = server.execute(req.send()).unwrap();
                assert_eq!(StatusCode::OK, resp.status());
            }

            // We don't create accounts on `GET` requests and instead wait until the agent
            // performs a mutation before making one more them.
            #[test]
            fn test_middleware_web_authenticator_is_get() {
                let bootstrap = TestBootstrap::new();

                let params = Params {
                    is_get:     true,
                    last_ip:    "1.2.3.4".to_owned(),
                    secret:     None,
                    user_agent: Some("Chrome".to_owned()),
                };

                let view_model = handle_inner(&bootstrap.log, &bootstrap.conn, params).unwrap();
                match view_model {
                    ViewModel::NoAccount => (),
                    _ => panic!("Unexpected view model: {:?}", view_model),
                }
            }

            #[test]
            fn test_middleware_web_authenticator_bot() {
                let bootstrap = TestBootstrap::new();

                let params = Params {
                    is_get:     false,
                    last_ip:    "1.2.3.4".to_owned(),
                    secret:     None,
                    user_agent: Some("Googlebot/2.1; Some Other Stuff".to_owned()),
                };

                let view_model = handle_inner(&bootstrap.log, &bootstrap.conn, params).unwrap();
                match view_model {
                    ViewModel::NoAccount => (),
                    _ => panic!("Unexpected view model: {:?}", view_model),
                }
            }

            #[test]
            fn test_middleware_web_authenticator_no_user_agent() {
                let bootstrap = TestBootstrap::new();

                let params = Params {
                    is_get:     false,
                    last_ip:    "1.2.3.4".to_owned(),
                    secret:     None,
                    user_agent: None,
                };

                let view_model = handle_inner(&bootstrap.log, &bootstrap.conn, params).unwrap();
                match view_model {
                    ViewModel::NoAccount => (),
                    _ => panic!("Unexpected view model: {:?}", view_model),
                }
            }

            #[test]
            fn test_middleware_web_authenticator_existing_account() {
                let bootstrap = TestBootstrap::new();

                let account = test_data::account::insert(&bootstrap.log, &bootstrap.conn);
                let key = test_data::key::insert_args(
                    &bootstrap.log,
                    &bootstrap.conn,
                    test_data::key::Args {
                        account:   Some(&account),
                        expire_at: None,
                    },
                );

                let params = Params {
                    is_get:     false,
                    last_ip:    "1.2.3.4".to_owned(),
                    secret:     Some(key.secret.clone()),
                    user_agent: Some("Chrome".to_owned()),
                };

                let view_model = handle_inner(&bootstrap.log, &bootstrap.conn, params).unwrap();
                match view_model {
                    ViewModel::ExistingAccount(actual_account) => {
                        assert_eq!(account.id, actual_account.id);
                    }
                    _ => panic!("Unexpected view model: {:?}", view_model),
                }
            }

            #[test]
            fn test_middleware_web_authenticator_new_account() {
                let bootstrap = TestBootstrap::new();

                let params = Params {
                    is_get:     false,
                    last_ip:    "1.2.3.4".to_owned(),
                    secret:     None,
                    user_agent: Some("Chrome".to_owned()),
                };

                let view_model = handle_inner(&bootstrap.log, &bootstrap.conn, params).unwrap();
                match view_model {
                    ViewModel::NewAccount(account, key) => {
                        assert_ne!(0, account.id);
                        assert_ne!(0, key.id);
                    }
                    _ => panic!("Unexpected view model: {:?}", view_model),
                }
            }

            struct TestBootstrap {
                _common: test_helpers::CommonTestBootstrap,
                conn:    PooledConnection<ConnectionManager<PgConnection>>,
                log:     Logger,
            }

            impl TestBootstrap {
                fn new() -> TestBootstrap {
                    TestBootstrap {
                        _common: test_helpers::CommonTestBootstrap::new(),
                        conn:    test_helpers::connection(),
                        log:     test_helpers::log(),
                    }
                }
            }
        }
    }
}
