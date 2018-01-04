extern crate diesel;
extern crate hyper;
extern crate podcore;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
extern crate tokio_core;

use podcore::mediators::podcast_updater::PodcastUpdater;
use podcore::url_fetcher::URLFetcherLive;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use hyper::Client;
use slog::Drain;
use std::env;
use tokio_core::reactor::Core;

//
// Main
//

fn main() {
    let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
    let drain = slog_term::CompactFormat::new(decorator).build().fuse();
    let async_drain = slog_async::Async::new(drain).build().fuse();
    let log = slog::Logger::root(async_drain, o!());

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let conn = PgConnection::establish(&database_url).unwrap();

    let mut core = Core::new().unwrap();
    let client = Client::new(&core.handle());
    let mut url_fetcher = URLFetcherLive {
        client: &client,
        core:   &mut core,
    };

    PodcastUpdater {
        conn:             &conn,
        disable_shortcut: false,
        feed_url:         "http://feeds.feedburner.com/RoderickOnTheLine".to_owned(),
        url_fetcher:      &mut url_fetcher,
    }.run(&log)
        .unwrap();
}
