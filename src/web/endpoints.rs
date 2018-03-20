use errors::*;
use http_requester::HttpRequesterLive;
use server;
use web::views;

use actix_web::{HttpRequest, HttpResponse, StatusCode};
use hyper::Client;
use hyper_tls::HttpsConnector;
use slog::Logger;
use tokio_core::reactor::Core;

//
// Macros
//

/// Creates an asynchronous HTTP handler function suitable for use with Actix for the current
/// endpoint module.
///
/// The key point to understand here is that because we have a convention so that all
/// `server::Params` and `ViewModel`s are given the same name in every module, this can be pulled in
/// and expanded while still properly resolving symbols.
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
            mut req: HttpRequest<server::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            use time_helpers;
            // Imported so that we can use the traits, but assigned a different name to avoid
            // clashing with the module's implementations.
            use server::Params as P;
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

            let message = server::Message::new(&log, params);

            let sync_addr = req.state().sync_addr.as_ref().unwrap();

            sync_addr
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

/// Identical to `handler!` except useful in cases where the `server::SyncExecutor` doesn't need to
/// do any work. Skips sending a blocking message to `server::SyncExecutor` and getting a Postgres
/// connection from the pool to increase performance and avoid contention.
macro_rules! handler_noop {
    ($noop_response:path) => {
        pub fn handler(
            mut req: HttpRequest<server::StateImpl>,
        ) -> Box<Future<Item = HttpResponse, Error = Error>> {
            use time_helpers;
            // Imported so that we can use the traits, but assigned a different name to avoid
            // clashing with the module's implementations.
            use web::endpoints::ViewModel as VM;
            use web::middleware;

            use futures::future;

            let log = middleware::log_initializer::log(&mut req);

            let view_model = $noop_response;
            let response_res = time_helpers::log_timed(
                &log.new(o!("step" => "render_view_model")),
                |log| {
                    view_model.render(log, &req)
                }
            );
            let response = match response_res {
                Ok(response) => response,
                Err(e) => return Box::new(future::err(e)),
            };

            Box::new(future::ok(response))
        }
    }
}
/// Macro that easily creates the scaffolding necessary for a `server::SyncExecutor` message handler
/// from within an endpoint. It puts the necessary type definitions in place and creates a wrapper
/// function with access to a connection and log.
macro_rules! message_handler {
    () => {
        type MessageResult = ::actix::prelude::MessageResult<server::Message<Params>>;

        impl ::actix::prelude::Handler<server::Message<Params>> for server::SyncExecutor {
            type Result = MessageResult;

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

        // TODO: `ResponseType` will change to `Message`
        impl ::actix::prelude::ResponseType for server::Message<Params> {
            type Item = ViewModel;
            type Error = Error;
        }
    }
}

//
// Traits
//

/// A trait to be implemented by the view models that render views, which is also the same trait
/// for the typed responses that come from `server::SyncExecutor`. A view model is a model
/// containing all the information needed to build a view.
pub trait ViewModel {
    /// Renders a `ViewModel` implementation to an HTTP response. This could be a standard HTML
    /// page, but could also be any arbitrary response like a redirect.
    fn render(&self, log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<HttpResponse>;
}

//
// Structs
//

pub struct CommonViewModel {
    pub assets_version: String,
    pub title:          String,
}

//
// Functions
//

/// Builds a `CommonViewModel` from request information and takes in any other
/// required parameters to do so.
fn build_common(req: &HttpRequest<server::StateImpl>, title: &str) -> CommonViewModel {
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

/// Shortcut for a basic 200 response with standard HTML body content.
pub fn respond_200(body: String) -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(body)?)
}

//
// Endpoints
//

pub mod episode_show {
    use errors::*;
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
        id:         i64,
        podcast_id: i64,
    }

    impl server::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<Self> {
            Ok(Self {
                id:         req.match_info()
                    .get("id")
                    .unwrap()
                    .parse::<i64>()
                    .chain_err(|| "Error parsing episode ID")?,
                podcast_id: req.match_info()
                    .get("podcast_id")
                    .unwrap()
                    .parse::<i64>()
                    .chain_err(|| "Error parsing podcast ID")?,
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
            pub episode: model::Episode,
        }
    }

    impl endpoints::ViewModel for ViewModel {
        fn render(
            &self,
            _log: &Logger,
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Found(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        &format!("Episode: {}", view_model.episode.title.as_str()),
                    );
                    endpoints::respond_200(views::episode_show::render(&common, view_model)?)
                }
                ViewModel::NotFound => Ok(endpoints::handle_404()?),
            }
        }
    }

    //
    // Private functions
    //

    fn handle_inner(_log: &Logger, conn: &PgConnection, params: &Params) -> MessageResult {
        let episode: Option<model::Episode> = schema::episode::table
            .filter(schema::episode::id.eq(params.id))
            .filter(schema::episode::podcast_id.eq(params.podcast_id))
            .first(conn)
            .optional()?;
        match episode {
            Some(episode) => Ok(ViewModel::Found(view_model::Found { episode })),
            None => Ok(ViewModel::NotFound),
        }
    }
}

pub mod directory_podcast_show {
    use errors::*;
    use mediators::directory_podcast_updater;
    use model;
    use schema;
    use server;
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

    impl server::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<Self> {
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
            req: &HttpRequest<server::StateImpl>,
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
                let mut mediator = directory_podcast_updater::Mediator {
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
        id: i64,
    }

    impl server::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<Self> {
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
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            match *self {
                ViewModel::Found(ref view_model) => {
                    let common = endpoints::build_common(
                        req,
                        &format!("Podcast: {}", view_model.podcast.title.as_str()),
                    );
                    endpoints::respond_200(views::podcast_show::render(&common, view_model)?)
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

pub mod search_new_show {
    use errors::*;
    use server;
    use web::endpoints;
    use web::views;

    use actix_web::{HttpRequest, HttpResponse};
    use futures::future::Future;
    use slog::Logger;

    handler_noop!(ViewModel::Found);

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
            req: &HttpRequest<server::StateImpl>,
        ) -> Result<HttpResponse> {
            let common = endpoints::build_common(req, "Search");
            endpoints::respond_200(views::search_new_show::render(&common, self)?)
        }
    }
}

pub mod search_show {
    use errors::*;
    use mediators::directory_podcast_searcher;
    use server;
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
    impl server::Params for Params {
        fn build(_log: &Logger, req: &HttpRequest<server::StateImpl>) -> Result<Self> {
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
            req: &HttpRequest<server::StateImpl>,
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
                    endpoints::respond_200(views::search_show::render(&common, view_model)?)
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

        let res = directory_podcast_searcher::Mediator {
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
