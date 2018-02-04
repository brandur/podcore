use errors::*;

use futures::Stream;
use hyper::{Body, Client, Request, StatusCode};
use hyper::client::HttpConnector;
use hyper_tls::HttpsConnector;
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
// URLFetcherFactory trait + implementations
//

pub trait URLFetcherFactory: Send {
    // This is here because it's difficult to make a trait cloneable.
    fn clone_box(&self) -> Box<URLFetcherFactory>;

    fn create(&self) -> Box<URLFetcher>;
}

#[derive(Clone, Debug)]
pub struct URLFetcherFactoryLive {}

impl URLFetcherFactory for URLFetcherFactoryLive {
    fn clone_box(&self) -> Box<URLFetcherFactory> {
        return Box::new(Self {});
    }

    fn create(&self) -> Box<URLFetcher> {
        let core = Core::new().unwrap();
        let client = Client::configure()
            .connector(HttpsConnector::new(4, &core.handle()).unwrap())
            .build(&core.handle());
        Box::new(URLFetcherLive {
            client: client,
            core:   core,
        })
    }
}

#[derive(Clone, Debug)]
pub struct URLFetcherFactoryPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl URLFetcherFactory for URLFetcherFactoryPassThrough {
    fn clone_box(&self) -> Box<URLFetcherFactory> {
        return Box::new(Self {
            data: Arc::clone(&self.data),
        });
    }

    fn create(&self) -> Box<URLFetcher> {
        Box::new(URLFetcherPassThrough {
            data: Arc::clone(&self.data),
        })
    }
}

//
// URLFetcher trait + implementations
//

pub trait URLFetcher {
    // TODO: StatusCode should just be entire Response struct
    fn fetch(&mut self, req: Request) -> Result<(StatusCode, Vec<u8>, String)>;
}

#[derive(Debug)]
pub struct URLFetcherLive {
    pub client: Client<HttpsConnector<HttpConnector>, Body>,
    pub core:   Core,
}

impl URLFetcher for URLFetcherLive {
    fn fetch(&mut self, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
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
pub struct URLFetcherPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl URLFetcher for URLFetcherPassThrough {
    fn fetch(&mut self, req: Request) -> Result<(StatusCode, Vec<u8>, String)> {
        let uri = req.uri().to_string();
        return Ok((StatusCode::Ok, (*self.data).clone(), uri));
    }
}
