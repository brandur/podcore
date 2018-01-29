use errors::*;

use futures::Stream;
use hyper;
use hyper::{Client, Uri};
use std::str::FromStr;
use std::sync::Arc;
use tokio_core::reactor::Core;

//
// URLFetcherFactory
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
        let client = Client::new(&core.handle());
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
// URLFetcher
//

pub trait URLFetcher {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)>;
}

#[derive(Debug)]
pub struct URLFetcherLive {
    pub client: Client<hyper::client::HttpConnector, hyper::Body>,
    pub core:   Core,
}

impl URLFetcher for URLFetcherLive {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)> {
        let feed_url = Uri::from_str(raw_url.as_str())
            .chain_err(|| format!("Error parsing feed URL: {}", raw_url))?;
        let res = self.core
            .run(self.client.get(feed_url))
            .chain_err(|| format!("Error fetching feed URL: {}", raw_url))?;

        // TODO: Follow redirects

        let body = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", raw_url))?;
        Ok(((*body).to_vec(), raw_url))
    }
}

#[derive(Clone, Debug)]
pub struct URLFetcherPassThrough {
    pub data: Arc<Vec<u8>>,
}

impl URLFetcher for URLFetcherPassThrough {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)> {
        return Ok(((*self.data).clone(), raw_url));
    }
}
