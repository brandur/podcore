use actix;
use actix_web;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use errors::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;
use std::result;

//
// Traits
//

/// A trait to be implemented for parameters that are decoded from an incoming
/// HTTP request. It's also reused as a message to be received by
/// `SyncExecutor` containing enough information to run its synchronous
/// database operations.
pub trait Params: Sized {
    /// Builds a `Params` implementation by decoding an HTTP request. This may
    /// result in an error if appropriate parameters were not found or not
    /// valid.
    fn build(log: &Logger, req: &HttpRequest<StateImpl>) -> Result<Self>;
}

pub trait State {
    fn log(&self) -> &Logger;
}

//
// Trait implementations
//

impl From<Error> for ::actix_web::error::Error {
    fn from(error: Error) -> Self {
        ::actix_web::error::ErrorInternalServerError(error.to_string())
    }
}

//
// Structs
//

pub struct Message<P: Params> {
    pub log:    Logger,
    pub params: P,
}

impl<P: Params> Message<P> {
    pub fn new(log: &Logger, params: P) -> Message<P> {
        Message {
            log: log.clone(),
            params,
        }
    }
}

pub struct StateImpl {
    // Assets are versioned so that they can be expired immediately without worrying about any kind
    // of client-side caching. This is a version represented as a string.
    //
    // Note that this is only used by `web::Server`.
    pub assets_version: String,

    pub log:       Logger,
    pub sync_addr: actix::prelude::Addr<actix::prelude::Syn, SyncExecutor>,
}

impl State for StateImpl {
    fn log(&self) -> &Logger {
        &self.log
    }
}

pub struct SyncExecutor {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl actix::Actor for SyncExecutor {
    type Context = actix::SyncContext<Self>;
}

//
// Functions
//

// Takes a `Result<HttpResponse, actix_web::Error`, which is the standard type
// seen when building an `HttpResponse`, and flattens it down to just an
// `HttpResponse` by converting the error to a proper response if necessary.
//
// This is a bit of a cludge because I don't know of a better way to accomplish
// this, but there probably is one.
pub fn flatten_http(res: result::Result<HttpResponse, actix_web::Error>) -> HttpResponse {
    match res {
        Ok(resp) => resp,
        Err(err_resp) => HttpResponse::from_error(err_resp),
    }
}

/// Handles a `Result` and renders an error that was intended for the user.
/// Otherwise (on either a successful result or non-user error), passes through
/// the normal result.
pub fn transform_user_error<F>(res: Result<HttpResponse>, render: F) -> Result<HttpResponse>
where
    F: FnOnce(StatusCode, String) -> Result<HttpResponse>,
{
    match res {
        Err(e @ Error(ErrorKind::BadRequest(_), _)) => {
            // `format!` activates the `Display` traits and shows our error's `display`
            // definition
            render(StatusCode::BAD_REQUEST, format!("{}", e))
        }
        r => r,
    }
}
