use errors::*;
use web::views;

use actix_web::http::StatusCode;
use actix_web::HttpResponse;
use slog::Logger;

/// Renders an internal error to a human compatible HTML form.
///
/// This function is not allowed to throw an error because it also renders our
/// 500 status page. Use `unwrap`s and make sure that it works.
pub fn error_internal(log: &Logger, code: StatusCode, message: String) -> HttpResponse {
    // For the time being, we're just reusing the same view as the one for user
    // errors. We might want to change this at some point to hide the various
    // reasons for failure.
    error_user(log, code, message).unwrap()
}

/// Renders a user error to a human compatible HTML form.
pub fn error_user(log: &Logger, code: StatusCode, message: String) -> Result<HttpResponse> {
    error!(log, "Rendering error";
        "status" => format!("{}", code), "message" => message.as_str());
    let html = views::render_user_error(code, message)?;
    Ok(HttpResponse::build(code)
        .content_type("text/html; charset=utf-8")
        .body(html))
}
