// Define an errors module and use a glob import as recommended by:
//
//     http://brson.github.io/2016/11/30/starting-with-error-chain
//

use futures::Future;
use std;

// Create the Error, ErrorKind, ResultExt, and Result types
error_chain!{
    // Automatic conversions between this error chain and other error types not defined by the
    // `error_chain!`. The description and cause will forward to the description and cause of the
    // original error.
    foreign_links {
        Database(::diesel::result::Error);
        DatabaseConnectionPool(::r2d2::Error);
        Http(::http::Error);
        Hyper(::hyper::Error);
        Io(::std::io::Error);
        NativeTls(::native_tls::Error);
        HyperUri(::hyper::error::UriError);
        Json(::serde_json::Error);
        Template(::horrorshow::Error);
        UrlParse(::url::ParseError);
        Xml(::quick_xml::errors::Error);
    }

    errors {
        /// Occurs when encountering a job in the job queue which we don't know how to handle.
        ///
        /// This is often the result of a deployment mismatch. When new job classes are added, the
        /// job worker should ideally be updated first so that it can handle them before any are
        /// inserted into the queue. When job classes are removed, the worker should be updated
        /// last to given any existing jobs in the queue a chance to drain.
        JobUnknown(name: String) {
            description("Unknown job"),
            display("Unknown job: {}", name),
        }

        SentryCredentialParseError {
            description("Invalid Sentry DSN syntax. Expected the form `(http|https)://{public key}:{private key}@{host}:{port}/{project id}`")
        }
    }

    links {
        User(user_errors::Error, user_errors::ErrorKind);
    }
}

//
// Error functions
//

pub mod errors {
    use errors::*;

    #[inline]
    pub fn job_unknown<S: Into<String>>(name: S) -> Error {
        ErrorKind::JobUnknown(name.into()).into()
    }
}

// Collect error strings together so that we can build a good error message to
// send up. It's worth nothing that the original error is actually at the end of
// the iterator, but since it's the most relevant, we reverse the list.
//
// The chain isn't a double-ended iterator (meaning we can't use `rev`), so we
// have to collect it to a Vec first before reversing it.
//
// I've located this function here instead of error_helpers because it's needed
// by `error_reporter::Mediator`. It's a bit of a breakage in modularity
// though, so it might be better just to duplicate the function in two places
// instead.
pub fn error_strings(error: &Error) -> Vec<String> {
    error
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .iter()
        .cloned()
        .rev()
        .collect()
}

/// Gets an error message suitable for display by the user.
///
/// This function only returns `Some` if the given `Result` is an error and if
/// that error is a user error. Otherwise, it returns `None`.
pub fn user_error_message<T>(res: &Result<T>) -> Option<String> {
    if let &Err(Error(ErrorKind::User(ref error_kind), _)) = res {
        return Some(format!("{}", error_kind));
    }

    return None;
}

//
// User error chain
//

/// An error chain for user errors that should be transformed into something
/// user-facing before being sent back with a request.
pub mod user_errors {
    use errors;

    error_chain!{
        errors {
            BadParameter(parameter: String, detail: String) {
                description("Bad parameter"),
                display("Bad request: Error parsing parameter \"{}\": {}", parameter, detail),
            }

            BadRequest(message: String) {
                description("Bad request"),
                display("Bad request: {}", message),
            }

            MissingParameter(parameter: String) {
                description("Bad parameter"),
                display("Bad request: Missing parameter \"{}\"", parameter),
            }

            NotFound(resource: String, id: i64) {
                description("Not found"),
                display("Not found: resource \"{}\" with ID {} was not found.", resource, id),
            }

            // A more generalized "not found" that doesn't identify a specific resource.
            NotFoundGeneral(message: String) {
                description("Not found"),
                display("Not found: {}", message),
            }

            Unauthorized {
                description("Unauthorized"),
                display("Unauthorized: You need to present valid credentials to access this endpoint."),
            }

            Validation(message: String) {
                description("Validation error"),
                display("Validation failed: {}", message),
            }
        }
    }

    //
    // Public functions
    //

    #[inline]
    pub fn bad_parameter<E: ::std::error::Error, S: Into<String>>(name: S, e: &E) -> errors::Error {
        // `format!` invokes the error's `Display` trait implementation
        to_error(ErrorKind::BadParameter(
            name.into(),
            format!("{}", e).to_owned(),
        ))
    }

    #[inline]
    pub fn bad_request<S: Into<String>>(message: S) -> errors::Error {
        to_error(ErrorKind::BadRequest(message.into()))
    }

    #[inline]
    pub fn missing_parameter<S: Into<String>>(message: S) -> errors::Error {
        to_error(ErrorKind::MissingParameter(message.into()))
    }

    #[inline]
    pub fn not_found<S: Into<String>>(resource: S, id: i64) -> errors::Error {
        to_error(ErrorKind::NotFound(resource.into(), id))
    }

    #[inline]
    pub fn not_found_general<S: Into<String>>(message: S) -> errors::Error {
        to_error(ErrorKind::NotFoundGeneral(message.into()))
    }

    #[inline]
    pub fn unauthorized() -> errors::Error {
        to_error(ErrorKind::Unauthorized)
    }

    #[inline]
    pub fn validation<S: Into<String>>(message: S) -> errors::Error {
        to_error(ErrorKind::Validation(message.into()))
    }

    //
    // Private functions
    //

    #[inline]
    fn to_error(e: ErrorKind) -> errors::Error {
        let user_e: Error = e.into();
        user_e.into()
    }
}

pub trait FutureChainErr<T> {
    fn chain_err<F, E>(self, callback: F) -> Box<Future<Item = T, Error = Error>>
    where
        F: FnOnce() -> E + 'static,
        E: Into<ErrorKind>;
}

impl<F> FutureChainErr<F::Item> for F
where
    F: Future + 'static,
    F::Error: std::error::Error + Send + 'static,
{
    fn chain_err<C, E>(self, callback: C) -> Box<Future<Item = F::Item, Error = Error>>
    where
        C: FnOnce() -> E + 'static,
        E: Into<ErrorKind>,
    {
        Box::new(self.then(|r| r.chain_err(callback)))
    }
}
