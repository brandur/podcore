use errors;
use errors::*;
use http_requester::HttpRequester;
use mediators::common;
use time_helpers;

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
    pub creds:          &'a SentryCredentials,
    pub error:          &'a Error,
    pub http_requester: &'a mut HttpRequester,
}

impl<'a> ErrorReporter<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let error_strings = errors::error_strings(self.error);
        let stack_trace = build_stack_trace(self.error);
        let event = build_event(&error_strings, stack_trace);
        info!(log, "Generated event"; "event_id" => event.event_id.as_str());

        let req = build_request(self.creds, &event)?;
        post_error(log, self.http_requester, req)?;

        Ok(RunResult {})
    }
}

pub struct RunResult {}

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
    fn uri(&self) -> &hyper::Uri {
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
    //
    // required
    //

    // uuid4 exactly 32 characters (no dashes)
    event_id: String,

    // Maximum length is 1000 characters.
    message: String,

    // ISO 8601 format, without a timezone ex: "2011-05-02T17:41:36"
    timestamp: String,

    // fatal, error, warning, info, debug
    level: String,

    // ex "my.logger.name"
    logger: String,

    // Acceptable values ..., other
    platform: String,

    sdk: SDK,

    //
    // optional
    //

    // the primary perpetrator of this event ex: "my.module.function_name"
    culprit: Option<String>,

    // host client from which the event was recorded
    server_name: Option<String>,

    #[serde(rename = "stacktrace")]
    stack_trace: Option<StackTrace>,

    // generally be something along the lines of the git SHA for the given project
    release: Option<String>,

    tags: HashMap<String, String>,

    // e.g., "production"
    environment: Option<String>,

    modules: HashMap<String, String>,

    extra: HashMap<String, String>,

    // an array of strings used to dictate the deduplicating for this event.
    fingerprint: Vec<String>,
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

fn build_event(error_strings: &[String], stack_trace: Option<StackTrace>) -> Event {
    Event {
        // required
        event_id: Uuid::new_v4().simple().to_string(), // `simple` gets an unhyphenated UUID
        message: error_strings.join("\n"),
        timestamp: Utc::now().to_rfc3339(),
        level: "fatal".to_owned(),
        logger: "panic".to_owned(),
        platform: "other".to_owned(),
        sdk: SDK {
            name:    env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },

        // optional
        culprit: None,
        server_name: None, // TODO: Want server name
        stack_trace,
        release: None, // TODO: Want release,
        tags: Default::default(),
        environment: Some(env::var("PODCORE_ENV").unwrap_or_else(|_| "development".to_owned())),
        modules: Default::default(),
        extra: Default::default(),
        fingerprint: vec!["{{ default }}".to_owned()], // Maybe further customize the fingerprint
    }
}

fn build_request(creds: &SentryCredentials, event: &Event) -> Result<Request> {
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
    // Returns if None.
    error.backtrace()?;

    let backtrace = error.backtrace().unwrap();
    let mut frames = vec![];

    for frame in backtrace.frames() {
        // TODO: Is this right? Some frames can have no symbols. Try to check backtrace
        // implementation.
        for symbol in frame.symbols() {
            let function = symbol
                .name()
                .map_or("unresolved symbol".to_owned(), |name| name.to_string());
            let filename = symbol
                .filename()
                .map_or("".to_owned(), |sym| sym.to_string_lossy().into_owned());
            let lineno = symbol.lineno().unwrap_or(0);
            frames.push(StackFrame {
                filename,
                function,
                lineno,
            });
        }
    }
    Some(StackTrace { frames })
}

fn post_error(log: &Logger, http_requester: &mut HttpRequester, req: Request) -> Result<()> {
    let (status, body, _final_url) = time_helpers::log_timed(
        &log.new(o!("step" => "post_error")),
        |log| http_requester.execute(log, req),
    )?;
    common::log_body_sample(log, status, &body);
    ensure!(
        status == StatusCode::Ok,
        "Unexpected status while reporting error: {}",
        status
    );
    Ok(())
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use http_requester::HttpRequesterPassThrough;
    use mediators::error_reporter::*;
    use test_helpers;

    use std::sync::Arc;

    #[test]
    fn test_error_reporting() {
        let error = Error::from("Error triggered by user request")
            .chain_err(|| "Chained context 1")
            .chain_err(|| "Chained context 2");

        let mut bootstrap = TestBootstrap::new(error);
        let (mut mediator, log) = bootstrap.mediator();
        let _res = mediator.run(&log).unwrap();
    }

    //
    // Private types/functions
    //

    // Encapsulates the structures that are needed for tests to run. One should
    // only be obtained by invoking TestBootstrap::new().
    struct TestBootstrap {
        _common:        test_helpers::CommonTestBootstrap,
        creds:          SentryCredentials,
        error:          Error,
        log:            Logger,
        http_requester: HttpRequesterPassThrough,
    }

    impl TestBootstrap {
        fn new(error: Error) -> TestBootstrap {
            TestBootstrap {
                _common:        test_helpers::CommonTestBootstrap::new(),
                creds:          "https://user:pass@sentry.io/1"
                    .parse::<SentryCredentials>()
                    .unwrap(),
                error:          error,
                log:            test_helpers::log(),
                http_requester: HttpRequesterPassThrough {
                    data: Arc::new("{}".as_bytes().to_vec()),
                },
            }
        }

        fn mediator(&mut self) -> (ErrorReporter, Logger) {
            (
                ErrorReporter {
                    creds:          &self.creds,
                    error:          &self.error,
                    http_requester: &mut self.http_requester,
                },
                self.log.clone(),
            )
        }
    }
}
