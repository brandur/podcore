use errors::*;

use futures::Stream;
use hyper;
use hyper::{Client, Uri};
use slog::Logger;
#[cfg(test)]
use std::collections::HashMap;
use std::str;
use std::str::FromStr;
use time::precise_time_ns;
use tokio_core::reactor::Core;

pub trait URLFetcher {
    fn fetch(&mut self, raw_url: String) -> Result<(Vec<u8>, String)>;
}

/*
let mut core = Core::new().unwrap();
let client = Client::new(&core.handle());
let mut url_fetcher = URLFetcherLive {
    client: &client,
    core:   &mut core,
};
*/
#[allow(dead_code)]
pub struct URLFetcherLive<'a> {
    client: &'a Client<hyper::client::HttpConnector, hyper::Body>,
    core:   &'a mut Core,
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

#[inline]
pub fn log_timed<T, F>(log: &Logger, f: F) -> T
where
    F: FnOnce(&Logger) -> T,
{
    let start = precise_time_ns();
    info!(log, "Start");
    let res = f(&log);
    let elapsed = precise_time_ns() - start;
    let (div, unit) = unit(elapsed);
    info!(log, "Finish"; "elapsed" => format!("{:.*}{}", 3, ((elapsed as f64) / div), unit));
    res
}

//
// Private functions
//

#[inline]
fn unit(ns: u64) -> (f64, &'static str) {
    if ns >= 1_000_000_000 {
        (1_000_000_000_f64, "s")
    } else if ns >= 1_000_000 {
        (1_000_000_f64, "ms")
    } else if ns >= 1_000 {
        (1_000_f64, "µs")
    } else {
        (1_f64, "ns")
    }
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::common::*;

    #[test]
    fn test_unit() {
        assert_eq!((1_f64, "ns"), unit(2_u64));
        assert_eq!((1_000_f64, "µs"), unit(2_000_u64));
        assert_eq!((1_000_000_f64, "ms"), unit(2_000_000_u64));
        assert_eq!((1_000_000_000_f64, "s"), unit(2_000_000_000_u64));
    }
}
