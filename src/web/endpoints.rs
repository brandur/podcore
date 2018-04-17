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
            use web;
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use actix_web::AsyncResponder;
            use futures::future;

            let log = middleware::log_initializer::log(&mut req);

            let params_res = time_helpers::log_timed(
                &log.new(o!("step" => "build_params")),
                |log| Params::build(log, &mut req),
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
                .sync_addr
                .send(message)
                .map_err(|_e| Error::from("Error from SyncExecutor"))
                .flatten()
                .and_then(move |view_model| {
                    time_helpers::log_timed(&log.new(o!("step" => "render_view_model")), |log| {
                        view_model.render(log, &req)
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

macro_rules! handler_post {
    () => {
        pub fn handler(
            mut req: HttpRequest<server::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            // Imported so that we can use the traits, but assigned a different name to
            // avoid clashing with the module's implementations.
            use server::Params as P;
            use web;
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use actix_web::AsyncResponder;
            use actix_web::HttpMessage;
            use bytes::Bytes;

            let log = middleware::log_initializer::log(&mut req);

            // We need `log` and `req` clones because we have multiple `move` closures below (and
            // only one can take the original log).
            let log2 = log.clone();
            let log3 = log.clone();
            let log4 = log.clone();
            let mut req2 = req.clone();
            let req3 = req.clone();
            let sync_addr = req.state().sync_addr.clone();

            req.body()
                // `map_err` is used here instead of `chain_err` because `PayloadError` doesn't
                // implement the `Error` trait and I was unable to put it in the error chain.
                .map_err(|_e| Error::from("Error reading request body"))
                .and_then(move |_bytes: Bytes| {
                    time_helpers::log_timed(&log.new(o!("step" => "build_params")), |log| {
                        //Params::build_from_post(log, &mut req_clone, bytes.as_ref())
                        Params::build(log, &mut req2)
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
                        view_model.render(log, &req3)
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

            let view_model = ViewModel::Ok(view_model::Ok {
                account: server::account(&mut req),
            });
            let response_res = time_helpers::log_timed(
                &log.new(o!("step" => "render_view_model")),
                |log| view_model.render(log, &req),
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
    fn render(&self, log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<HttpResponse>;
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

pub mod episode_show {
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

    // TODO: Change this back to normal handler.
    handler_post!();
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
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
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
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        &format!("Episode: {}", view_model.episode.title.as_str()),
                    );
                    endpoints::respond_200(views::episode_show::render(&common, view_model)?)
                }
            }
        }
    }
}

pub mod directory_podcast_show {
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
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
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
            _req: &HttpRequest<server::StateImpl>,
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

pub mod podcast_show {
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
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
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
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        &format!("Podcast: {}", view_model.podcast.title.as_str()),
                    );
                    endpoints::respond_200(views::podcast_show::render(&common, view_model)?)
                }
            }
        }
    }
}

pub mod search_show {
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
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
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
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        view_model.title.as_str(),
                    );
                    endpoints::respond_200(views::search_show::render(&common, view_model)?)
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

pub mod signup_post {
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

    /// Gets the value for the given parameter name or returns a "parameter
    /// missing" error.

    struct Params {
        account:          Option<model::Account>,
        email:            String,
        password:         String,
        password_confirm: String,
    }
    impl server::Params for Params {
        fn build<S: server::State>(_log: &Logger, req: &mut HttpRequest<S>) -> Result<Self> {
            Ok(Self {
                account:          server::account(req),
                email:            param_or_missing(req, "email")?,
                password:         param_or_missing(req, "password")?,
                password_confirm: param_or_missing(req, "password_confirm")?,
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
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        view_model.account.as_ref(),
                        view_model.title.as_str(),
                    );
                    endpoints::respond_200(views::search_show::render(&common, view_model)?)
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
*/

pub mod signup_show {
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

    pub enum ViewModel {
        Ok(view_model::Ok),
    }

    pub mod view_model {
        use model;

        pub struct Ok {
            pub account: Option<model::Account>,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Ok(ref view_model) => {
                    let common =
                        endpoints::build_common(req, view_model.account.as_ref(), "Signup");
                    endpoints::respond_200(views::signup_show::render(&common, self)?)
                }
            }
        }
    }
}
