#![recursion_limit = "128"]

extern crate actix;
extern crate actix_web;
extern crate bytes;

#[macro_use]
extern crate chan;

extern crate chrono;
extern crate crypto;

#[macro_use]
extern crate diesel;

#[macro_use]
extern crate error_chain;

extern crate flate2;
extern crate futures;

#[macro_use]
extern crate horrorshow;

#[macro_use]
extern crate html5ever;

extern crate http;

#[macro_use]
extern crate hyper;

extern crate hyper_tls;

#[macro_use]
extern crate juniper;

#[macro_use]
extern crate lazy_static;

extern crate native_tls;
extern crate percent_encoding;
extern crate quick_xml;
extern crate r2d2;
extern crate r2d2_diesel;
extern crate rand;
extern crate regex;

#[macro_use]
extern crate serde_derive;

#[macro_use]
extern crate serde_json;

extern crate serde_urlencoded;

#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
extern crate time;
extern crate tokio_core;
extern crate url;
extern crate uuid;

pub mod api;
pub mod database_helpers;
pub mod error_helpers;
pub mod errors;

// Compiler and Clippy linting problems that come from within juniper macros
// and which can't currently be fixed.
#[allow(unused_parens)]
#[cfg_attr(feature = "cargo-clippy", allow(double_parens, op_ref))]
pub mod graphql;

mod html;

#[cfg(test)]
#[macro_use]
mod macros;

pub mod http_requester;
mod links;
pub mod mediators;
mod model;

// Generated file: skip rustfmt
#[cfg_attr(rustfmt, rustfmt_skip)]
mod schema;

#[macro_use]
mod server;

// We try to keep alphabetical order, but `middleware` must appear after
// `server` so that it can resolve its exported macros.
mod middleware;

#[cfg(test)]
mod test_data;

#[cfg(test)]
mod test_helpers;

mod time_helpers;
pub mod web;
