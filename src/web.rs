use errors::*;
use model;
use schema;

use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use diesel::prelude::*;
use horrorshow::helper::doctype;
use horrorshow::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

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
        let host = format!("127.0.0.1:{}", self.port.as_str());
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
}

struct ShowPodcastViewModel {
    common: CommonViewModel,

    episodes: Vec<model::Episode>,
    podcast:  model::Podcast,
}

//
// Web handlers
//

fn handle_show_podcast(mut req: HttpRequest<StateImpl>) -> actix_web::Result<HttpResponse> {
    let id = req.match_info()
        .get("id")
        .unwrap()
        .parse::<i64>()
        .chain_err(|| "Error parsing ID")?;
    let log = req.extensions()
        .get::<middleware::log_initializer::Log>()
        .unwrap()
        .0
        .new(o!("step" => "execute"));
    info!(&log, "Serving podcast"; "id" => id);

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
                    common: CommonViewModel {
                        assets_version: req.state().assets_version.clone(),
                    },

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

                meta(content="text/html; charset=utf-8", http-equiv="Content-Type");

                link(href=format_args!("/assets/{}/app.css", view_model.common.assets_version), media="screen", rel="stylesheet", type="text/css");
            }
            body {
                h1: view_model.podcast.title.as_str();
                p {
                    : "Hello! This is <html />"
                }
                ul {
                    @ for episode in &view_model.episodes {
                        li: episode.title.as_str();
                    }
                }
            }
        }
    }).into_string()
        .map_err(Error::from)
}
