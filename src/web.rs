use errors::*;

use actix;
use actix_web;
use diesel::pg::PgConnection;
use horrorshow::helper::doctype;
use horrorshow::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

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
            actix_web::Application::with_state(State {
                log:  log.clone(),
                pool: pool.clone(),
            }).middleware(actix_web::middleware::Logger::default())
                .resource("/{name}", |r| r.method(actix_web::Method::GET).f(index))
        });

        let _addr = server.bind(host)?.start();
        let _ = system.run();

        Ok(())
    }
}

//
// Private types
//

struct State {
    log:  Logger,
    pool: Pool<ConnectionManager<PgConnection>>,
}

//
// Web handlers
//

fn index(req: actix_web::HttpRequest<State>) -> String {
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
