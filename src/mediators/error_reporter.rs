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
use std::env;
use url;
use uuid::Uuid;

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
        let error_strings = build_error_strings(&self.error);
        let stack_trace = build_stack_trace(&self.error);
        let event = build_event(error_strings, stack_trace);
        info!(log, "Generated event"; "event_id" => event.event_id.as_str());

        let req = build_request(&self.creds, event)?;
        post_error(log, self.url_fetcher, req)
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

    // optional
    culprit:     Option<String>, // the primary perpetrator of this event ex: "my.module.function_name"
    server_name: Option<String>, // host client from which the event was recorded

    #[serde(rename = "stacktrace")]
    stack_trace: Option<StackTrace>, // stack trace

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

#[derive(Debug, Clone, Serialize)]
struct StackTrace {
    frames: Vec<StackFrame>,
}

//
// Private functions
//

// Collect error strings together so that we can build a good error message to send up. It's worth
// nothing that the original error is actually at the end of the iterator, but since it's the most
// relevant, we reverse the list.
//
// The chain isn't a double-ended iterator (meaning we can't use `rev`), so we have to collect it
// to a Vec first before reversing it.
fn build_error_strings(error: &Error) -> Vec<String> {
    error
        .iter()
        .map(|ref e| e.to_string())
        .collect::<Vec<_>>()
        .iter()
        .cloned()
        .rev()
        .collect()
}

fn build_event(error_strings: Vec<String>, stack_trace: Option<StackTrace>) -> Event {
    Event {
        // required
        event_id:  Uuid::new_v4().simple().to_string(), // `simple` gets an unhyphenated UUID
        message:   error_strings.join("\n"),
        timestamp: Utc::now().to_rfc3339(),
        level:     "fatal".to_owned(),
        logger:    "panic".to_owned(),
        platform:  "other".to_owned(),
        sdk:       SDK {
            name:    env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },

        // optional
        culprit:     None,
        server_name: None, // TODO: Want server name
        stack_trace: stack_trace,
        release:     None, // TODO: Want release,
        tags:        Default::default(),
        environment: Some(env::var("PODCORE_ENV").unwrap_or("development".to_owned())),
        modules:     Default::default(),
        extra:       Default::default(),
        fingerprint: vec!["{{ default }}".to_owned()], // Maybe further customize the fingerprint
    }
}

fn build_request(creds: &SentryCredentials, event: Event) -> Result<Request> {
    let mut req = Request::new(Method::Post, creds.uri().clone());
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
            concat!(
                "Sentry sentry_version=7,sentry_client=rust-sentry/{},",
                "sentry_timestamp={},sentry_key={},sentry_secret={}"
            ),
            env!("CARGO_PKG_VERSION"),
            timestamp,
            creds.key,
            creds.secret
        );
        headers.set(XSentryAuth(sentry_auth));
        headers.set(ContentType::json());
        headers.set(ContentLength(body.len() as u64));
    }
    req.set_body(Body::from(body));
    Ok(req)
}

fn build_stack_trace(error: &Error) -> Option<StackTrace> {
    if error.backtrace().is_none() {
        return None;
    }

    let backtrace = error.backtrace().unwrap();
    let mut frames = vec![];

    for ref frame in backtrace.frames() {
        // TODO: Is this right? Some frames can have no symbols. Try to check backtrace
        // implementation.
        for ref symbol in frame.symbols() {
            let name = symbol
                .name()
                .map_or("unresolved symbol".to_owned(), |name| name.to_string());
            let filename = symbol
                .filename()
                .map_or("".to_owned(), |sym| sym.to_string_lossy().into_owned());
            let lineno = symbol.lineno().unwrap_or(0);
            frames.push(StackFrame {
                filename: filename,
                function: name,
                lineno:   lineno,
            });
        }
    }
    Some(StackTrace { frames: frames })
}

fn post_error(log: &Logger, url_fetcher: &mut URLFetcher, req: Request) -> Result<()> {
    let (status, body, _final_url) = common::log_timed(
        &log.new(o!("step" => "post_error")),
        |ref _log| url_fetcher.fetch(req),
    )?;
    common::log_body_sample(log, status, &body);
    ensure!(
        status == StatusCode::Ok,
        "Unexpected status while reporting error: {}",
        status
    );
    Ok(())
}
