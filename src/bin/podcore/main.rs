extern crate clap;
extern crate diesel;
extern crate hyper;
extern crate hyper_tls;
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
use podcore::errors;
use podcore::errors::*;
use podcore::graphql;
use podcore::mediators::directory_podcast_searcher::DirectoryPodcastSearcher;
use podcore::mediators::error_reporter::{ErrorReporter, SentryCredentials};
use podcore::mediators::podcast_crawler::PodcastCrawler;
use podcore::mediators::podcast_reingester::PodcastReingester;
use podcore::mediators::podcast_updater::PodcastUpdater;
use podcore::url_fetcher::{URLFetcherFactoryLive, URLFetcherLive};

use clap::{App, ArgMatches, SubCommand};
use diesel::pg::PgConnection;
use hyper::Client;
use hyper_tls::HttpsConnector;
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
    // Note that when using `arg_from_usage`, `<arg>` is required and `[arg]` is
    // optional.
    let mut app = App::new("podcore")
        .version("0.1")
        .about("A general utility command for the podcore project")
        .arg_from_usage("-c, --num-connections [NUM_CONNECTIONS] 'Number of Postgres connections'")
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
            SubCommand::with_name("error")
                .about("Triggers an error (for testing error output and Sentry)"),
        )
        .subcommand(
            SubCommand::with_name("reingest")
                .about("Reingests podcasts by reusing their stored raw feeds"),
        )
        .subcommand(
            SubCommand::with_name("search")
                .about("Search iTunes directory for podcasts")
                .arg_from_usage("[QUERY]... 'Search query'"),
        )
        .subcommand(
            SubCommand::with_name("serve")
                .about("Starts the API server")
                .arg_from_usage("-p, --port [PORT] 'Port to bind server to'"),
        );

    let matches = app.clone().get_matches();
    let options = parse_global_options(&matches);

    let res = match matches.subcommand_name() {
        Some("add") => add_podcast(matches, &options),
        Some("crawl") => crawl_podcasts(matches, &options),
        Some("error") => trigger_error(matches, &options),
        Some("reingest") => reingest_podcasts(matches, &options),
        Some("search") => search_podcasts(matches, &options),
        Some("serve") => serve_http(matches, &options),
        None => {
            app.print_help().unwrap();
            Ok(())
        }
        _ => unreachable!(),
    };
    if let Err(ref e) = res {
        handle_error(&e, options.quiet);
    };
}

//
// Subcommands
//

fn add_podcast(matches: ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("add").unwrap();

    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle())
            .chain_err(|| "Error initializing HTTPS connector")?)
        .build(&core.handle());
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
        }.run(&log(options.quiet))?;
    }
    Ok(())
}

fn crawl_podcasts(matches: ArgMatches, options: &GlobalOptions) -> Result<()> {
    let log = log(options.quiet);
    let _matches = matches.subcommand_matches("crawl").unwrap();
    let mut num_loops = 0;

    loop {
        let res = PodcastCrawler {
            num_workers:         options.num_connections - 1,
            pool:                pool(options.num_connections).clone(),
            url_fetcher_factory: Box::new(URLFetcherFactoryLive {}),
        }.run(&log)?;

        num_loops += 1;
        info!(log, "Finished work loop"; "num_loops" => num_loops, "num_podcasts" => res.num_podcasts);

        if res.num_podcasts < 1 {
            info!(log, "No podcasts processed -- sleeping"; "seconds" => SLEEP_SECONDS);
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
    }
}

fn reingest_podcasts(matches: ArgMatches, options: &GlobalOptions) -> Result<()> {
    let _matches = matches.subcommand_matches("reingest").unwrap();

    PodcastReingester {
        num_workers: options.num_connections - 1,
        pool:        pool(options.num_connections).clone(),
    }.run(&log(options.quiet))?;
    Ok(())
}

fn search_podcasts(matches: ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("search").unwrap();

    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle())
            .chain_err(|| "Error initializing HTTPS connector")?)
        .build(&core.handle());
    let mut url_fetcher = URLFetcherLive {
        client: client,
        core:   core,
    };

    let query = matches.value_of("QUERY").unwrap();
    DirectoryPodcastSearcher {
        conn:        &*connection(),
        query:       query.to_owned(),
        url_fetcher: &mut url_fetcher,
    }.run(&log(options.quiet))?;
    Ok(())
}

fn serve_http(matches: ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("serve").unwrap();

    let port = env::var("PORT").unwrap_or("8080".to_owned());
    let port = matches.value_of("PORT").unwrap_or_else(|| port.as_str());
    let host = format!("0.0.0.0:{}", port);
    let log = log(options.quiet);
    let num_connections = options.num_connections;

    let mut mount = Mount::new();

    let graphiql_endpoint = GraphiQLHandler::new("/graphql");
    mount.mount("/", graphiql_endpoint);

    let graphql_endpoint = GraphQLHandler::new(
        move |_: &mut Request| -> graphql::Context { graphql::Context::new(pool(num_connections)) },
        graphql::Query::new(),
        graphql::Mutation::new(),
    );
    mount.mount("/graphql", graphql_endpoint);

    info!(log, "API starting on: {}", host);
    Iron::new(api::chain(&log, mount))
        .http(host.as_str())
        .chain_err(|| "Error binding API")?;
    Ok(())
}

fn trigger_error(matches: ArgMatches, _options: &GlobalOptions) -> Result<()> {
    let _matches = matches.subcommand_matches("error").unwrap();

    // We chain some extra context on to add a little flavor and to help show what
    // output would look like
    Err(Error::from("Error triggered by user request")
        .chain_err(|| "Chained context 1")
        .chain_err(|| "Chained context 2"))
}

//
// Private types/functions
//

const NUM_CONNECTIONS: u32 = 50;

// For commands that loop, the number of seconds to sleep between iterations
// where no records were processed.
const SLEEP_SECONDS: u64 = 60;

struct GlobalOptions {
    num_connections: u32,
    quiet:           bool,
}

/// Acquires a single connection from a connection pool. This is suitable for use a shortcut by
/// subcommands that only need to run one single-threaded task.
fn connection() -> PooledConnection<ConnectionManager<PgConnection>> {
    pool(1)
        .get()
        .expect("Error acquiring connection from connection pool")
}

fn handle_error(e: &Error, quiet: bool) {
    errors::print_error(e);

    if let Err(inner_e) = report_error(e, quiet) {
        errors::print_error(&inner_e);
    }

    ::std::process::exit(1);
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

fn parse_global_options(matches: &ArgMatches) -> GlobalOptions {
    GlobalOptions {
        num_connections: env::var("NUM_CONNECTIONS")
            .map(|s| s.parse::<u32>().unwrap())
            .unwrap_or(
                matches
                    .value_of("NUM_CONNECTIONS")
                    .map(|s| s.parse::<u32>().unwrap())
                    .unwrap_or(NUM_CONNECTIONS),
            ),
        quiet:           matches.is_present("quiet"),
    }
}

/// Initializes and returns a connection pool suitable for use across threads.
fn pool(num_connections: u32) -> Pool<ConnectionManager<PgConnection>> {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    Pool::builder()
        .idle_timeout(Some(Duration::from_secs(5)))
        .max_size(num_connections)
        .build(manager)
        .expect("Failed to create pool.")
}

// Reports an error to Sentry.
fn report_error(error: &Error, quiet: bool) -> Result<()> {
    match env::var("SENTRY_URL") {
        Ok(url) => {
            use std::io::Write;
            let stderr = &mut ::std::io::stderr();

            writeln!(stderr, "Sending event to Sentry").unwrap();

            let core = Core::new().unwrap();
            let client = Client::configure()
                .connector(HttpsConnector::new(4, &core.handle())
                    .chain_err(|| "Error initializing HTTPS connector")?)
                .build(&core.handle());
            let creds = url.parse::<SentryCredentials>().unwrap();
            let mut url_fetcher = URLFetcherLive {
                client: client,
                core:   core,
            };

            let _res = ErrorReporter {
                creds:       &creds,
                error:       &error,
                url_fetcher: &mut url_fetcher,
            }.run(&log(quiet))?;
            Ok(())
        }
        Err(_) => Ok(()),
    }
}
