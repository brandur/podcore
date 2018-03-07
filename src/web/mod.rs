mod common;
mod endpoints;
mod middleware;
mod views;

use errors::*;

use actix;
use actix_web;
use actix_web::Method;
use diesel::pg::PgConnection;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

pub struct WebServer {
    pub assets_version:     String,
    pub log:                Logger,
    pub num_sync_executors: u32,
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
        let sync_addr = actix::SyncArbiter::start(self.num_sync_executors as usize, move || {
            endpoints::SyncExecutor {
                pool: pool_clone.clone(),
            }
        });

        let server = actix_web::HttpServer::new(move || {
            actix_web::Application::with_state(endpoints::StateImpl {
                assets_version: assets_version.clone(),
                log:            log.clone(),
                sync_addr:      sync_addr.clone(),
            }).middleware(middleware::log_initializer::Middleware)
                .middleware(middleware::request_id::Middleware)
                .middleware(middleware::request_response_logger::Middleware)
                .resource("/", |r| {
                    r.method(Method::GET).f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/directory-podcasts/{id}", |r| {
                    r.method(Method::GET)
                        .a(endpoints::directory_podcast_show::handler)
                })
                .resource("/health", |r| {
                    r.method(Method::GET).f(|_req| actix_web::httpcodes::HTTPOk)
                })
                .resource("/search", |r| {
                    r.method(Method::GET).a(endpoints::search_show::handler)
                })
                .resource("/search/new", |r| {
                    r.method(Method::GET)
                        .a(endpoints::search_home_show::handler)
                })
                .resource("/podcasts/{id}", |r| {
                    r.method(Method::GET).a(endpoints::podcast_show::handler)
                })
                .resource("/podcasts/{podcast_id}/episodes/{id}", |r| {
                    r.method(Method::GET).a(endpoints::episode_show::handler)
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
