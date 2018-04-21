use errors::*;
use http_requester::HttpRequesterLive;
use model;
use server;

use actix_web::http::StatusCode;
use actix_web::{HttpRequest, HttpResponse};
use hyper::Client;
use hyper_tls::HttpsConnector;
use slog::Logger;
use tokio_core::reactor::Core;

//
// Macros
//

/// Creates an asynchronous HTTP handler function suitable for use with Actix
/// for the current endpoint module.
///
/// The key point to understand here is that because we have a convention so
/// that all `server::Params` and `ViewModel`s are given the same name in every
/// module, this can be pulled in and expanded while still properly resolving
/// symbols.
///
/// Honestly, I would've preferred not to have to sink into a macro to get this
/// working, but I started running into some serious typing problems when
/// trying to make this a generic function. Be it with generics or associated
/// types I'd always get a complaint from the compiler that there
/// was no implementation for the generic version when sending a message to
/// Actix (and in a few other places). After trying many different approaches
/// and failing on all of them, I eventually just resorted to this. To keep
/// things clean, offload as much work as possible to functions outside of the
/// macro. Try to change this as little as possible.
macro_rules! handler {
    () => {
        pub fn handler(
            mut req: HttpRequest<server::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            // Imported so that we can use the traits, but assigned a different name to
            // avoid clashing with the module's implementations.
            use server::Params as P;
            use server::State;
            use web;
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use actix_web::AsyncResponder;
            use futures::future;

            let log = middleware::log_initializer::log(&mut req);

            let params_res = time_helpers::log_timed(
                &log.new(o!("step" => "build_params")),
                |log| Params::build(log, &mut req, None),
            );
            let params = match params_res {
                Ok(params) => params,
                Err(e) => {
                    let response = server::render_error(
                        &log,
                        e,
                        web::errors::error_internal,
                        web::errors::error_user,
                    );
                    return Box::new(future::ok(response));
                }
            };

            let message = server::Message::new(&log, params);

            // We need `log` clones because we have multiple `move` closures below (and only
            // one can take the original log).
            let log2 = log.clone();

            req.state()
                .get_sync_addr()
                .send(message)
                .map_err(|_e| Error::from("Error from SyncExecutor"))
                .flatten()
                .and_then(move |view_model| {
                    time_helpers::log_timed(&log.new(o!("step" => "render_view_model")), |log| {
                        view_model.render(log, &mut req)
                    })
                })
                .then(move |res| match res {
                    Err(e) => Ok(server::render_error(
                        &log2,
                        e,
                        web::errors::error_internal,
                        web::errors::error_user,
                    )),
                    r => r,
                })
                .responder()
        }
    };
}

/// Identical to `handler!` except that it also waits on the future to receive
/// request body data. This will be usually need to be used instead of
/// `handler!` for handling `POST` requests.
macro_rules! handler_post {
    () => {
        pub fn handler(
            mut req: HttpRequest<server::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            // Imported so that we can use the traits, but assigned a different name to
            // avoid clashing with the module's implementations.
            use server::Params as P;
            use server::State;
            use web;
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use actix_web::AsyncResponder;
            use actix_web::HttpMessage;
            use bytes::Bytes;

            let log = middleware::log_initializer::log(&mut req);

            // We need `log` and `req` clones because we have multiple `move` closures below (and
            // only one can take the original of each object).
            let log2 = log.clone();
            let log3 = log.clone();
            let log4 = log.clone();
            let req2 = req.clone();
            let mut req3 = req.clone();
            let sync_addr = req.state().get_sync_addr().clone();

            req2.body()
                // `map_err` is used here instead of `chain_err` because `PayloadError` doesn't
                // implement the `Error` trait and I was unable to put it in the error chain.
                .map_err(|_e| Error::from("Error reading request body"))
                .and_then(move |bytes: Bytes| {
                    time_helpers::log_timed(&log.new(o!("step" => "build_params")), |log| {
                        Params::build(log, &mut req3, Some(bytes.as_ref()))
                    })
                })
                .and_then(move |params| {
                    let message = server::Message::new(&log2, params);
                    sync_addr
                        .send(message)
                        .map_err(|_e| Error::from("Future canceled"))
                })
                .flatten()
                .and_then(move |view_model| {
                    time_helpers::log_timed(&log3.new(o!("step" => "render_view_model")), |log| {
                        // We use the *original* request here because this function might set a
                        // cookie and I'm worried that this won't work on a cloned copy (it'd be
                        // good to verify this one way or the other with some investigation
                        // though).
                        view_model.render(log, &mut req)
                    })
                })
                .then(move |res| match res {
                    Err(e) => Ok(server::render_error(
                        &log4,
                        e,
                        web::errors::error_internal,
                        web::errors::error_user,
                    )),
                    r => r,
                })
                .responder()
        }
    };
}

/// Identical to `handler!` except useful in cases where the
/// `server::SyncExecutor` doesn't need to do any work. Skips sending a
/// blocking message to `server::SyncExecutor` and getting a Postgres connection
/// from the pool to increase performance and avoid contention.
macro_rules! handler_noop {
    () => {
        pub fn handler(
            mut req: HttpRequest<server::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            use time_helpers;
            // Imported so that we can use the traits, but assigned a different name to
            // avoid clashing with the module's implementations.
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use futures::future;

            let log = middleware::log_initializer::log(&mut req);

            let view_model = time_helpers::log_timed(
                &log.new(o!("step" => "build_view_model")),
                |log| ViewModel::build(log, &mut req),
            );
            let response_res = time_helpers::log_timed(
                &log.new(o!("step" => "render_view_model")),
                |log| view_model.render(log, &mut req),
            );
            let response = match response_res {
                Ok(response) => response,
                Err(e) => return Box::new(future::err(e)),
            };

            Box::new(future::ok(response))
        }
    };
}

//
// Traits
//

/// A trait to be implemented by the view models that render views, which is
/// also the same trait for the typed responses that come from
/// `server::SyncExecutor`. A view model is a model containing all the
/// information needed to build a view.
pub trait ViewModel {
    /// Renders a `ViewModel` implementation to an HTTP response. This could be
    /// a standard HTML page, but could also be any arbitrary response like
    /// a redirect.
    fn render(
        &self,
        log: &Logger,
        req: &mut HttpRequest<server::StateImpl>,
    ) -> Result<HttpResponse>;
}

//
// Structs
//

pub struct CommonViewModel<'a> {
    pub account:        Option<&'a model::Account>,
    pub assets_version: String,
    pub title:          String,
}

//
// Functions
//

/// Builds a `CommonViewModel` from request information and takes in any other
/// required parameters to do so.
fn build_common<'a>(
    req: &HttpRequest<server::StateImpl>,
    account: Option<&'a model::Account>,
    title: &str,
) -> CommonViewModel<'a> {
    CommonViewModel {
        account:        account,
        assets_version: req.state().assets_version.clone(),
        title:          title.to_owned(),
    }
}

fn build_requester() -> Result<HttpRequesterLive> {
    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
        .build(&core.handle());
    Ok(HttpRequesterLive { client, core })
}

/// Shortcut for a basic 200 response with standard HTML body content.
pub fn respond_200(body: String) -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(body))
}

//
// Endpoints
//

pub mod episode_get {
    use errors::*;
    use links;
    use model;
    use schema;
    use server;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse};
    use diesel::prelude::*;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        account:    Option<model::Account>,
        episode_id: i64,
        podcast_id: i64,
    }

    impl server::Params for Params {
        fn build<S: server::State>(
            _log: &Logger,
            req: &mut HttpRequest<S>,
            _data: Option<&[u8]>,
        ) -> Result<Self> {
            Ok(Self {
                account:    server::account(req),
                episode_id: links::unslug_id(req.match_info().get("id").unwrap())
                    .map_err(|e| error::bad_parameter("episode_id", &e))?,
                podcast_id: links::unslug_id(req.match_info().get("podcast_id").unwrap())
                    .map_err(|e| error::bad_parameter("podcast_id", &e))?,
            })
        }
    }

    //
    // Handler
    //

    fn handle_inner(_log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
        let episode: Option<model::Episode> = schema::episode::table
            .filter(schema::episode::id.eq(params.episode_id))
            .filter(schema::episode::podcast_id.eq(params.podcast_id))
            .first(conn)
            .optional()?;
        match episode {
            Some(episode) => {
                let tuple: Option<(
                    model::AccountPodcastEpisode,
                    model::AccountPodcast,
                )> = match params.account {
                    Some(ref account) => schema::account_podcast_episode::table
                        .inner_join(schema::account_podcast::table)
                        .filter(schema::account_podcast::account_id.eq(account.id))
                        .filter(schema::account_podcast::podcast_id.eq(episode.podcast_id))
                        .filter(schema::account_podcast_episode::episode_id.eq(episode.id))
                        .first(conn)
                        .optional()?,
                    None => None,
                };

                Ok(ViewModel::Ok(view_model::Ok {
                    account: params.account,
                    account_podcast_episode: tuple.map(|t| t.0),
                    episode,
                }))
            }
            None => Err(error::not_found("episode", params.episode_id)),
        }
    }

    //
    // ViewModel
    //

    pub enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        pub struct Ok {
            pub account:                 Option<model::Account>,
            pub account_podcast_episode: Option<model::AccountPodcastEpisode>,
            pub episode:                 model::Episode,
        }

        static MEDIA_TYPE_DEFAULT: &'static str = "audio/mpeg";

        impl Ok {
            pub fn episode_media_type_or_default(&self) -> &str {
                self.episode
                    .media_type
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or(MEDIA_TYPE_DEFAULT)
            }

            pub fn is_episode_favorited(&self) -> bool {
                match self.account_podcast_episode {
                    Some(ref episode) => episode.favorited,
                    None => false,
                }
            }

            pub fn is_episode_played(&self) -> bool {
                match self.account_podcast_episode {
                    Some(ref episode) => episode.played,
                    None => false,
                }
            }
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        &format!("Episode: {}", view_model.episode.title.as_str()),
                    );
                    endpoints::respond_200(views::episode_get::render(&common, view_model)?)
                }
            }
        }
    }
}

pub mod directory_podcast_get {
    use errors::*;
    use links;
    use mediators::directory_podcast_updater;
    use model;
    use schema;
    use server;
    use time_helpers;
    use web::endpoints;

    use actix_web::http::StatusCode;
    use actix_web::{HttpRequest, HttpResponse};
    use diesel::prelude::*;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        directory_podcast_id: i64,
    }

    impl server::Params for Params {
        fn build<S: server::State>(
            _log: &Logger,
            req: &mut HttpRequest<S>,
            _data: Option<&[u8]>,
        ) -> Result<Self> {
            Ok(Self {
                directory_podcast_id: links::unslug_id(req.match_info().get("id").unwrap())
                    .map_err(|e| error::bad_parameter("directory_podcast_id", &e))?,
            })
        }
    }

    //
    // Handler
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
        info!(log, "Expanding directory podcast"; "id" => params.directory_podcast_id);

        let dir_podcast: Option<model::DirectoryPodcast> = schema::directory_podcast::table
            .filter(schema::directory_podcast::id.eq(params.directory_podcast_id))
            .first(conn)
            .optional()?;
        match dir_podcast {
            Some(mut dir_podcast) => {
                let mut mediator = directory_podcast_updater::Mediator {
                    conn,
                    dir_podcast: &mut dir_podcast,
                    http_requester: &mut endpoints::build_requester()?,
                };
                let res = mediator.run(log)?;
                Ok(ViewModel::Ok(res.podcast))
            }
            None => Err(error::not_found(
                "directory_podcast",
                params.directory_podcast_id,
            )),
        }
    }

    //
    // ViewModel
    //

    pub enum ViewModel {
        Ok(model::Podcast),
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            _req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    // This could really be a permanent redirect, but just to make debugging
                    // easier, I have it set as a temporary redirect so that I can reuse it across
                    // database cleans without the browser caching a result that's since invalid.
                    Ok(HttpResponse::build(StatusCode::TEMPORARY_REDIRECT)
                        .header("Location", links::link_podcast(view_model).as_str())
                        .finish())
                }
            }
        }
    }
}

pub mod login_get {
    use errors::*;
    use server;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse};
    use futures::future::Future;
    use slog::Logger;

    handler_noop!();

    //
    // ViewModel
    //

    #[derive(Debug)]
    pub enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        #[derive(Debug)]
        pub struct Ok {
            pub account: Option<model::Account>,
            pub message: Option<String>,
        }
    }

    impl ViewModel {
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> ViewModel {
            ViewModel::Ok(view_model::Ok {
                account: server::account(req),
                message: None,
            })
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(req, view_model.account.as_ref(), "Login");
                    endpoints::respond_200(views::login_get::render(&common, view_model)?)
                }
            }
        }
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use test_helpers;
        use web::endpoints::login_get::*;
        use web::endpoints::ViewModel as VM;

        use actix_web::test::TestRequest;

        //
        // ViewModel tests
        //

        #[test]
        fn test_login_get_view_model_build() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();
            let view_model = ViewModel::build(&bootstrap.log, &mut req);

            match view_model {
                ViewModel::Ok(view_model::Ok {
                    account: None,
                    message: None,
                }) => (),
                _ => panic!("Unexpected view model: {:?}", view_model),
            };
        }

        #[test]
        fn test_login_get_view_model_render_ok() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let view_model = ViewModel::Ok(view_model::Ok {
                account: None,
                message: Some("Hello, world.".to_owned()),
            });
            let _response = view_model.render(&bootstrap.log, &mut req).unwrap();
        }

        //
        // Private types/functions
        //

        struct TestBootstrap {
            _common: test_helpers::CommonTestBootstrap,
            log:     Logger,
        }

        impl TestBootstrap {
            fn new() -> TestBootstrap {
                TestBootstrap {
                    _common: test_helpers::CommonTestBootstrap::new(),
                    log:     test_helpers::log(),
                }
            }
        }
    }
}

pub mod login_post {
    use errors::*;
    use mediators;
    use middleware;
    use model;
    use server;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::http::StatusCode;
    use actix_web::{HttpRequest, HttpResponse};
    use diesel::pg::PgConnection;
    use futures::future::Future;
    use serde_urlencoded;
    use slog::Logger;

    handler_post!();
    message_handler!();

    //
    // Params
    //

    /// Gets the value for the given parameter name or returns a "parameter
    /// missing" error.

    struct Params {
        account:  Option<model::Account>,
        email:    String,
        last_ip:  String,
        password: String,
    }

    impl server::Params for Params {
        fn build<S: server::State>(
            _log: &Logger,
            req: &mut HttpRequest<S>,
            data: Option<&[u8]>,
        ) -> Result<Self> {
            let form = serde_urlencoded::from_bytes::<ParamsForm>(data.unwrap())
                .map_err(|e| error::bad_request(format!("{}", e)))?;

            // The missing parameter errors are "hard" errors that usually the user will
            // not see because even empty fields will be submitted with an HTML
            // form. There's also validations in the mediator that check each
            // value for content which will pass a more digestible error back
            // to the user.
            Ok(Params {
                account:  server::account(req),
                email:    form.email.ok_or_else(|| error::missing_parameter("email"))?,
                last_ip:  server::ip_for_request(req).to_owned(),
                password: form.password
                    .ok_or_else(|| error::missing_parameter("password"))?,
            })
        }
    }

    /// A parameters struct solely intended to be a target for form decoding.
    #[derive(Debug, Deserialize)]
    struct ParamsForm {
        email:    Option<String>,
        password: Option<String>,
    }

    //
    // Handler
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
        let res = mediators::account_password_authenticator::Mediator {
            conn,
            email: params.email.as_str(),
            last_ip: params.last_ip.as_str(),
            password: params.password.as_str(),
        }.run(log);

        if let Err(Error(ErrorKind::Validation(message), _)) = res {
            return message_invalid(params.account, message.as_str());
        }

        let res = res?;
        Ok(ViewModel::Ok(view_model::Ok {
            account: res.account,
            key:     res.key,
        }))
    }

    //
    // ViewModel
    //

    #[derive(Debug)]
    enum ViewModel {
        Invalid(endpoints::login_get::view_model::Ok),
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        #[derive(Debug)]
        pub struct Ok {
            pub account: model::Account,
            pub key:     model::Key,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Invalid(ref view_model) => {
                    let common = endpoints::build_common(req, view_model.account.as_ref(), "Login");
                    endpoints::respond_200(views::login_get::render(&common, view_model)?)
                }
                ViewModel::Ok(ref view_model) => {
                    // Note that we don't set the account state for *this* request because we're
                    // just redirecting right away. If that ever changes, we should set account
                    // state.
                    middleware::web::authenticator::set_session_key(log, req, &view_model.key);

                    Ok(HttpResponse::build(StatusCode::TEMPORARY_REDIRECT)
                        .header("Location", "/account")
                        .finish())
                }
            }
        }
    }

    //
    // Private functions
    //

    fn message_invalid(account: Option<model::Account>, message: &str) -> Result<ViewModel> {
        Ok(ViewModel::Invalid(endpoints::login_get::view_model::Ok {
            account: account,
            message: Some(message.to_owned()),
        }))
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use server::Params as P;
        use test_data;
        use test_helpers;
        use web::endpoints::login_post::*;
        use web::endpoints::ViewModel as VM;

        use actix_web::test::TestRequest;
        use r2d2::PooledConnection;
        use r2d2_diesel::ConnectionManager;

        //
        // Params tests
        //

        #[test]
        fn test_login_post_params() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();
            let params = Params::build(
                &bootstrap.log,
                &mut req,
                Some(b"email=foo@example.com&password=my-password"),
            ).unwrap();
            assert!(params.account.is_none());
            assert_eq!("foo@example.com", params.email);
            assert_eq!(test_helpers::REQUEST_IP, params.last_ip);
            assert_eq!("my-password", params.password);
        }

        //
        // Handler tests
        //

        #[test]
        fn test_login_post_handler_ok() {
            let bootstrap = TestBootstrap::new();

            let account = test_data::account::insert_args(
                &bootstrap.log,
                &*bootstrap.conn,
                test_data::account::Args {
                    email:     Some(TEST_EMAIL),
                    ephemeral: false,
                    mobile:    false,
                },
            );
            let _key = test_data::key::insert_args(
                &bootstrap.log,
                &*bootstrap.conn,
                test_data::key::Args {
                    account:   Some(&account),
                    expire_at: None,
                },
            );

            let view_model =
                handle_inner(&bootstrap.log, &*bootstrap.conn, valid_params()).unwrap();

            match view_model {
                ViewModel::Ok(_) => (),
                _ => panic!("Unexpected view model: {:?}", view_model),
            };
        }

        //
        // ViewModel tests
        //

        #[test]
        fn test_login_post_view_model_render_invalid() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let view_model = ViewModel::Invalid(endpoints::login_get::view_model::Ok {
                account: None,
                message: Some("Invalid action.".to_owned()),
            });
            let _response = view_model.render(&bootstrap.log, &mut req).unwrap();
        }

        #[test]
        fn test_login_post_view_model_render_ok() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let account = test_data::account::insert(&bootstrap.log, &*bootstrap.conn);
            let key = test_data::key::insert_args(
                &bootstrap.log,
                &*bootstrap.conn,
                test_data::key::Args {
                    account:   Some(&account),
                    expire_at: None,
                },
            );

            let view_model = ViewModel::Ok(view_model::Ok { account, key });
            let response = view_model.render(&bootstrap.log, &mut req).unwrap();
            assert_eq!(StatusCode::TEMPORARY_REDIRECT, response.status());
            assert_eq!("/account", response.headers().get("Location").unwrap());
        }

        //
        // Private types/functions
        //

        static TEST_EMAIL: &str = "foo@example.com";

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

        fn valid_params() -> Params {
            Params {
                account:  None,
                email:    TEST_EMAIL.to_owned(),
                last_ip:  test_helpers::REQUEST_IP.to_owned(),
                password: test_helpers::PASSWORD.to_owned(),
            }
        }
    }
}

pub mod logout_get {
    use errors::*;
    use middleware;
    use server;
    use web::endpoints;

    use actix_web::http::StatusCode;
    use actix_web::{HttpRequest, HttpResponse};
    use futures::future::Future;
    use slog::Logger;

    handler_noop!();

    //
    // ViewModel
    //

    #[derive(Debug)]
    pub enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        #[derive(Debug)]
        pub struct Ok {}
    }

    impl ViewModel {
        fn build<S: server::State>(_log: &Logger, _req: &mut HttpRequest<S>) -> ViewModel {
            ViewModel::Ok(view_model::Ok {})
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref _view_model) => {
                    middleware::web::authenticator::remove_session_key(log, req);
                    Ok(HttpResponse::build(StatusCode::TEMPORARY_REDIRECT)
                        .header("Location", "/")
                        .finish())
                }
            }
        }
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use test_helpers;
        use web::endpoints::logout_get::*;
        use web::endpoints::ViewModel as VM;

        use actix_web::test::TestRequest;

        //
        // ViewModel tests
        //

        #[test]
        fn test_logout_get_view_model_build() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();
            let view_model = ViewModel::build(&bootstrap.log, &mut req);

            match view_model {
                ViewModel::Ok(view_model::Ok {}) => (),
            };
        }

        #[test]
        fn test_logout_get_view_model_render_ok() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let view_model = ViewModel::Ok(view_model::Ok {});
            let response = view_model.render(&bootstrap.log, &mut req).unwrap();
            assert_eq!(StatusCode::TEMPORARY_REDIRECT, response.status());
            assert_eq!("/", response.headers().get("Location").unwrap());
        }

        //
        // Private types/functions
        //

        struct TestBootstrap {
            _common: test_helpers::CommonTestBootstrap,
            log:     Logger,
        }

        impl TestBootstrap {
            fn new() -> TestBootstrap {
                TestBootstrap {
                    _common: test_helpers::CommonTestBootstrap::new(),
                    log:     test_helpers::log(),
                }
            }
        }
    }
}

pub mod podcast_get {
    use errors::*;
    use links;
    use model;
    use schema;
    use server;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse};
    use diesel::prelude::*;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        account:    Option<model::Account>,
        podcast_id: i64,
    }

    impl server::Params for Params {
        fn build<S: server::State>(
            _log: &Logger,
            req: &mut HttpRequest<S>,
            _data: Option<&[u8]>,
        ) -> Result<Self> {
            Ok(Self {
                account:    server::account(req),
                podcast_id: links::unslug_id(req.match_info().get("id").unwrap())
                    .map_err(|e| error::bad_parameter("podcast_id", &e))?,
            })
        }
    }

    //
    // Handler
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
        info!(log, "Looking up podcast"; "id" => params.podcast_id);
        let podcast: Option<model::Podcast> = schema::podcast::table
            .filter(schema::podcast::id.eq(params.podcast_id))
            .first(&*conn)
            .optional()?;
        match podcast {
            Some(podcast) => {
                let episodes: Vec<model::Episode> = schema::episode::table
                    .filter(schema::episode::podcast_id.eq(podcast.id))
                    .order(schema::episode::published_at.desc())
                    .limit(50)
                    .load(&*conn)?;

                let account_podcast = match params.account {
                    Some(ref account) => schema::account_podcast::table
                        .filter(schema::account_podcast::account_id.eq(account.id))
                        .filter(schema::account_podcast::podcast_id.eq(podcast.id))
                        .get_result(conn)
                        .optional()?,
                    None => None,
                };

                Ok(ViewModel::Ok(view_model::Ok {
                    account_podcast,
                    account: params.account,
                    episodes,
                    podcast,
                }))
            }
            None => Err(error::not_found("podcast", params.podcast_id)),
        }
    }

    //
    // ViewModel
    //

    pub enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        pub struct Ok {
            pub account:         Option<model::Account>,
            pub account_podcast: Option<model::AccountPodcast>,
            pub episodes:        Vec<model::Episode>,
            pub podcast:         model::Podcast,
        }

        impl Ok {
            // The likelihood is that we'll want a `account_podcast` relation at some
            // point, so this helper exists partly for forward compatibility,
            // and partly to help establish convention for this kind of pattern.
            pub fn is_subscribed(&self) -> bool {
                match self.account_podcast {
                    Some(ref account_podcast) => account_podcast.is_subscribed(),
                    None => false,
                }
            }
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        &format!("Podcast: {}", view_model.podcast.title.as_str()),
                    );
                    endpoints::respond_200(views::podcast_get::render(&common, view_model)?)
                }
            }
        }
    }
}

pub mod search_get {
    use errors::*;
    use mediators::directory_podcast_searcher;
    use model;
    use schema;
    use server;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse};
    use diesel::pg::PgConnection;
    use diesel::prelude::*;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        account: Option<model::Account>,
        query:   Option<String>,
    }
    impl server::Params for Params {
        fn build<S: server::State>(
            _log: &Logger,
            req: &mut HttpRequest<S>,
            _data: Option<&[u8]>,
        ) -> Result<Self> {
            Ok(Self {
                account: server::account(req),
                query:   req.query().get("q").map(|q| q.to_owned()),
            })
        }
    }

    //
    // Handler
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
        if params.query.is_none() || params.query.as_ref().unwrap().is_empty() {
            return Ok(ViewModel::Ok(view_model::Ok {
                account: params.account,
                directory_podcasts_and_podcasts: None,
                query: None,
                title: "Search".to_owned(),
            }));
        }

        let query = params.query.clone().unwrap();
        info!(log, "Executing query"; "id" => query.as_str());

        let res = directory_podcast_searcher::Mediator {
            conn:           &*conn,
            query:          query.to_owned(),
            http_requester: &mut endpoints::build_requester()?,
        }.run(log)?;

        // This uses a join to get us the podcast records along with the directory
        // podcast records (the former being an `Option`). We might want to
        // move this back into the searcher mediator because we're kind of
        // duplicating work by having this out here.
        let directory_podcasts_and_podcasts = Some(load_directory_podcasts_and_podcasts(
            log,
            &*conn,
            &res.directory_search,
        )?);

        Ok(ViewModel::Ok(view_model::Ok {
            account: params.account,
            directory_podcasts_and_podcasts,
            title: format!("Search: {}", query.as_str()),

            // Moves into the struct, so set after setting `title`.
            query: Some(query),
        }))
    }

    //
    // ViewModel
    //

    enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        pub struct Ok {
            pub account: Option<model::Account>,
            pub directory_podcasts_and_podcasts:
                Option<Vec<(model::DirectoryPodcast, Option<model::Podcast>)>>,
            pub query: Option<String>,
            pub title: String,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        view_model.title.as_str(),
                    );
                    endpoints::respond_200(views::search_get::render(&common, view_model)?)
                }
            }
        }
    }

    //
    // Private functions
    //

    fn load_directory_podcasts_and_podcasts(
        log: &Logger,
        conn: &PgConnection,
        directory_search: &model::DirectorySearch,
    ) -> Result<Vec<(model::DirectoryPodcast, Option<model::Podcast>)>> {
        let tuples = time_helpers::log_timed(
            &log.new(o!("step" => "load_directory_podcasts_and_podcasts")),
            |_log| {
                schema::directory_podcast_directory_search::table
                    .inner_join(
                        schema::directory_podcast::table.left_outer_join(schema::podcast::table),
                    )
                    .filter(
                        schema::directory_podcast_directory_search::directory_search_id
                            .eq(directory_search.id),
                    )
                    .order(schema::directory_podcast_directory_search::position)
                    .load::<(
                        model::DirectoryPodcastDirectorySearch,
                        (model::DirectoryPodcast, Option<model::Podcast>),
                    )>(&*conn)
                    .chain_err(|| "Error loading directory search/podcast tuples")
            },
        )?;
        Ok(tuples.into_iter().map(|t| t.1).collect())
    }
}

/*
fn param_or_missing<S: server::State>(req: &mut HttpRequest<S>, name: &str) -> Result<String> {
    match req.query().get(name) {
        Some(val) => Ok(val.to_owned()),
        None => Err(error::missing_parameter(name)),
    }
}
*/

pub mod signup_post {
    use errors::*;
    use mediators;
    use middleware;
    use model;
    use server;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::http::StatusCode;
    use actix_web::{HttpRequest, HttpResponse};
    use diesel::pg::PgConnection;
    use futures::future::Future;
    use serde_urlencoded;
    use slog::Logger;

    handler_post!();
    message_handler!();

    //
    // Params
    //

    /// Gets the value for the given parameter name or returns a "parameter
    /// missing" error.

    struct Params {
        account:          Option<model::Account>,
        email:            String,
        last_ip:          String,
        password:         String,
        password_confirm: String,
        scrypt_log_n:     u8,
    }

    impl server::Params for Params {
        fn build<S: server::State>(
            _log: &Logger,
            req: &mut HttpRequest<S>,
            data: Option<&[u8]>,
        ) -> Result<Self> {
            let form = serde_urlencoded::from_bytes::<ParamsForm>(data.unwrap())
                .map_err(|e| error::bad_request(format!("{}", e)))?;

            // The missing parameter errors are "hard" errors that usually the user will
            // not see because even empty fields will be submitted with an HTML
            // form. There's also validations in the mediator that check each
            // value for content which will pass a more digestible error back
            // to the user.
            Ok(Params {
                account:          server::account(req),
                email:            form.email.ok_or_else(|| error::missing_parameter("email"))?,
                last_ip:          server::ip_for_request(req).to_owned(),
                password:         form.password
                    .ok_or_else(|| error::missing_parameter("password"))?,
                password_confirm: form.password_confirm
                    .ok_or_else(|| error::missing_parameter("password_confirm"))?,
                scrypt_log_n:     req.state().get_scrypt_log_n(),
            })
        }
    }

    /// A parameters struct solely intended to be a target for form decoding.
    #[derive(Debug, Deserialize)]
    struct ParamsForm {
        email:            Option<String>,
        password:         Option<String>,
        password_confirm: Option<String>,
    }

    //
    // Handler
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: Params) -> Result<ViewModel> {
        // Most validations happen within the mediator, but we do a few of them here
        // for fields that are not available there.
        if params.password_confirm.is_empty() {
            return message_invalid(
                params.account,
                "Please type your password again in the confirmation box.",
            );
        }
        if params.password != params.password_confirm {
            return message_invalid(
                params.account,
                "Password and password confirmation didn't match.",
            );
        }

        // TODO: Should take existing account for merge.
        let res = mediators::account_creator::Mediator {
            conn,
            create_key: true,
            email: Some(params.email.as_str()),
            ephemeral: false,
            last_ip: params.last_ip.as_str(),
            mobile: false,
            password: Some(params.password.as_str()),
            scrypt_log_n: Some(params.scrypt_log_n),
        }.run(log);

        if let Err(Error(ErrorKind::Validation(message), _)) = res {
            return message_invalid(params.account, message.as_str());
        }

        let res = res?;
        Ok(ViewModel::Ok(view_model::Ok {
            account: res.account,
            key:     res.key.unwrap(),
        }))
    }

    //
    // ViewModel
    //

    #[derive(Debug)]
    enum ViewModel {
        Invalid(endpoints::signup_get::view_model::Ok),
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        #[derive(Debug)]
        pub struct Ok {
            pub account: model::Account,
            pub key:     model::Key,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Invalid(ref view_model) => {
                    let common =
                        endpoints::build_common(req, view_model.account.as_ref(), "Signup");
                    endpoints::respond_200(views::signup_get::render(&common, view_model)?)
                }
                ViewModel::Ok(ref view_model) => {
                    // Note that we don't set the account state for *this* request because we're
                    // just redirecting right away. If that ever changes, we should set account
                    // state.
                    middleware::web::authenticator::set_session_key(log, req, &view_model.key);

                    Ok(HttpResponse::build(StatusCode::TEMPORARY_REDIRECT)
                        .header("Location", "/account")
                        .finish())
                }
            }
        }
    }

    //
    // Private functions
    //

    fn message_invalid(account: Option<model::Account>, message: &str) -> Result<ViewModel> {
        Ok(ViewModel::Invalid(endpoints::signup_get::view_model::Ok {
            account: account,
            message: Some(message.to_owned()),
        }))
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use server::Params as P;
        use test_data;
        use test_helpers;
        use web::endpoints::signup_post::*;
        use web::endpoints::ViewModel as VM;

        use actix_web::test::TestRequest;
        use r2d2::PooledConnection;
        use r2d2_diesel::ConnectionManager;

        //
        // Params tests
        //

        #[test]
        fn test_signup_post_params() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();
            let params = Params::build(
                &bootstrap.log,
                &mut req,
                Some(b"email=foo@example.com&password=my-password&password_confirm=my-password"),
            ).unwrap();
            assert!(params.account.is_none());
            assert_eq!("foo@example.com", params.email);
            assert_eq!(test_helpers::REQUEST_IP, params.last_ip);
            assert_eq!("my-password", params.password);
            assert_eq!("my-password", params.password_confirm);
            assert_eq!(test_helpers::SCRYPT_LOG_N, params.scrypt_log_n);
        }

        //
        // Handler tests
        //

        #[test]
        fn test_signup_post_handler_ok() {
            let bootstrap = TestBootstrap::new();

            let view_model =
                handle_inner(&bootstrap.log, &*bootstrap.conn, valid_params()).unwrap();

            match view_model {
                ViewModel::Ok(_) => (),
                _ => panic!("Unexpected view model: {:?}", view_model),
            };
        }

        // Notably, we don't test *all* validations because most of them are already
        // tested in the mediator's suite.
        #[test]
        fn test_signup_post_handler_missing_password_confirm() {
            let bootstrap = TestBootstrap::new();

            let mut params = valid_params();
            params.password_confirm = "".to_owned();

            let view_model = handle_inner(&bootstrap.log, &*bootstrap.conn, params).unwrap();

            match view_model {
                ViewModel::Invalid(endpoints::signup_get::view_model::Ok {
                    account: _,
                    message: Some(message),
                }) => {
                    assert_eq!(
                        "Please type your password again in the confirmation box.",
                        message
                    );
                }
                _ => panic!("Unexpected view model: {:?}", view_model),
            };
        }

        #[test]
        fn test_signup_post_handler_mismatched_passwords() {
            let bootstrap = TestBootstrap::new();

            let mut params = valid_params();
            params.password_confirm = "not-my-password".to_owned();

            let view_model = handle_inner(&bootstrap.log, &*bootstrap.conn, params).unwrap();

            match view_model {
                ViewModel::Invalid(endpoints::signup_get::view_model::Ok {
                    account: _,
                    message: Some(message),
                }) => {
                    assert_eq!("Password and password confirmation didn't match.", message);
                }
                _ => panic!("Unexpected view model: {:?}", view_model),
            };
        }

        //
        // ViewModel tests
        //

        #[test]
        fn test_signup_post_view_model_render_invalid() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let view_model = ViewModel::Invalid(endpoints::signup_get::view_model::Ok {
                account: None,
                message: Some("Invalid action.".to_owned()),
            });
            let _response = view_model.render(&bootstrap.log, &mut req).unwrap();
        }

        #[test]
        fn test_signup_post_view_model_render_ok() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let account = test_data::account::insert(&bootstrap.log, &*bootstrap.conn);
            let key = test_data::key::insert_args(
                &bootstrap.log,
                &*bootstrap.conn,
                test_data::key::Args {
                    account:   Some(&account),
                    expire_at: None,
                },
            );

            let view_model = ViewModel::Ok(view_model::Ok { account, key });
            let response = view_model.render(&bootstrap.log, &mut req).unwrap();
            assert_eq!(StatusCode::TEMPORARY_REDIRECT, response.status());
            assert_eq!("/account", response.headers().get("Location").unwrap());
        }

        //
        // Private types/functions
        //

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

        fn valid_params() -> Params {
            Params {
                account:          None,
                email:            "foo@example.com".to_owned(),
                last_ip:          test_helpers::REQUEST_IP.to_owned(),
                password:         "my-password".to_owned(),
                password_confirm: "my-password".to_owned(),
                scrypt_log_n:     test_helpers::SCRYPT_LOG_N,
            }
        }
    }
}

pub mod signup_get {
    use errors::*;
    use server;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse};
    use futures::future::Future;
    use slog::Logger;

    handler_noop!();

    //
    // ViewModel
    //

    #[derive(Debug)]
    pub enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        #[derive(Debug)]
        pub struct Ok {
            pub account: Option<model::Account>,
            pub message: Option<String>,
        }
    }

    impl ViewModel {
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> ViewModel {
            ViewModel::Ok(view_model::Ok {
                account: server::account(req),
                message: None,
            })
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &mut HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common =
                        endpoints::build_common(req, view_model.account.as_ref(), "Signup");
                    endpoints::respond_200(views::signup_get::render(&common, view_model)?)
                }
            }
        }
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use test_helpers;
        use web::endpoints::signup_get::*;
        use web::endpoints::ViewModel as VM;

        use actix_web::test::TestRequest;

        //
        // ViewModel tests
        //

        #[test]
        fn test_signup_get_view_model_build() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();
            let view_model = ViewModel::build(&bootstrap.log, &mut req);

            match view_model {
                ViewModel::Ok(view_model::Ok {
                    account: None,
                    message: None,
                }) => (),
                _ => panic!("Unexpected view model: {:?}", view_model),
            };
        }

        #[test]
        fn test_signup_get_view_model_render_ok() {
            let bootstrap = TestBootstrap::new();
            let mut req =
                TestRequest::with_state(test_helpers::server_state(&bootstrap.log)).finish();

            let view_model = ViewModel::Ok(view_model::Ok {
                account: None,
                message: Some("Hello, world.".to_owned()),
            });
            let _response = view_model.render(&bootstrap.log, &mut req).unwrap();
        }

        //
        // Private types/functions
        //

        struct TestBootstrap {
            _common: test_helpers::CommonTestBootstrap,
            log:     Logger,
        }

        impl TestBootstrap {
            fn new() -> TestBootstrap {
                TestBootstrap {
                    _common: test_helpers::CommonTestBootstrap::new(),
                    log:     test_helpers::log(),
                }
            }
        }
    }
}
