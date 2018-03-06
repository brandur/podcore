use errors::*;

use flate2::read::GzDecoder;
use futures::Stream;
use hyper::{Body, Client, Request, StatusCode, Uri};
use hyper::client::HttpConnector;
use hyper::header::{qitem, AcceptEncoding, ContentEncoding, Encoding, Location, UserAgent};
use hyper_tls::HttpsConnector;
use slog::Logger;
use std::io::prelude::*;
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
// HttpRequesterFactory trait + implementations
//

pub trait HttpRequesterFactory: Send {
    // This is here because it's difficult to make a trait cloneable.
    fn clone_box(&self) -> Box<HttpRequesterFactory>;

    fn create(&self) -> Box<HttpRequester>;
}

#[derive(Clone, Debug)]
pub struct HttpRequesterFactoryLive {}

impl HttpRequesterFactory for HttpRequesterFactoryLive {
    fn clone_box(&self) -> Box<HttpRequesterFactory> {
        Box::new(Self {})
    }

    fn create(&self) -> Box<HttpRequester> {
        let core = Core::new().unwrap();
        let client = Client::configure()
            .connector(HttpsConnector::new(4, &core.handle()).unwrap())
            .build(&core.handle());
        Box::new(HttpRequesterLive { client, core })
    }
}

#[derive(Clone, Debug)]
pub struct HttpRequesterFactoryPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl HttpRequesterFactory for HttpRequesterFactoryPassThrough {
    fn clone_box(&self) -> Box<HttpRequesterFactory> {
        Box::new(Self {
            data: Arc::clone(&self.data),
        })
    }

    fn create(&self) -> Box<HttpRequester> {
        Box::new(HttpRequesterPassThrough {
            data: Arc::clone(&self.data),
        })
    }
}

//
// HttpRequester trait + implementations
//

// Maximum number of redirects that we'll follow.
const REDIRECT_LIMIT: i64 = 5;

pub trait HttpRequester {
    fn execute(&mut self, log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)>;
}

#[derive(Debug)]
pub struct HttpRequesterLive {
    pub client: Client<HttpsConnector<HttpConnector>, Body>,
    pub core:   Core,
}

impl HttpRequesterLive {
    fn execute_inner(
        &mut self,
        log: &Logger,
        mut req: Request,
        redirect_depth: i64,
    ) -> Result<(StatusCode, Vec<u8>, String)> {
        if redirect_depth >= REDIRECT_LIMIT {
            return Err(Error::from("Hit HTTP redirect limit and not continuing"));
        }

        {
            let headers = req.headers_mut();
            headers.set::<AcceptEncoding>(AcceptEncoding(vec![qitem(Encoding::Gzip)]));
            headers.set::<UserAgent>(UserAgent::new("Podcore/1.0".to_owned()));
        }

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

        let gzipped = match res.headers().get::<ContentEncoding>() {
            Some(e) => e.contains(&Encoding::Gzip),
            None => false,
        };

        let body_chunk = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", uri))?;

        let mut body = (*body_chunk).to_vec();
        if gzipped {
            info!(log, "Decoding gzip-encoded body"; "body_length" => body.len());
            let mut body_decoded: Vec<u8> = Vec::new();
            {
                let mut decoder = GzDecoder::new(body.as_slice());
                decoder.read_to_end(&mut body_decoded)?;
            }
            body = body_decoded;
        }

        Ok((status, body, uri))
    }
}

impl HttpRequester for HttpRequesterLive {
    fn execute(&mut self, log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
        self.execute_inner(log, req, 0)
    }
}

#[derive(Clone, Debug)]
pub struct HttpRequesterPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl HttpRequester for HttpRequesterPassThrough {
    fn execute(&mut self, _log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
        let uri = req.uri().to_string();
        Ok((StatusCode::Ok, (*self.data).clone(), uri))
    }
}
