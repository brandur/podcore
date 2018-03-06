mod common;
mod endpoints;
mod middleware;

use errors::*;
use model;
use schema;
use time_helpers;

use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use diesel::prelude::*;
use horrorshow::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

pub struct WebServer {
    pub assets_version:     String,
    pub log:                Logger,
    pub num_sync_executors: usize,
    pub pool:               Pool<ConnectionManager<PgConnection>>,
    pub port:               String,
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
        let sync_addr =
            actix::SyncArbiter::start(self.num_sync_executors, move || endpoints::SyncExecutor {
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
                        .a(endpoints::directory_podcast_show::handler)
                })
                .resource("/health", |r| {
                    r.method(actix_web::Method::GET)
                        .f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/search", |r| {
                    r.method(actix_web::Method::GET)
                        .a(endpoints::search_show::handler)
                })
                .resource("/search/new", |r| {
                    r.method(actix_web::Method::GET)
                        .a(endpoints::search_home_show::handler)
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

//
// Web handlers
//

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

fn render_show_podcast(view_model: &ShowPodcastViewModel) -> Result<String> {
    endpoints::render_layout(
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
