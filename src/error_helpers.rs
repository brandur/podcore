use errors::*;
use mediators::error_reporter::{ErrorReporter, SentryCredentials};

use hyper::Client;
use hyper_tls::HttpsConnector;
use slog::Logger;
use std::env;
use tokio_core::reactor::Core;
use url_fetcher::URLFetcherLive;

// Prints an error to stderr.
pub fn print_error(log: &Logger, error: &Error) {
    let error_strings = error_strings(error);
    error!(log, "Error: {}", error_strings[0]);
    for s in error_strings.iter().skip(1) {
        error!(log, "Chained error: {}", s);
    }

    // The backtrace is not always generated. Programs must be run with
    // `RUST_BACKTRACE=1`.
    if let Some(backtrace) = error.backtrace() {
        error!(log, "{:?}", backtrace);
    }
}

// Reports an error to Sentry.
//
// This method is really slow because it assumes that reporting errors isn't
// going to need to be a hyper optimized operation (which is hopefully the
// case). It instantiates its own Tokio Core and a brand new client and then
// blocks until the error is reported. If at some point that errors need to be
// going up to Sentry constantly, this could be optimized without too much
// trouble.
pub fn report_error(log: &Logger, error: &Error) -> Result<()> {
    match env::var("SENTRY_URL") {
        Ok(url) => {
            info!(log, "Sending event to Sentry");

            let core = Core::new().unwrap();
            let client = Client::configure()
                .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
                .build(&core.handle());
            let creds = url.parse::<SentryCredentials>().unwrap();
            let mut url_fetcher = URLFetcherLive {
                client: client,
                core:   core,
            };

            let _res = ErrorReporter {
                creds:       &creds,
                error:       error,
                url_fetcher: &mut url_fetcher,
            }.run(log)?;
            Ok(())
        }
        Err(_) => Ok(()),
    }
}
