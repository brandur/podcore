use errors::*;

use futures::Stream;
use hyper;
use hyper::{Client, Uri};
#[cfg(test)]
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio_core::reactor::Core;

pub trait URLFetcher: Send + URLFetcherClone {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)>;
}

// An insanely complicated workaround so as to allow us to produce a trait that can implement
// Clone:
//
//     https://stackoverflow.com/questions/30353462/how-to-clone-a-struct-storing-a-trait-object/30353928#30353928
//
pub trait URLFetcherClone {
    fn clone_box(&self) -> Box<URLFetcher>;
}

impl<T> URLFetcherClone for T
where
    T: 'static + URLFetcher + Clone,
{
    fn clone_box(&self) -> Box<URLFetcher> {
        Box::new(self.clone())
    }
}

impl Clone for Box<URLFetcher> {
    fn clone(&self) -> Box<URLFetcher> {
        self.clone_box()
    }
}

#[derive(Clone, Debug)]
pub struct URLFetcherLive {
    pub client: Client<hyper::client::HttpConnector, hyper::Body>,
    pub core:   Arc<Core>,
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
    pub data: Vec<u8>,
}

impl URLFetcher for URLFetcherPassThrough {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)> {
        return Ok((self.data.clone(), raw_url));
    }
}
