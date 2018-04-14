use errors::*;

use actix_web::HttpResponse;
use actix_web::http::StatusCode;
use serde_json;
use slog::Logger;

//
// Public functions
//

/// Renders an internal error to JSON form according to GraphQL specification.
///
/// This function is not allowed to throw an error because it also renders our
/// 500 status page. Use `unwrap`s and make sure that it works.
pub fn error_internal(log: &Logger, code: StatusCode, message: String) -> HttpResponse {
    // For the time being, we're just reusing the same view as the one for user
    // errors. We might want to change this at some point to hide the various
    // reasons for failure.
    error_user(log, code, message).unwrap()
}

/// Renders a user error to JSON form according to GraphQL specification.
pub fn error_user(log: &Logger, code: StatusCode, message: String) -> Result<HttpResponse> {
    error!(log, "Rendering error";
        "status" => format!("{}", code), "message" => message.as_str());
    let body = serde_json::to_string_pretty(&GraphQLErrors {
        errors: vec![GraphQLError { message }],
    })?;
    Ok(HttpResponse::build(code)
        .content_type("application/json; charset=utf-8")
        .body(body))
}

//
// Private types
//

/// A struct to serialize a set of `GraphQL` errors back to a client (errors
/// are always sent back as an array).
#[derive(Debug, Clone, Deserialize, Serialize)]
struct GraphQLErrors {
    errors: Vec<GraphQLError>,
}

/// A struct to serialize a `GraphQL` error back to the client. Should be
/// nested within `GraphQLErrors`.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct GraphQLError {
    message: String,
}
