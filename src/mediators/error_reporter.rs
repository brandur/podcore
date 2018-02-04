use errors::*;
use mediators::common;
use url_fetcher::URLFetcher;

use chrono::Utc;
use hyper;
use hyper::{Body, Method, Request, StatusCode, Uri};
use hyper::header::{ContentLength, ContentType};
use serde_json;
use slog::Logger;
use std;
use std::collections::HashMap;
use std::default::Default;
use url;

pub struct ErrorReporter<'a> {
    pub creds:       &'a SentryCredentials,
    pub error:       &'a Error,
    pub url_fetcher: &'a mut URLFetcher,
}

impl<'a> ErrorReporter<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<()> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.run_inner(&log)
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<()> {
        let event = Event {
            // required
            event_id:  "".to_owned(),
            message:   self.error.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            level:     "fatal".to_owned(),
            logger:    "panic".to_owned(), // TODO: What should my logger be?
            platform:  "other".to_owned(),
            sdk:       SDK {
                name:    "rust-sentry".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            device:    Device {
                name:    std::env::var_os("OSTYPE")
                    .and_then(|cs| cs.into_string().ok())
                    .unwrap_or("".to_owned()),
                version: "".to_owned(),
                build:   "".to_owned(),
            },

            // optional
            culprit:     None,
            server_name: None, // TODO: Want server name
            stack_trace: None, // TODO: Want stack trace
            release:     None, // TODO: Want release,
            tags:        Default::default(),
            environment: None, // TODO: Want environment,
            modules:     Default::default(),
            extra:       Default::default(),
            fingerprint: vec![], // TODO: Probably want fingerprint
        };

        let mut req = Request::new(Method::Post, self.creds.uri().clone());
        let body = serde_json::to_string(&event).map_err(Error::from)?;
        {
            let headers = req.headers_mut();

            // X-Sentry-Auth: Sentry sentry_version=7,
            // sentry_client=<client version, arbitrary>,
            // sentry_timestamp=<current timestamp>,
            // sentry_key=<public api key>,
            // sentry_secret=<secret api key>
            //
            let timestamp = Utc::now().timestamp();
            let sentry_auth = format!(
            "Sentry sentry_version=7,sentry_client=rust-sentry/{},sentry_timestamp={},sentry_key={},sentry_secret={}",
            env!("CARGO_PKG_VERSION"),
            timestamp,
            self.creds.key,
            self.creds.secret
        );
            headers.set(XSentryAuth(sentry_auth));
            headers.set(ContentType::json());
            headers.set(ContentLength(body.len() as u64));
        }
        req.set_body(Body::from(body));

        let (status, body, _final_url) = common::log_timed(
            &log.new(o!("step" => "post_error")),
            |ref _log| self.url_fetcher.fetch(req),
        )?;
        common::log_body_sample(log, status, &body);
        ensure!(
            status == StatusCode::Ok,
            "Unexpected status while reporting error: {}",
            status
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentryCredentials {
    scheme:     String,
    key:        String,
    secret:     String,
    host:       String,
    port:       u16,
    project_id: String,

    uri: Uri,
}

impl SentryCredentials {
    /// {SCHEME}://{PUBLIC_KEY}:{SECRET_KEY}@{HOST}/{PATH}{PROJECT_ID}/store/
    fn uri<'a>(&'a self) -> &'a hyper::Uri {
        &self.uri
    }
}

impl std::str::FromStr for SentryCredentials {
    type Err = Error;
    fn from_str(s: &str) -> std::result::Result<SentryCredentials, Error> {
        let url = url::Url::parse(s).map_err(Error::from)?;

        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            bail!(ErrorKind::SentryCredentialParseError);
        }

        let host = url.host_str().ok_or(ErrorKind::SentryCredentialParseError)?;

        let port = url.port()
            .unwrap_or_else(|| if scheme == "http" { 80 } else { 443 });

        let key = url.username();
        let secret = url.password().ok_or(ErrorKind::SentryCredentialParseError)?;

        let project_id = url.path_segments()
            .and_then(|paths| paths.last())
            .ok_or(ErrorKind::SentryCredentialParseError)?;

        if key.is_empty() || project_id.is_empty() {
            bail!(ErrorKind::SentryCredentialParseError);
        }

        let uri_str = format!(
            "{}://{}:{}@{}:{}/api/{}/store/",
            scheme, key, secret, host, port, project_id
        );
        let uri = uri_str.parse().map_err(Error::from)?;

        Ok(SentryCredentials {
            scheme: scheme.to_owned(),
            key: key.to_owned(),
            secret: secret.to_owned(),
            host: host.to_owned(),
            port,
            project_id: project_id.to_owned(),

            uri,
        })
    }
}

//
// Private types
//

header! { (XSentryAuth, "X-Sentry-Auth") => [String] }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    name:    String,
    version: String,
    build:   String,
}

// see https://docs.getsentry.com/hosted/clientdev/attributes/
#[derive(Debug, Clone, Serialize)]
struct Event {
    // required
    event_id:  String, // uuid4 exactly 32 characters (no dashes!)
    message:   String, // Maximum length is 1000 characters.
    timestamp: String, // ISO 8601 format, without a timezone ex: "2011-05-02T17:41:36"
    level:     String, // fatal, error, warning, info, debug
    logger:    String, // ex "my.logger.name"
    platform:  String, // Acceptable values ..., other
    sdk:       SDK,
    device:    Device,

    // optional
    culprit:     Option<String>, // the primary perpetrator of this event ex: "my.module.function_name"
    server_name: Option<String>, // host client from which the event was recorded
    stack_trace: Option<Vec<StackFrame>>, // stack trace
    release:     Option<String>, // generally be something along the lines of the git SHA for the given project
    tags:        HashMap<String, String>, // WARNING! should be serialized as json object k->v
    environment: Option<String>, // ex: "production"
    modules:     HashMap<String, String>, // WARNING! should be serialized as json object k->v
    extra:       HashMap<String, String>, // WARNING! should be serialized as json object k->v
    fingerprint: Vec<String>, // An array of strings used to dictate the deduplicating for this event.
}

#[derive(Debug, Clone, Serialize)]
pub struct SDK {
    name:    String,
    version: String,
}

#[derive(Debug, Clone, Serialize)]
struct StackFrame {
    filename: String,
    function: String,
    lineno:   u32,
}
