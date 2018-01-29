extern crate clap;
extern crate diesel;
extern crate hyper;
extern crate iron;
extern crate juniper_iron;
extern crate mount;
extern crate podcore;
extern crate r2d2;
extern crate r2d2_diesel;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
extern crate tokio_core;

use podcore::api;
use podcore::graphql;
use podcore::mediators::podcast_crawler::PodcastCrawler;
use podcore::mediators::podcast_reingester::PodcastReingester;
use podcore::mediators::podcast_updater::PodcastUpdater;
use podcore::url_fetcher::{URLFetcherFactoryLive, URLFetcherLive};

use clap::{App, ArgMatches, SubCommand};
use diesel::pg::PgConnection;
use hyper::Client;
use iron::prelude::*;
use juniper_iron::{GraphQLHandler, GraphiQLHandler};
use mount::Mount;
use r2d2::{Pool, PooledConnection};
use r2d2_diesel::ConnectionManager;
use slog::{Drain, Logger};
use std::env;
use std::thread;
use std::time::Duration;
use tokio_core::reactor::Core;

// Main
//

fn main() {
    // Note that when using `arg_from_usage`, `<arg>` is required and `[arg]` is optional.
    let mut app = App::new("podcore")
        .version("0.1")
        .about("A general utility command for the podcore project")
        .arg_from_usage("-q --quiet 'Quiets all output'")
        .subcommand(
            SubCommand::with_name("add")
                .about("Fetches a podcast and adds it to the database")
                .arg_from_usage("<URL>... 'URL(s) to fetch'"),
        )
        .subcommand(
            SubCommand::with_name("crawl")
                .about("Crawls the web to retrieve podcasts that need to be updated"),
        )
        .subcommand(
            SubCommand::with_name("reingest")
                .about("Reingests podcasts by reusing their stored raw feeds"),
        )
        .subcommand(
            SubCommand::with_name("serve")
                .about("Starts the API server")
                .arg_from_usage("-p, --port [PORT] 'Port to bind server to'"),
        );

    let matches = app.clone().get_matches();
    match matches.subcommand_name() {
        Some("add") => add_podcast(matches),
        Some("crawl") => crawl_podcasts(matches),
        Some("reingest") => reingest_podcasts(matches),
        Some("serve") => serve_http(matches),
        None => {
            app.print_help().unwrap();
            return;
        }
        _ => unreachable!(),
    }
}

// Subcommands
//

fn add_podcast(matches: ArgMatches) {
    let quiet = matches.is_present("quiet");
    let matches = matches.subcommand_matches("add").unwrap();

    let core = Core::new().unwrap();
    let client = Client::new(&core.handle());
    let mut url_fetcher = URLFetcherLive {
        client: client,
        core:   core,
    };

    for url in matches.values_of("URL").unwrap().collect::<Vec<_>>().iter() {
        PodcastUpdater {
            conn:             &*connection(),
            disable_shortcut: false,
            feed_url:         url.to_owned().to_owned(),
            url_fetcher:      &mut url_fetcher,
        }.run(&log(quiet))
            .unwrap();
    }
}

fn crawl_podcasts(matches: ArgMatches) {
    let quiet = matches.is_present("quiet");
    let log = log(quiet);
    let _matches = matches.subcommand_matches("crawl").unwrap();
    let mut num_loops = 0;

    loop {
        let res = PodcastCrawler {
            num_workers:         NUM_CONNECTIONS - 1,
            pool:                pool().clone(),
            url_fetcher_factory: Box::new(URLFetcherFactoryLive {}),
        }.run(&log)
            .unwrap();

        num_loops += 1;
        info!(log, "Finished work loop"; "num_loops" => num_loops, "num_podcasts" => res.num_podcasts);

        if res.num_podcasts < 1 {
            info!(log, "No podcasts processed -- sleeping"; "seconds" => SLEEP_SECONDS);
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
    }
}

// For commands that loop, the number of seconds to sleep between iterations where no records were
// processed.
const SLEEP_SECONDS: u64 = 60;

fn reingest_podcasts(matches: ArgMatches) {
    let quiet = matches.is_present("quiet");
    let _matches = matches.subcommand_matches("reingest").unwrap();

    PodcastReingester {
        num_workers: NUM_CONNECTIONS - 1,
        pool:        pool().clone(),
    }.run(&log(quiet))
        .unwrap();
}

fn serve_http(matches: ArgMatches) {
    let quiet = matches.is_present("quiet");
    let matches = matches.subcommand_matches("serve").unwrap();

    let port = env::var("PORT").unwrap_or("8080".to_owned());
    let port = matches.value_of("PORT").unwrap_or_else(|| port.as_str());
    let host = format!("0.0.0.0:{}", port);
    let log = log(quiet);

    let mut mount = Mount::new();

    let graphiql_endpoint = GraphiQLHandler::new("/graphql");
    mount.mount("/", graphiql_endpoint);

    let graphql_endpoint = GraphQLHandler::new(
        move |_: &mut Request| -> graphql::Context { graphql::Context::new(pool()) },
        graphql::Query::new(),
        graphql::Mutation::new(),
    );
    mount.mount("/graphql", graphql_endpoint);

    info!(log, "API starting on: {}", host);
    Iron::new(api::chain(&log, mount))
        .http(host.as_str())
        .unwrap();
}

// Private types/functions
//

pub static NUM_CONNECTIONS: u32 = 10;

/// Acquires a single connection from a connection pool. This is suitable for use a shortcut by
/// subcommands that only need to run one single-threaded task.
fn connection() -> PooledConnection<ConnectionManager<PgConnection>> {
    pool()
        .get()
        .expect("Error acquiring connection from connection pool")
}

fn log(quiet: bool) -> Logger {
    if !quiet {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        let async_drain = slog_async::Async::new(drain).build().fuse();
        slog::Logger::root(async_drain, o!())
    } else {
        slog::Logger::root(slog::Discard, o!())
    }
}

/// Initializes and returns a connection pool suitable for use across threads.
fn pool() -> Pool<ConnectionManager<PgConnection>> {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    Pool::builder()
        .idle_timeout(Some(Duration::from_secs(5)))
        .max_size(NUM_CONNECTIONS)
        .build(manager)
        .expect("Failed to create pool.")
}
