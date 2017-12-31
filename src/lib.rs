#![recursion_limit = "128"]

extern crate chrono;
extern crate crypto;
#[macro_use]
extern crate diesel;
#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate hyper;
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
extern crate regex;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
extern crate time;
extern crate tokio_core;

mod errors;
pub mod graphql;
#[cfg(test)]
#[macro_use]
mod macros;
pub mod mediators;
mod model;

// Generated file: skip rustfmt
#[cfg_attr(rustfmt, rustfmt_skip)]
mod schema;

#[cfg(test)]
mod test_helpers;
