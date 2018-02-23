use errors::*;
use http_requester::HTTPRequesterLive;
use mediators::directory_podcast_searcher::DirectoryPodcastSearcher;
use mediators::directory_podcast_updater::DirectoryPodcastUpdater;
use model;
use schema;
use time_helpers;

use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use diesel::prelude::*;
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

        let server = actix_web::HttpServer::new(move || {
            actix_web::Application::with_state(StateImpl {
                assets_version: assets_version.clone(),
                log:            log.clone(),
                pool:           pool.clone(),
            }).middleware(middleware::log_initializer::Middleware)
                .middleware(middleware::request_id::Middleware)
                .middleware(middleware::request_response_logger::Middleware)
                .resource("/directory-podcasts/{id}", |r| {
                    r.method(actix_web::Method::GET)
                        .f(handle_show_directory_podcast)
                })
                .resource("/health", |r| {
                    r.method(actix_web::Method::GET)
                        .f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/search", |r| {
                    r.method(actix_web::Method::GET).f(handle_show_search)
                })
                .resource("/search-home", |r| {
                    r.method(actix_web::Method::GET).f(handle_show_search_home)
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
// Private types
//

struct StateImpl {
    assets_version: String,
    log:            Logger,
    pool:           Pool<ConnectionManager<PgConnection>>,
}

impl middleware::State for StateImpl {
    fn log(&self) -> &Logger {
        &self.log
    }
}

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

    use actix_web;
    use actix_web::{HttpRequest, HttpResponse};
    use actix_web::middleware::{Response, Started};
    use slog::Logger;

    pub trait State {
        fn log(&self) -> &Logger;
    }

    pub mod log_initializer {
        use web::middleware::*;

        pub struct Middleware;

        pub struct Log(pub Logger);

        impl<S: State> actix_web::middleware::Middleware<S> for Middleware {
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

        impl<S: State> actix_web::middleware::Middleware<S> for Middleware {
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

        impl<S: State> actix_web::middleware::Middleware<S> for Middleware {
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

struct CommonViewModel {
    assets_version: String,
    title:          String,
}

struct ShowDirectoryPodcastViewModel {
    _common: CommonViewModel,

    dir_podcast_ex: Option<model::DirectoryPodcastException>,
    podcast:        Option<model::Podcast>,
}

struct ShowPodcastViewModel {
    common: CommonViewModel,

    episodes: Vec<model::Episode>,
    podcast:  model::Podcast,
}

struct ShowSearchHomeViewModel {
    common: CommonViewModel,
}

struct ShowSearchViewModel {
    common: CommonViewModel,

    directory_podcasts: Vec<model::DirectoryPodcast>,
    query:              String,
}

//
// Web handlers
//

fn handle_show_directory_podcast(
    mut req: HttpRequest<StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .clone();
    time_helpers::log_timed(&log.new(o!("step" => "execute")), |log| {
        handle_show_directory_podcast_inner(log, &req)
    })
}

fn handle_show_directory_podcast_inner(
    log: &Logger,
    req: &HttpRequest<StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let id = req.match_info()
        .get("id")
        .unwrap()
        .parse::<i64>()
        .chain_err(|| "Error parsing ID")?;
    info!(log, "Expanding directory podcast"; "id" => id);

    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
        .build(&core.handle());
    let mut http_requester = HTTPRequesterLive {
        client: client,
        core:   core,
    };

    let view_model: Option<ShowDirectoryPodcastViewModel> = time_helpers::log_timed(
        &log.new(o!("step" => "build_view_model")),
        |log| -> Result<Option<ShowDirectoryPodcastViewModel>> {
            let conn = req.state().pool.get()?;
            let dir_podcast: Option<model::DirectoryPodcast> = schema::directory_podcast::table
                .filter(schema::directory_podcast::id.eq(id))
                .first(&*conn)
                .optional()?;
            match dir_podcast {
                Some(mut dir_podcast) => {
                    let mut mediator = DirectoryPodcastUpdater {
                        conn:           &*conn,
                        dir_podcast:    &mut dir_podcast,
                        http_requester: &mut http_requester,
                    };
                    let res = mediator.run(log)?;

                    Ok(Some(ShowDirectoryPodcastViewModel {
                        _common: CommonViewModel {
                            assets_version: req.state().assets_version.clone(),
                            title:          "".to_owned(),
                        },

                        dir_podcast_ex: res.dir_podcast_ex,
                        podcast:        res.podcast,
                    }))
                }
                None => Ok(None),
            }
        },
    )?;

    if view_model.is_none() {
        return Ok(handle_404()?);
    }

    let view_model = view_model.unwrap();

    // TODO: This error should be more elaborate: recover more gracefully and show
    // more information.
    if let Some(_dir_podcast_ex) = view_model.dir_podcast_ex {
        return Err(Error::from("Couldn't expand directory podcast").into());
    }

    Ok(HttpResponse::build(StatusCode::PERMANENT_REDIRECT)
        .header(
            "Location",
            format!("/podcasts/{}", view_model.podcast.unwrap().id).as_str(),
        )
        .finish()?)
}

fn handle_show_search(mut req: HttpRequest<StateImpl>) -> actix_web::Result<HttpResponse> {
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
    req: &HttpRequest<StateImpl>,
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
    let mut http_requester = HTTPRequesterLive {
        client: client,
        core:   core,
    };

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
                common: CommonViewModel {
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

fn handle_show_search_home(mut req: HttpRequest<StateImpl>) -> actix_web::Result<HttpResponse> {
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .clone();
    time_helpers::log_timed(&log.new(o!("step" => "execute")), |log| {
        handle_show_search_home_inner(log, &req)
    })
}

fn handle_show_search_home_inner(
    log: &Logger,
    req: &HttpRequest<StateImpl>,
) -> actix_web::Result<HttpResponse> {
    let view_model = ShowSearchHomeViewModel {
        common: CommonViewModel {
            assets_version: req.state().assets_version.clone(),
            title:          "Search".to_owned(),
        },
    };

    let html = time_helpers::log_timed(&log.new(o!("step" => "render_view")), |_log| {
        render_show_search_home(&view_model)
    })?;

    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)?)
}

fn handle_show_podcast(mut req: HttpRequest<StateImpl>) -> actix_web::Result<HttpResponse> {
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
    req: &HttpRequest<StateImpl>,
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
                        common: CommonViewModel {
                            assets_version: req.state().assets_version.clone(),
                            title:          format!("Podcast: {}", podcast.title),
                        },

                        episodes: episodes,
                        podcast:  podcast,
                    }))
                }
                None => Ok(None),
            }
        },
    )?;

    if view_model.is_none() {
        return Ok(handle_404()?);
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

fn handle_404() -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::NOT_FOUND)
        .content_type("text/html; charset=utf-8")
        .body("404!")?)
}

//
// Views
//

fn render_layout(view_model: &CommonViewModel, content: &str) -> Result<String> {
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

fn render_show_search_home(view_model: &ShowSearchHomeViewModel) -> Result<String> {
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
