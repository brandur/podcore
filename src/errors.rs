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
        DatabaseConnectionPool(::r2d2::Error);
        HyperError(::hyper::Error);
        Io(::std::io::Error);
        NativeTls(::native_tls::Error);
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

// Collect error strings together so that we can build a good error message to
// send up. It's worth nothing that the original error is actually at the end of
// the iterator, but since it's the most relevant, we reverse the list.
//
// The chain isn't a double-ended iterator (meaning we can't use `rev`), so we
// have to collect it to a Vec first before reversing it.
//
// I've located this function here instead of error_helpers because it's needed
// by ErrorReporter. It's a bit of a breakage in modularity though, so it might
// be better just to duplicate the function in two places instead.
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
