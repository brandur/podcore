// Define an errors module and use a glob import as recommended by:
//
//     http://brson.github.io/2016/11/30/starting-with-error-chain
//

// Create the Error, ErrorKind, ResultExt, and Result types
error_chain!{
    // Automatic conversions between this error chain and other error types not defined by the
    // `error_chain!`. The description and cause will forward to the description and cause of the
    // original error.
    foreign_links {
        Database(::diesel::result::Error);
        HyperError(::hyper::Error);
        HyperUri(::hyper::error::UriError);
        Json(::serde_json::Error);
        UrlParse(::url::ParseError);
    }

    errors {
        SentryCredentialParseError {
            description("Invalid Sentry DSN syntax. Expected the form `(http|https)://{public key}:{private key}@{host}:{port}/{project id}`")
        }
    }
}
