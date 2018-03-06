use errors::*;
use http_requester::HttpRequesterLive;
use web::common;
use web::views;

use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use hyper::Client;
use hyper_tls::HttpsConnector;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use tokio_core::reactor::Core;

//
// Macros
//

/// Creates an asynchronous HTTP handler function suitable for use with Actix for the current
/// endpoint module.
///
/// The key point to understand here is that because we have a convention so that all `Params` and
/// `ViewModel`s are given the same name in every module, this can be pulled in and expanded while
/// still properly resolving symbols.
///
/// Honestly, I would've preferred not to have to sink into a macro to get this working, but I
/// started running into some serious typing problems when trying to make this a generic function.
/// Be it with generics or associated types I'd always get a complaint from the compiler that there
/// was no implementation for the generic version when sending a message to Actix (and in a few
/// other places). After trying many different approaches and failing on all of them, I eventually
/// just resorted to this. To keep things clean, offload as much work as possible to functions
/// outside of the macro. Try to change this as little as possible.
macro_rules! handler {
    () => (
        pub fn handler(
            mut req: HttpRequest<endpoints::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            use time_helpers;
            use web::endpoints;
            // Imported so that we can use the traits, but assigned a different name to avoid
            // clashing with the module's implementations.
            use web::endpoints::Params as P;
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use actix_web::AsyncResponder;
            use futures::future;

            let log = middleware::log_initializer::log(&mut req);

            let params_res = time_helpers::log_timed(&log.new(o!("step" => "build_params")), |log| {
                Params::build(log, &req)
            });
            let params = match params_res {
                Ok(params) => params,
                Err(e) => return Box::new(future::err(e)),
            };

            let message = endpoints::Message::new(&log, params);

            req.state()
                .sync_addr
                .call_fut(message)
                .chain_err(|| "Error from SyncExecutor")
                .from_err()
                .and_then(move |res| {
                    let view_model = res?;
                    time_helpers::log_timed(&log.new(o!("step" => "render_view_model")), |log| {
                        view_model.render(log, &req)
                    })
                })
                .responder()
        }
    )
}

/// Macro that easily creates the scaffolding necessary for a `SyncExecutor` message handler from
/// within an endpoint. It puts the necessary type definitions in place and creates a wrapper
/// function with access to a connection and log.
macro_rules! message_handler {
    () => {
        type MessageResult = ::actix::prelude::MessageResult<endpoints::Message<Params>>;

        impl ::actix::prelude::Handler<endpoints::Message<Params>> for endpoints::SyncExecutor {
            type Result = MessageResult;

            fn handle(
                &mut self,
                message: endpoints::Message<Params>,
                _: &mut Self::Context,
            ) -> Self::Result {
                let conn = self.pool.get()?;
                let log = message.log.clone();
                time_helpers::log_timed(&log.new(o!("step" => "handle_message")), |log| {
                    handle_inner(log, &*conn, &message.params)
                })
            }
        }

        // TODO: `ResponseType` will change to `Message`
        impl ::actix::prelude::ResponseType for endpoints::Message<Params> {
            type Item = ViewModel;
            type Error = Error;
        }
    }
}

/// Identical to `message_handler!` except useful in cases where the `SyncExecutor` doesn't need to
/// do any work. Skips getting a connection from the pool to minimize its contention.
macro_rules! message_handler_noop {
    ($noop_response:path) => {
        type MessageResult = ::actix::prelude::MessageResult<endpoints::Message<Params>>;

        impl ::actix::prelude::Handler<endpoints::Message<Params>> for endpoints::SyncExecutor {
            type Result = MessageResult;

            fn handle(
                &mut self,
                message: endpoints::Message<Params>,
                _: &mut Self::Context,
            ) -> Self::Result {
                let log = message.log.clone();
                time_helpers::log_timed(&log.new(o!("step" => "handle_message")), |_log| {
                    Ok($noop_response)
                })
            }
        }

        // TODO: `ResponseType` will change to `Message`
        impl ::actix::prelude::ResponseType for endpoints::Message<Params> {
            type Item = ViewModel;
            type Error = Error;
        }
    }
}

//
// Traits
//

/// A trait to be implemented for parameters that are decoded from an incoming HTTP request. It's
/// also reused as a message to be received by `SyncExecutor` containing enough information to run
/// its synchronous database operations.
pub trait Params: Sized {
    /// Builds a `Params` implementation by decoding an HTTP request. This may result in an error
    /// if appropriate parameters were not found or not valid.
    fn build(log: &Logger, req: &HttpRequest<StateImpl>) -> Result<Self>;
}

/// A trait to be implemented by the view models that render views, which is also the same trait
/// for the typed responses that come from `SyncExecutor`. A view model is a model containing all
/// the information needed to build a view.
pub trait ViewModel {
    /// Renders a `ViewModel` implementation to an HTTP response. This could be a standard HTML
    /// page, but could also be any arbitrary response like a redirect.
    fn render(&self, log: &Logger, req: &HttpRequest<StateImpl>) -> Result<HttpResponse>;
}

//
// Structs
//

pub struct CommonViewModel {
    pub assets_version: String,
    pub title:          String,
}

pub struct Message<P: Params> {
    pub log:    Logger,
    pub params: P,
}

impl<P: Params> Message<P> {
    fn new(log: &Logger, params: P) -> Message<P> {
        Message {
            log: log.clone(),
            params,
        }
    }
}

pub struct StateImpl {
    pub assets_version: String,
    pub log:            Logger,
    pub sync_addr:      actix::prelude::SyncAddress<SyncExecutor>,
}

impl common::State for StateImpl {
    fn log(&self) -> &Logger {
        &self.log
    }
}

pub struct SyncExecutor {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl actix::Actor for SyncExecutor {
    type Context = actix::SyncContext<Self>;
}

//
// Functions
//

impl From<Error> for actix_web::error::Error {
    fn from(error: Error) -> Self {
        actix_web::error::ErrorInternalServerError(error.to_string()).into()
    }
}

/// Builds a `CommonViewModel` from request information and takes in any other
/// required parameters to do so.
fn build_common(req: &HttpRequest<StateImpl>, title: &str) -> CommonViewModel {
    CommonViewModel {
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

pub fn handle_404() -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::NOT_FOUND)
        .content_type("text/html; charset=utf-8")
        .body("404!")?)
}

pub fn handle_500(view_model: &CommonViewModel, error: &str) -> Result<HttpResponse> {
    let html = views::render_500(view_model, error)?;
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)?)
}

//
// Endpoints
//

pub mod directory_podcast_show {
    use errors::*;
    use mediators::directory_podcast_updater::DirectoryPodcastUpdater;
    use model;
    use schema;
    use time_helpers;
    use web::endpoints;

    use actix_web::{HttpRequest, HttpResponse, StatusCode};
    use diesel::prelude::*;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        id: i64,
    }

    impl endpoints::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<endpoints::StateImpl>) -> Result<Self> {
            Ok(Self {
                id: req.match_info()
                    .get("id")
                    .unwrap()
                    .parse::<i64>()
                    .chain_err(|| "Error parsing ID")?,
            })
        }
    }

    //
    // ViewModel
    //

    pub enum ViewModel {
        Exception(model::DirectoryPodcastException),
        Found(model::Podcast),
        NotFound,
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &HttpRequest<endpoints::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Exception(ref _exception) => Ok(endpoints::handle_500(
                    &endpoints::build_common(req, "Error"),
                    "Error ingesting podcast",
                )?),
                ViewModel::Found(ref view_model) => {
                    Ok(HttpResponse::build(StatusCode::PERMANENT_REDIRECT)
                        .header("Location", format!("/podcasts/{}", view_model.id).as_str())
                        .finish()?)
                }
                ViewModel::NotFound => Ok(endpoints::handle_404()?),
            }
        }
    }

    //
    // Private functions
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: &Params) -> MessageResult {
        info!(log, "Expanding directory podcast"; "id" => params.id);

        let dir_podcast: Option<model::DirectoryPodcast> = schema::directory_podcast::table
            .filter(schema::directory_podcast::id.eq(params.id))
            .first(conn)
            .optional()?;
        match dir_podcast {
            Some(mut dir_podcast) => {
                let mut mediator = DirectoryPodcastUpdater {
                    conn,
                    dir_podcast: &mut dir_podcast,
                    http_requester: &mut endpoints::build_requester()?,
                };
                let res = mediator.run(log)?;

                if let Some(dir_podcast_ex) = res.dir_podcast_ex {
                    return Ok(ViewModel::Exception(dir_podcast_ex));
                }

                Ok(ViewModel::Found(res.podcast.unwrap()))
            }
            None => Ok(ViewModel::NotFound),
        }
    }
}

pub mod podcast_show {
    use errors::*;
    use model;
    use schema;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse, StatusCode};
    use diesel::prelude::*;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        id: i64,
    }

    impl endpoints::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<endpoints::StateImpl>) -> Result<Self> {
            Ok(Self {
                id: req.match_info()
                    .get("id")
                    .unwrap()
                    .parse::<i64>()
                    .chain_err(|| "Error parsing ID")?,
            })
        }
    }

    //
    // ViewModel
    //

    pub enum ViewModel {
        Found(view_model::Found),
        NotFound,
    }

    pub mod view_model {
        use model;

        pub struct Found {
            pub episodes: Vec<model::Episode>,
            pub podcast:  model::Podcast,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &HttpRequest<endpoints::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Found(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        &format!("Podcast: {}", view_model.podcast.title.as_str()),
                    );
                    let html = views::podcast_show::render(&common, view_model)?;
                    Ok(HttpResponse::build(StatusCode::OK)
                        .content_type("text/html; charset=utf-8")
                        .body(html)?)
                }
                ViewModel::NotFound => Ok(endpoints::handle_404()?),
            }
        }
    }

    //
    // Private functions
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: &Params) -> MessageResult {
        info!(log, "Looking up podcast"; "id" => params.id);
        let podcast: Option<model::Podcast> = schema::podcast::table
            .filter(schema::podcast::id.eq(params.id))
            .first(&*conn)
            .optional()?;
        match podcast {
            Some(podcast) => {
                let episodes: Vec<model::Episode> = schema::episode::table
                    .filter(schema::episode::podcast_id.eq(podcast.id))
                    .order(schema::episode::published_at.desc())
                    .limit(50)
                    .load(&*conn)?;
                Ok(ViewModel::Found(view_model::Found { episodes, podcast }))
            }
            None => Ok(ViewModel::NotFound),
        }
    }
}

pub mod search_home_show {
    use errors::*;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse, StatusCode};
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler_noop!(ViewModel::Found);

    //
    // Params
    //

    struct Params {}
    impl endpoints::Params for Params {
        fn build(_log: &Logger, _req: &HttpRequest<endpoints::StateImpl>) -> Result<Self> {
            Ok(Self {})
        }
    }

    //
    // ViewModel
    //

    pub enum ViewModel {
        Found,
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &HttpRequest<endpoints::StateImpl>,
        ) -> Result<HttpResponse> {
            let common = endpoints::build_common(req, "Search");
            let html = views::search_home_show::render(&common, self)?;
            Ok(HttpResponse::build(StatusCode::OK)
                .content_type("text/html; charset=utf-8")
                .body(html)?)
        }
    }
}

pub mod search_show {
    use errors::*;
    use mediators::directory_podcast_searcher::DirectoryPodcastSearcher;
    use time_helpers;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse, StatusCode};
    use diesel::pg::PgConnection;
    use futures::future::Future;
    use slog::Logger;

    handler!();
    message_handler!();

    //
    // Params
    //

    struct Params {
        query: Option<String>,
    }
    impl endpoints::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<endpoints::StateImpl>) -> Result<Self> {
            Ok(Self {
                query: req.query().get("q").map(|q| q.to_owned()),
            })
        }
    }

    //
    // ViewModel
    //

    enum ViewModel {
        NoQuery,
        SearchResults(view_model::SearchResults),
    }

    pub mod view_model {
        use model;

        pub struct SearchResults {
            pub directory_podcasts: Vec<model::DirectoryPodcast>,
            pub query:              String,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &HttpRequest<endpoints::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::NoQuery => Ok(HttpResponse::build(StatusCode::TEMPORARY_REDIRECT)
                    .header("Location", "/search-home")
                    .finish()?),
                ViewModel::SearchResults(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        &format!("Search: {}", view_model.query.as_str()),
                    );
                    let html = views::search_show::render(&common, view_model)?;
                    Ok(HttpResponse::build(StatusCode::OK)
                        .content_type("text/html; charset=utf-8")
                        .body(html)?)
                }
            }
        }
    }

    //
    // Private functions
    //

    fn handle_inner(log: &Logger, conn: &PgConnection, params: &Params) -> MessageResult {
        if params.query.is_none() {
            return Ok(ViewModel::NoQuery);
        }

        let query = params.query.clone().unwrap();
        if query.is_empty() {
            return Ok(ViewModel::NoQuery);
        }

        info!(log, "Executing query"; "id" => query.as_str());

        let res = DirectoryPodcastSearcher {
            conn:           &*conn,
            query:          query.to_owned(),
            http_requester: &mut endpoints::build_requester()?,
        }.run(log)?;

        Ok(ViewModel::SearchResults(view_model::SearchResults {
            directory_podcasts: res.directory_podcasts,
            query,
        }))
    }
}
