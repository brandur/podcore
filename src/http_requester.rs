use errors::*;

use futures::Stream;
use hyper::{Body, Client, Request, StatusCode};
use hyper::client::HttpConnector;
use hyper_tls::HttpsConnector;
use slog::Logger;
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

pub trait HTTPRequester {
    fn execute(&mut self, log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)>;
}

#[derive(Debug)]
pub struct HTTPRequesterLive {
    pub client: Client<HttpsConnector<HttpConnector>, Body>,
    pub core:   Core,
}

impl HTTPRequester for HTTPRequesterLive {
    fn execute(&mut self, log: &Logger, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
        info!(log, "Executing HTTP request";
            "method" => format!("{}", req.method()), "uri" => format!("{}", req.uri()));

        let uri = req.uri().to_string();

        let res = self.core
            .run(self.client.request(req))
            .chain_err(|| format!("Error fetching feed URL: {}", uri))?;
        let status = res.status();

        // TODO: Follow redirects

        let body = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", uri))?;
        Ok((status, (*body).to_vec(), uri))
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
