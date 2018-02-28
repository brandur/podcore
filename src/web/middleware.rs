use time_helpers;
use web::common;

use actix_web;
use actix_web::{HttpRequest, HttpResponse};
use actix_web::middleware::{Response, Started};
use slog::Logger;

pub mod log_initializer {
    use web::middleware::*;

    pub struct Middleware;

    pub struct Log(pub Logger);

    impl<S: common::State> actix_web::middleware::Middleware<S> for Middleware {
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

    impl<S: common::State> actix_web::middleware::Middleware<S> for Middleware {
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

    impl<S: common::State> actix_web::middleware::Middleware<S> for Middleware {
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
            let elapsed = time::precise_time_ns() - req.extensions().get::<StartTime>().unwrap().0;
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
