mod endpoints;
pub mod errors;
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

/// Contains names for resource URLs so that we don't have to stringly type them
/// to the same extent.
///
/// Please always use this for resource URL names, and for resource URL names
/// *only*.
pub mod names {
    pub static SIGNUP: &str = "signup";
}

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
    pub scrypt_log_n:       u8,
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
        let scrypt_log_n = self.scrypt_log_n;

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
                scrypt_log_n:   scrypt_log_n,
                sync_addr:      Some(sync_addr.clone()),
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
                        .a(endpoints::directory_podcast_get::handler)
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
                .resource("/login", |r| {
                    r.name(names::SIGNUP);
                    r.method(Method::GET).a(endpoints::login_get::handler);
                })
                .resource("/search", |r| {
                    r.method(Method::GET).a(endpoints::search_get::handler)
                })
                .resource("/signup", |r| {
                    r.name(names::SIGNUP);
                    r.method(Method::GET).a(endpoints::signup_get::handler);
                    r.method(Method::POST).a(endpoints::signup_post::handler);
                })
                .resource("/podcasts/{id}", |r| {
                    r.method(Method::GET).a(endpoints::podcast_get::handler)
                })
                .resource("/podcasts/{podcast_id}/episodes/{id}", |r| {
                    r.method(Method::GET).a(endpoints::episode_get::handler)
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
