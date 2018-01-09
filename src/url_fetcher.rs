use errors::*;

use futures::Stream;
use hyper;
use hyper::{Client, Uri};
#[cfg(test)]
use std::collections::HashMap;
use std::str::FromStr;
use tokio_core::reactor::Core;

pub trait URLFetcher {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)>;
}

pub struct URLFetcherLive<'a> {
    pub client: &'a Client<hyper::client::HttpConnector, hyper::Body>,
    pub core:   &'a mut Core,
}

impl<'a> URLFetcher for URLFetcherLive<'a> {
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

pub struct URLFetcherPassThrough {
    pub data: Vec<u8>,
}

impl URLFetcher for URLFetcherPassThrough {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)> {
        return Ok((self.data.clone(), raw_url));
    }
}

#[cfg(test)]
pub struct URLFetcherStub {
    pub map: HashMap<&'static str, Vec<u8>>,
}

#[cfg(test)]
impl URLFetcher for URLFetcherStub {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)> {
        Ok((self.map.get(raw_url.as_str()).unwrap().clone(), raw_url))
    }
}
