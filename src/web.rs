use self::endpoints::ViewModel;
use errors::*;
use http_requester::HTTPRequesterLive;
use mediators::directory_podcast_searcher::DirectoryPodcastSearcher;
use model;
use schema;
use time_helpers;

use actix;
use actix_web;
use actix_web::{AsyncResponder, HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use diesel::prelude::*;
use futures::future;
use futures::future::Future;
use horrorshow::helper::doctype;
use horrorshow::prelude::*;
use hyper::Client;
use hyper_tls::HttpsConnector;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use tokio_core::reactor::Core;

pub struct WebServer {
    pub assets_version: String,
    pub log:            Logger,
    pub pool:           Pool<ConnectionManager<PgConnection>>,
    pub port:           String,
}

impl WebServer {
    pub fn run(&self) -> Result<()> {
        let assets_version = self.assets_version.clone();
        let log = self.log.clone();
        let pool = self.pool.clone();

        // Must appear up here because we're going to move `log` into server closure.
        let host = format!("0.0.0.0:{}", self.port.as_str());
        info!(log, "Web server starting"; "host" => host.as_str());

        // Although not referenced in the server definition, a `System` must be defined
        // or the server will crash on `start()`.
        let system = actix::System::new("podcore-web");

        // TODO: Get rid of this once StateImpl no longers takes a pool
        let pool_clone = pool.clone();
        let sync_addr = actix::SyncArbiter::start(3, move || endpoints::SyncExecutor {
            pool: pool_clone.clone(),
        });

        let server = actix_web::HttpServer::new(move || {
            actix_web::Application::with_state(endpoints::StateImpl {
                assets_version: assets_version.clone(),
                log:            log.clone(),
                pool:           pool.clone(),
                sync_addr:      sync_addr.clone(),
            }).middleware(middleware::log_initializer::Middleware)
                .middleware(middleware::request_id::Middleware)
                .middleware(middleware::request_response_logger::Middleware)
                .resource("/", |r| {
                    r.method(actix_web::Method::GET)
                        .f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/directory-podcasts/{id}", |r| {
                    r.method(actix_web::Method::GET)
                        .a(handle_show_directory_podcast)
                })
                .resource("/health", |r| {
                    r.method(actix_web::Method::GET)
                        .f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/search", |r| {
                    r.method(actix_web::Method::GET).f(handle_show_search)
                })
                .resource("/search/new", |r| {
                    r.method(actix_web::Method::GET).f(handle_show_search_new)
                })
                .resource("/podcasts/{id}", |r| {
                    r.method(actix_web::Method::GET).f(handle_show_podcast)
                })
                .handler(
                    format!("/assets/{}/", assets_version.as_str()).as_str(),
                    actix_web::fs::StaticFiles::new("./assets/", false),
                )
                .default_resource(|r| r.h(actix_web::NormalizePath::default()))
        });

        let _addr = server.bind(host)?.start();
        let _ = system.run();

        Ok(())
    }
}

//
// Common traits/types
//

mod common {
    use slog::Logger;

    pub trait State {
        fn log(&self) -> &Logger;
    }
}

//
// Error handling
//

impl From<Error> for actix_web::error::Error {
    fn from(error: Error) -> Self {
        actix_web::error::ErrorInternalServerError(error.to_string()).into()
    }
}

//
// Middleware
//

mod middleware {
    use time_helpers;
    use web::common;

    use actix_web;
    use actix_web::{HttpRequest, HttpResponse};
    use actix_web::middleware::{Response, Started};
    use slog::Logger;

    pub mod log_initializer {
        use web::middleware::*;

        pub struct Middleware;

        pub struct Log(pub Logger);

        impl<S: common::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                let log = req.state().log().clone();
                req.extensions().insert(Log(log));
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

    pub mod request_id {
        use web::middleware::*;

        use uuid::Uuid;

        pub struct Middleware;

        impl<S: common::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                let log = req.extensions().remove::<log_initializer::Log>().unwrap().0;

                let request_id = Uuid::new_v4().simple().to_string();
                debug!(&log, "Generated request ID"; "request_id" => request_id.as_str());

                req.extensions().insert(log_initializer::Log(log.new(
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
        use web::middleware::*;

        use time;

        pub struct Middleware;

        struct StartTime(u64);

        impl<S: common::State> actix_web::middleware::Middleware<S> for Middleware {
            fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
                req.extensions().insert(StartTime(time::precise_time_ns()));
                Ok(Started::Done)
            }

            fn response(
                &self,
                req: &mut HttpRequest<S>,
                resp: HttpResponse,
            ) -> actix_web::Result<Response> {
                let log = req.extensions()
                    .get::<log_initializer::Log>()
                    .unwrap()
                    .0
                    .clone();
                let elapsed =
                    time::precise_time_ns() - req.extensions().get::<StartTime>().unwrap().0;
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
}

//
// View models
//

struct ShowPodcastViewModel {
    common: endpoints::CommonViewModel,

    episodes: Vec<model::Episode>,
    podcast:  model::Podcast,
}

struct ShowSearchNewViewModel {
    common: endpoints::CommonViewModel,
}

struct ShowSearchViewModel {
    common: endpoints::CommonViewModel,

    directory_podcasts: Vec<model::DirectoryPodcast>,
    query:              String,
}

//
// SyncExecutor
//

mod endpoints {
    use errors::*;
    use web::common;

    use actix;
    use actix_web::{HttpRequest, HttpResponse, StatusCode};
    use diesel::pg::PgConnection;
    use r2d2::Pool;
    use r2d2_diesel::ConnectionManager;
    use slog::Logger;

    //
    // Traits
    //

    pub trait Params {}
    pub trait Response {}

    pub trait ViewModel {
        type Response: Response;
        type State: common::State;

        fn build(req: &HttpRequest<Self::State>, response: Self::Response) -> Self;
        fn render(&self, req: &HttpRequest<Self::State>) -> Result<HttpResponse>;
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

    pub struct StateImpl {
        pub assets_version: String,
        pub log:            Logger,
        pub pool:           Pool<ConnectionManager<PgConnection>>,
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
    // Error handlers
    //

    pub fn handle_404() -> Result<HttpResponse> {
        Ok(HttpResponse::build(StatusCode::NOT_FOUND)
            .content_type("text/html; charset=utf-8")
            .body("404!")?)
    }

    //
    // Endpoints
    //

    pub mod directory_podcast_show {
        use errors::*;
        use http_requester::HTTPRequesterLive;
        use mediators::directory_podcast_updater::DirectoryPodcastUpdater;
        use model;
        use schema;
        use web::endpoints;

        use actix;
        use actix_web::{HttpRequest, HttpResponse, StatusCode};
        use diesel::prelude::*;
        use hyper::Client;
        use hyper_tls::HttpsConnector;
        use tokio_core::reactor::Core;

        pub struct Params {
            pub id: i64,
        }

        impl endpoints::Params for Params {}

        // TODO: `ResponseType` will change to `Message`
        impl actix::prelude::ResponseType for endpoints::Message<Params> {
            type Item = Response;
            type Error = Error;
        }

        pub enum Response {
            Exception(model::DirectoryPodcastException),
            NotFound,
            Podcast(model::Podcast),
        }

        impl endpoints::Response for Response {}

        pub struct ViewModel {
            _common:  endpoints::CommonViewModel,
            response: Response,
        }

        impl endpoints::ViewModel for ViewModel {
            type Response = Response;
            type State = endpoints::StateImpl;

            fn build(req: &HttpRequest<Self::State>, response: Self::Response) -> Self {
                ViewModel {
                    _common:  endpoints::CommonViewModel {
                        assets_version: req.state().assets_version.clone(),
                        title:          "".to_owned(),
                    },
                    response: response,
                }
            }

            fn render(&self, _req: &HttpRequest<Self::State>) -> Result<HttpResponse> {
                match self.response {
                    Response::Exception(ref _dir_podcast_ex) => {
                        Err(Error::from("Couldn't expand directory podcast").into())
                    }
                    Response::NotFound => Ok(endpoints::handle_404()?),
                    Response::Podcast(ref podcast) => {
                        Ok(HttpResponse::build(StatusCode::PERMANENT_REDIRECT)
                            .header("Location", format!("/podcasts/{}", podcast.id).as_str())
                            .finish()?)
                    }
                }
            }
        }

        impl actix::prelude::Handler<endpoints::Message<Params>> for endpoints::SyncExecutor {
            type Result = actix::prelude::MessageResult<endpoints::Message<Params>>;

            fn handle(
                &mut self,
                message: endpoints::Message<Params>,
                _: &mut Self::Context,
            ) -> Self::Result {
                let conn = self.pool.get()?;
                let log = message.log;

                let core = Core::new().unwrap();
                let client = Client::configure()
                    .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
                    .build(&core.handle());
                let mut http_requester = HTTPRequesterLive { client, core };

                let dir_podcast: Option<model::DirectoryPodcast> = schema::directory_podcast::table
                    .filter(schema::directory_podcast::id.eq(message.params.id))
                    .first(&*conn)
                    .optional()?;
                match dir_podcast {
                    Some(mut dir_podcast) => {
                        let mut mediator = DirectoryPodcastUpdater {
                            conn:           &*conn,
                            dir_podcast:    &mut dir_podcast,
                            http_requester: &mut http_requester,
                        };
                        let res = mediator.run(&log)?;

                        if let Some(dir_podcast_ex) = res.dir_podcast_ex {
                            return Ok(Response::Exception(dir_podcast_ex));
                        }

                        return Ok(Response::Podcast(res.podcast.unwrap()));
                    }
                    None => Ok(Response::NotFound),
                }
            }
        }
    }
}

//
// ViewModel construction
//

fn build_show_directory_podcast_response(
    req: &HttpRequest<endpoints::StateImpl>,
    res: Result<endpoints::directory_podcast_show::Response>,
) -> Result<HttpResponse> {
    let response = res?;
    let view_model = endpoints::directory_podcast_show::ViewModel::build(req, response);
    view_model.render(req)
}

//
// Web handlers
//

fn handle_show_directory_podcast(
    mut req: HttpRequest<endpoints::StateImpl>,
) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .clone();

    let id = req.match_info()
        .get("id")
        .unwrap()
        .parse::<i64>()
        .chain_err(|| "Error parsing ID");

    if let Err(e) = id {
        return Box::new(future::err(e));
    }
    let id = id.unwrap();
    info!(log, "Expanding directory podcast"; "id" => id);

    let message = endpoints::Message {
        log:    log.clone(),
        params: endpoints::directory_podcast_show::Params { id: id },
    };
    req.state()
        .sync_addr
        .call_fut(message)
        .chain_err(|| "Error from SyncExecutor")
        .from_err()
        .and_then(move |res| build_show_directory_podcast_response(&req, res))
        .responder()
}

fn handle_show_search(
    mut req: HttpRequest<endpoints::StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .clone();
    time_helpers::log_timed(&log.new(o!("step" => "execute")), |log| {
        handle_show_search_inner(log, &req)
    })
}

fn handle_show_search_inner(
    log: &Logger,
    req: &HttpRequest<endpoints::StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let query = match req.query().get("q") {
        Some(q) => q,
        None => {
            return Ok(HttpResponse::build(StatusCode::TEMPORARY_REDIRECT)
                .header("Location", "/search-home")
                .finish()?);
        }
    };
    info!(log, "Searching directory podcasts"; "query" => query);

    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
        .build(&core.handle());
    let mut http_requester = HTTPRequesterLive { client, core };

    let view_model = time_helpers::log_timed(
        &log.new(o!("step" => "build_view_model")),
        |log| -> Result<ShowSearchViewModel> {
            let conn = req.state().pool.get()?;

            let res = DirectoryPodcastSearcher {
                conn:           &*conn,
                query:          query.to_owned(),
                http_requester: &mut http_requester,
            }.run(log)?;

            Ok(ShowSearchViewModel {
                common: endpoints::CommonViewModel {
                    assets_version: req.state().assets_version.clone(),
                    title:          format!("Search: {}", query),
                },

                directory_podcasts: res.directory_podcasts,
                query:              query.to_owned(),
            })
        },
    )?;

    let html = time_helpers::log_timed(&log.new(o!("step" => "render_view")), |_log| {
        render_show_search(&view_model)
    })?;

    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)?)
}

fn handle_show_search_new(
    mut req: HttpRequest<endpoints::StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .clone();
    time_helpers::log_timed(&log.new(o!("step" => "execute")), |log| {
        handle_show_search_new_inner(log, &req)
    })
}

fn handle_show_search_new_inner(
    log: &Logger,
    req: &HttpRequest<endpoints::StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let view_model = ShowSearchNewViewModel {
        common: endpoints::CommonViewModel {
            assets_version: req.state().assets_version.clone(),
            title:          "Search".to_owned(),
        },
    };

    let html = time_helpers::log_timed(&log.new(o!("step" => "render_view")), |_log| {
        render_show_search_new(&view_model)
    })?;

    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)?)
}

fn handle_show_podcast(
    mut req: HttpRequest<endpoints::StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .clone();
    time_helpers::log_timed(&log.new(o!("step" => "execute")), |log| {
        handle_show_podcast_inner(log, &req)
    })
}

fn handle_show_podcast_inner(
    log: &Logger,
    req: &HttpRequest<endpoints::StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let id = req.match_info()
        .get("id")
        .unwrap()
        .parse::<i64>()
        .chain_err(|| "Error parsing ID")?;
    info!(log, "Serving podcast"; "id" => id);

    let view_model: Option<ShowPodcastViewModel> = time_helpers::log_timed(
        &log.new(o!("step" => "build_view_model")),
        |_log| -> Result<Option<ShowPodcastViewModel>> {
            let conn = req.state().pool.get()?;
            let podcast: Option<model::Podcast> = schema::podcast::table
                .filter(schema::podcast::id.eq(id))
                .first(&*conn)
                .optional()?;
            match podcast {
                Some(podcast) => {
                    let episodes: Vec<model::Episode> = schema::episode::table
                        .filter(schema::episode::podcast_id.eq(podcast.id))
                        .order(schema::episode::published_at.desc())
                        .limit(50)
                        .load(&*conn)?;
                    Ok(Some(ShowPodcastViewModel {
                        common: endpoints::CommonViewModel {
                            assets_version: req.state().assets_version.clone(),
                            title:          format!("Podcast: {}", podcast.title),
                        },

                        episodes,
                        podcast,
                    }))
                }
                None => Ok(None),
            }
        },
    )?;

    if view_model.is_none() {
        return Ok(endpoints::handle_404()?);
    }

    let html = time_helpers::log_timed(&log.new(o!("step" => "render_view")), |_log| {
        render_show_podcast(&view_model.unwrap())
    })?;

    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)?)
}

//
// Error handlers
//

//
// Views
//

fn render_layout(view_model: &endpoints::CommonViewModel, content: &str) -> Result<String> {
    (html! {
        : doctype::HTML;
        html {
            head {
                title: view_model.title.as_str();

                meta(content="text/html; charset=utf-8", http-equiv="Content-Type");

                link(href=format_args!("/assets/{}/app.css", view_model.assets_version), media="screen", rel="stylesheet", type="text/css");
            }
            body {
                : Raw(content)
            }
        }
    }).into_string()
        .map_err(Error::from)
}

fn render_show_podcast(view_model: &ShowPodcastViewModel) -> Result<String> {
    render_layout(
        &view_model.common,
        (html! {
            h1: view_model.podcast.title.as_str();
            p {
                : "Hello! This is <html />"
            }
            ul {
                @ for episode in &view_model.episodes {
                    li: episode.title.as_str();
                }
            }
        }).into_string()?
            .as_str(),
    )
}

fn render_show_search(view_model: &ShowSearchViewModel) -> Result<String> {
    render_layout(
        &view_model.common,
        (html! {
            p {
                : format_args!("Query: {}", view_model.query);
            }
            ul {
                @ for dir_podcast in &view_model.directory_podcasts {
                    li {
                        @ if let Some(podcast_id) = dir_podcast.podcast_id {
                            a(href=format_args!("/podcasts/{}", podcast_id)) {
                                : dir_podcast.title.as_str()
                            }
                        } else {
                            a(href=format_args!("/directory-podcasts/{}", dir_podcast.id)) {
                                : dir_podcast.title.as_str()
                            }
                        }
                    }
                }
            }
        }).into_string()?
            .as_str(),
    )
}

fn render_show_search_new(view_model: &ShowSearchNewViewModel) -> Result<String> {
    render_layout(
        &view_model.common,
        (html! {
            h1: "Search";
            form(action="/search", method="get") {
                input(type="text", name="q");
                input(type="submit", value="Submit");
            }
        }).into_string()?
            .as_str(),
    )
}
