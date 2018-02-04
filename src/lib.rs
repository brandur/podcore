#![recursion_limit = "128"]

#[macro_use]
extern crate chan;
extern crate chrono;
extern crate crypto;
#[macro_use]
extern crate diesel;
#[macro_use]
extern crate error_chain;
extern crate futures;
#[macro_use]
extern crate hyper;
extern crate hyper_tls;
extern crate iron;
#[macro_use]
extern crate juniper;
extern crate juniper_iron;
#[macro_use]
extern crate lazy_static;
extern crate mount;
extern crate quick_xml;
extern crate r2d2;
extern crate r2d2_diesel;
extern crate rand;
extern crate regex;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
extern crate time;
extern crate tokio_core;
extern crate url;
extern crate uuid;

pub mod api;
pub mod errors;
pub mod graphql;
#[cfg(test)]
#[macro_use]
mod macros;
pub mod mediators;
mod model;
pub mod url_fetcher;

// Generated file: skip rustfmt
#[cfg_attr(rustfmt, rustfmt_skip)]
mod schema;

#[cfg(test)]
mod test_helpers;
