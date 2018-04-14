mod endpoints;
mod views;

use errors::*;
use graphql;
use middleware;
use server;

use actix;
use actix_web;
use actix_web::HttpResponse;
use actix_web::http::Method;
use diesel::pg::PgConnection;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use time::Duration;

pub struct Server {
    pub assets_version: String,

    // A secret to secure cookies sent to clients. Must be at least length 32.
    pub cookie_secret: String,

    // Whether the cookie should be marked as secure. Remember that secure cookies are only
    // returned over encrypted connections, so this will cause problems if set in development and
    // the server is being used over http://.
    pub cookie_secure: bool,

    pub log:                Logger,
    pub num_sync_executors: u32,
    pub pool:               Pool<ConnectionManager<PgConnection>>,
    pub port:               String,
}

impl Server {
    pub fn run(&self) -> Result<()> {
        // Clone some values locally that are safe to `move` into the server closure
        // below.
        let assets_version = self.assets_version.clone();
        let cookie_secret = self.cookie_secret.clone();
        let cookie_secure = self.cookie_secure;
        let log = self.log.clone();
        let pool = self.pool.clone();

        // Must appear up here because we're going to move `log` into server closure.
        let host = format!("0.0.0.0:{}", self.port.as_str());
        info!(log, "Web server starting"; "host" => host.as_str());

        // Although not referenced in the server definition, a `System` must be defined
        // or the server will crash on `start()`.
        let system = actix::System::new("podcore-web");

        let sync_addr = actix::SyncArbiter::start(self.num_sync_executors as usize, move || {
            server::SyncExecutor { pool: pool.clone() }
        });

        let server = actix_web::server::new(move || {
            actix_web::App::with_state(server::StateImpl {
                assets_version: assets_version.clone(),
                log:            log.clone(),
                sync_addr:      sync_addr.clone(),
            }).middleware(actix_web::middleware::SessionStorage::new(
                actix_web::middleware::CookieSessionBackend::signed(cookie_secret.as_bytes())
                    .name("podcore-session")
                    // Podcasts aren't generally considered to be a super security-sensitive
                    // business (and cookies are secure), so set a lengthy maximum age.
                    .max_age(Duration::days(365))
                    .secure(cookie_secure),
            ))
                .middleware(middleware::log_initializer::Middleware)
                .middleware(middleware::request_id::Middleware)
                .middleware(middleware::request_response_logger::Middleware)
                .middleware(middleware::web::authenticator::Middleware)
                .resource("/", |r| r.method(Method::GET).f(|_req| HttpResponse::Ok()))
                .resource("/directory-podcasts/{id}", |r| {
                    r.method(Method::GET)
                        .a(endpoints::directory_podcast_show::handler)
                })
                .resource("/graphiql", |r| {
                    r.method(Method::GET).f(graphql::handlers::graphiql_get);
                })
                .resource("/graphql", |r| {
                    // We really don't want to use `GET` operations that are potentially mutations
                    // on the web because of the possibility that crawlers will follow them, so
                    // just mount the `POST` handler for GraphQL.
                    r.method(Method::POST).a(graphql::handlers::graphql_post);
                })
                .resource("/health", |r| {
                    r.method(Method::GET).f(|_req| HttpResponse::Ok())
                })
                .resource("/search", |r| {
                    r.method(Method::GET).a(endpoints::search_show::handler)
                })
                .resource("/search/new", |r| {
                    r.method(Method::GET).a(endpoints::search_new_show::handler)
                })
                .resource("/podcasts/{id}", |r| {
                    r.method(Method::GET).a(endpoints::podcast_show::handler)
                })
                .resource("/podcasts/{podcast_id}/episodes/{id}", |r| {
                    r.method(Method::GET).a(endpoints::episode_show::handler)
                })
                .handler(
                    format!("/assets/{}/", assets_version.as_str()).as_str(),
                    actix_web::fs::StaticFiles::new("./assets/"),
                )
                .default_resource(|r| r.h(actix_web::http::NormalizePath::default()))
        });

        let _addr = server.bind(host)?.start();
        let _ = system.run();

        Ok(())
    }
}
