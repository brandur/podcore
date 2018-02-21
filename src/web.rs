use errors::*;
use model;
use schema;
use time_helpers;

use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use actix_web::middleware::{Middleware, Response, Started};
use diesel::pg::PgConnection;
use diesel::prelude::*;
use horrorshow::helper::doctype;
use horrorshow::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use time;

pub struct WebServer {
    log:  Logger,
    pool: Pool<ConnectionManager<PgConnection>>,
    port: String,
}

impl WebServer {
    pub fn new(log: Logger, pool: Pool<ConnectionManager<PgConnection>>, port: &str) -> WebServer {
        WebServer {
            log:  log,
            pool: pool,
            port: port.to_owned(),
        }
    }

    pub fn run(&self) -> Result<()> {
        let log = self.log.clone();
        let pool = self.pool.clone();

        let host = format!("127.0.0.1:{}", self.port.as_str());
        info!(log, "Web server starting"; "host" => host.as_str());

        let system = actix::System::new("podcore-web");

        let server = actix_web::HttpServer::new(move || {
            actix_web::Application::with_state(StateImpl {
                log:  log.clone(),
                pool: pool.clone(),
            }).middleware(RequestResponseLogger)
                .resource("/{name}", |r| {
                    r.method(actix_web::Method::GET).f(handle_index)
                })
                .resource("/podcasts/{id}", |r| {
                    r.method(actix_web::Method::GET).f(handle_show_podcast)
                })
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

trait State {
    fn log(&self) -> &Logger;
}

struct StateImpl {
    log:  Logger,
    pool: Pool<ConnectionManager<PgConnection>>,
}

impl State for StateImpl {
    fn log(&self) -> &Logger {
        return &self.log;
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

struct RequestResponseLogger;

struct StartTime(u64);

impl<S: State> Middleware<S> for RequestResponseLogger {
    fn start(&self, req: &mut HttpRequest<S>) -> actix_web::Result<Started> {
        req.extensions().insert(StartTime(time::precise_time_ns()));
        Ok(Started::Done)
    }

    fn response(
        &self,
        req: &mut HttpRequest<S>,
        resp: HttpResponse,
    ) -> actix_web::Result<Response> {
        let elapsed = time::precise_time_ns() - req.extensions().get::<StartTime>().unwrap().0;
        info!(req.state().log(), "Request finished";
            "elapsed" => time_helpers::unit_str(elapsed),
            "method"  => req.method().as_str(),
            "path"    => req.path(),
            "status"  => resp.status().as_u16(),
        );
        Ok(Response::Done(resp))
    }
}

//
// View models
//

struct ShowPodcastViewModel {
    episodes: Vec<model::Episode>,
    podcast:  model::Podcast,
}

//
// Web handlers
//

fn handle_index(req: HttpRequest<StateImpl>) -> String {
    info!(req.state().log, "Serving hello");

    (html! {
        : doctype::HTML;
        html {
            head {
                title: "Hello world!";
            }
            body {
                p {
                    : "Hello! This is <html />"
                }
            }
        }
    }).into_string()
        .unwrap()
}

fn handle_show_podcast(req: HttpRequest<StateImpl>) -> actix_web::Result<HttpResponse> {
    let id = req.match_info()
        .get("id")
        .unwrap()
        .parse::<i64>()
        .chain_err(|| "Error parsing ID")?;
    info!(req.state().log, "Serving podcast"; "id" => id);

    let view_model: Option<ShowPodcastViewModel> = {
        let conn = req.state().pool.get().map_err(Error::from)?;
        let podcast: Option<model::Podcast> = schema::podcast::table
            .filter(schema::podcast::id.eq(id))
            .first(&*conn)
            .optional()
            .chain_err(|| "Error selecting podcast")?;
        match podcast {
            Some(podcast) => {
                let episodes: Vec<model::Episode> = schema::episode::table
                    .filter(schema::episode::podcast_id.eq(podcast.id))
                    .order(schema::episode::published_at.desc())
                    .limit(50)
                    .load(&*conn)
                    .chain_err(|| "Error selecting episodes")?;
                Some(ShowPodcastViewModel {
                    episodes: episodes,
                    podcast:  podcast,
                })
            }
            None => None,
        }
    };

    if view_model.is_none() {
        return Ok(handle_404()?);
    }

    let html = render_show_podcast(&view_model.unwrap()).map_err(Error::from)?;
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)
        .map_err(Error::from)?)
}

//
// Error handlers
//

fn handle_404() -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::NOT_FOUND)
        .content_type("text/html; charset=utf-8")
        .body("404!")
        .map_err(Error::from)?)
}

//
// Views
//

fn render_show_podcast(view_model: &ShowPodcastViewModel) -> Result<String> {
    (html! {
        : doctype::HTML;
        html {
            head {
                title: format_args!("Podcast: {}", view_model.podcast.title);
            }
            body {
                h1: view_model.podcast.title.as_str();
                p {
                    : "Hello! This is <html />"
                }
                ul {
                    @ for ref episode in view_model.episodes.iter() {
                        li: episode.title.as_str();
                    }
                }
            }
        }
    }).into_string()
        .map_err(Error::from)
}
