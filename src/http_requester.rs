use errors::*;

use futures::Stream;
use hyper::{Body, Client, Request, StatusCode, Uri};
use hyper::client::HttpConnector;
use hyper::header::{Location, UserAgent};
use hyper_tls::HttpsConnector;
use slog::Logger;
use std::str::FromStr;
use std::sync::Arc;
use tokio_core::reactor::Core;

pub enum Verb {
    DELETE,
    GET,
    PATCH,
    POST,
    PUT,
}

//
// HTTPRequesterFactory trait + implementations
//

pub trait HTTPRequesterFactory: Send {
    // This is here because it's difficult to make a trait cloneable.
    fn clone_box(&self) -> Box<HTTPRequesterFactory>;

    fn create(&self) -> Box<HTTPRequester>;
}

#[derive(Clone, Debug)]
pub struct HTTPRequesterFactoryLive {}

impl HTTPRequesterFactory for HTTPRequesterFactoryLive {
    fn clone_box(&self) -> Box<HTTPRequesterFactory> {
        Box::new(Self {})
    }

    fn create(&self) -> Box<HTTPRequester> {
        let core = Core::new().unwrap();
        let client = Client::configure()
            .connector(HttpsConnector::new(4, &core.handle()).unwrap())
            .build(&core.handle());
        Box::new(HTTPRequesterLive {
            client: client,
            core:   core,
        })
    }
}

#[derive(Clone, Debug)]
pub struct HTTPRequesterFactoryPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl HTTPRequesterFactory for HTTPRequesterFactoryPassThrough {
    fn clone_box(&self) -> Box<HTTPRequesterFactory> {
        Box::new(Self {
            data: Arc::clone(&self.data),
        })
    }

    fn create(&self) -> Box<HTTPRequester> {
        Box::new(HTTPRequesterPassThrough {
            data: Arc::clone(&self.data),
        })
    }
}

//
// HTTPRequester trait + implementations
//

// Maximum number of redirects that we'll follow.
const REDIRECT_LIMIT: i64 = 5;

pub trait HTTPRequester {
    fn execute(&mut self, log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)>;
}

#[derive(Debug)]
pub struct HTTPRequesterLive {
    pub client: Client<HttpsConnector<HttpConnector>, Body>,
    pub core:   Core,
}

impl HTTPRequesterLive {
    fn execute_inner(
        &mut self,
        log: &Logger,
        mut req: Request,
        redirect_depth: i64,
    ) -> Result<(StatusCode, Vec<u8>, String)> {
        if redirect_depth >= REDIRECT_LIMIT {
            return Err(Error::from("Hit HTTP redirect limit and not continuing"));
        }

        req.headers_mut()
            .set::<UserAgent>(UserAgent::new("Podcore/1.0".to_owned()));

        info!(log, "Executing HTTP request"; "redirect_depth" => redirect_depth,
            "method" => format!("{}", req.method()), "uri" => format!("{}", req.uri()));

        let method = req.method().clone();
        let uri = req.uri().to_string();

        let res = self.core
            .run(self.client.request(req))
            .chain_err(|| format!("Error fetching feed URL: {}", uri))?;
        let status = res.status();

        // Follow redirects.
        if status.is_redirection() {
            let new_uri = match res.headers().get::<Location>() {
                Some(uri) => Uri::from_str(uri).map_err(Error::from),
                None => Err(Error::from(
                    "Received redirection without `Location` header",
                )),
            }?;

            let new_req = Request::new(method, new_uri);
            let (status, body, last_uri) = self.execute_inner(log, new_req, redirect_depth + 1)?;

            // If we got a permanent redirect we return the final URI so that it can be
            // persisted for next time we need to make this request. Otherwise,
            // we return the original URI that came in with the request.
            let uri = if status == StatusCode::PermanentRedirect {
                last_uri
            } else {
                uri
            };

            return Ok((status, body, uri));
        }

        let body = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", uri))?;
        Ok((status, (*body).to_vec(), uri))
    }
}

impl HTTPRequester for HTTPRequesterLive {
    fn execute(&mut self, log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
        self.execute_inner(log, req, 0)
    }
}

#[derive(Clone, Debug)]
pub struct HTTPRequesterPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl HTTPRequester for HTTPRequesterPassThrough {
    fn execute(&mut self, _log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
        let uri = req.uri().to_string();
        Ok((StatusCode::Ok, (*self.data).clone(), uri))
    }
}
