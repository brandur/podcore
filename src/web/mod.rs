mod common;
mod endpoints;
mod middleware;

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
// ViewModel construction
//

fn build_show_directory_podcast_response(
    req: &HttpRequest<endpoints::StateImpl>,
    res: Result<endpoints::directory_podcast_show::ExecutorResponse>,
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
        params: endpoints::directory_podcast_show::Params { id },
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
