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

pub struct Server {
    pub log:                Logger,
    pub num_sync_executors: u32,
    pub pool:               Pool<ConnectionManager<PgConnection>>,
    pub port:               String,
}

impl Server {
    pub fn run(&self) -> Result<()> {
        let log = self.log.clone();
        let pool = self.pool.clone();

        // Must appear up here because we're going to move `log` into server closure.
        let host = format!("0.0.0.0:{}", self.port.as_str());
        info!(log, "API server starting"; "host" => host.as_str());

        // Although not referenced in the server definition, a `System` must be defined
        // or the server will crash on `start()`.
        let system = actix::System::new("podcore-api");

        let sync_addr = actix::SyncArbiter::start(self.num_sync_executors as usize, move || {
            server::SyncExecutor { pool: pool.clone() }
        });

        let server = actix_web::server::new(move || {
            actix_web::App::with_state(server::StateImpl {
                assets_version: "".to_owned(),
                log:            log.clone(),
                sync_addr:      sync_addr.clone(),
            }).middleware(middleware::log_initializer::Middleware)
                .middleware(middleware::request_id::Middleware)
                .middleware(middleware::request_response_logger::Middleware)
                .resource("/", |r| r.method(Method::GET).f(|_req| HttpResponse::Ok()))
                .resource("/graphiql", |r| {
                    r.method(Method::GET).f(graphql::handlers::graphiql_get);
                })
                .resource("/graphql", |r| {
                    r.method(Method::GET).a(graphql::handlers::graphql_get);
                    r.method(Method::POST).a(graphql::handlers::graphql_post);
                })
                .resource("/health", |r| {
                    r.method(Method::GET).f(|_req| HttpResponse::Ok())
                })
                .default_resource(|r| r.h(actix_web::http::NormalizePath::default()))
        });

        let _addr = server.bind(host)?.start();
        let _ = system.run();

        Ok(())
    }
}
