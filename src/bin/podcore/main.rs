extern crate clap;
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;
#[macro_use]
extern crate error_chain;
extern crate hyper;
extern crate hyper_tls;
extern crate isatty;
extern crate openssl_probe;
extern crate podcore;
extern crate r2d2;
extern crate r2d2_diesel;
extern crate rand;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;
extern crate tokio_core;

use podcore::api;
use podcore::error_helpers;
use podcore::errors::*;
use podcore::http_requester::{HttpRequesterFactoryLive, HttpRequesterLive};
use podcore::mediators::cleaner;
use podcore::mediators::directory_podcast_searcher;
use podcore::mediators::podcast_crawler;
use podcore::mediators::podcast_feed_location_upgrader;
use podcore::mediators::podcast_reingester;
use podcore::mediators::podcast_updater;
use podcore::web;

use clap::{App, ArgMatches, SubCommand};
use diesel::pg::PgConnection;
use hyper::Client;
use hyper_tls::HttpsConnector;
use isatty::stdout_isatty;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use rand::EntropyRng;
use rand::distributions::Alphanumeric;
use slog::{Drain, Logger};
use std::env;
use std::iter;
use std::thread;
use std::time::Duration;
use tokio_core::reactor::Core;

// Migrations get pulled into the final binary. This makes it quite a bit
// easier to run them on remote clusters without trouble.
embed_migrations!("./migrations");

// Main
//

fn main() {
    // While the various TLS libraries tend to work out of the box on Mac OS, the
    // location of CA certs can vary across Linux distributions. This is a
    // library that helps locate a usable bundle so that we can properly make
    // TLS requests.
    openssl_probe::init_ssl_cert_env_vars();

    // Note that when using `arg_from_usage`, `<arg>` is required and `[arg]` is
    // optional.
    let mut app = App::new("podcore")
        .version("0.1")
        .about("A general utility command for the podcore project")
        .arg_from_usage(
            "    --pool-timeout=[SECONDS] 'Timeout for getting a database connection from pool",
        )
        .arg_from_usage("    --log-async 'Log asynchronously (good for logging on servers)'")
        .arg_from_usage("-c, --num-connections=[NUM] 'Number of Postgres connections'")
        .arg_from_usage("-q, --quiet 'Quiets all output'")
        .subcommand(
            SubCommand::with_name("add")
                .about("Fetches a podcast and adds it to the database")
                .arg_from_usage("--force 'Force the podcast to be readded even if it exists'")
                .arg_from_usage("<URL>... 'URL(s) to fetch'"),
        )
        .subcommand(
            SubCommand::with_name("api")
                .about("Starts the API server")
                .arg_from_usage("-p, --port=[PORT] 'Port to bind server to'"),
        )
        .subcommand(
            SubCommand::with_name("clean")
                .about("Cleans the database (should be run periodically)")
                .arg_from_usage("--run-once 'Run only one time instead of looping'"),
        )
        .subcommand(
            SubCommand::with_name("crawl")
                .about("Crawls the web to retrieve podcasts that need to be updated")
                .arg_from_usage("--run-once 'Run only one time instead of looping'"),
        )
        .subcommand(
            SubCommand::with_name("error")
                .about("Triggers an error (for testing error output and Sentry)"),
        )
        .subcommand(SubCommand::with_name("migrate").about("Migrates the database"))
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
            SubCommand::with_name("sleep")
                .about("Sleep (useful for attaching to with Docker)")
                .arg_from_usage("<SLEEP_SECONDS>... 'Number of seconds to sleep'"),
        )
        .subcommand(
            SubCommand::with_name("upgrade-https")
                .about("Upgrades podcast locations to HTTPS for hosts known to support it"),
        )
        .subcommand(
            SubCommand::with_name("web")
                .about("Starts the web server")
                .arg_from_usage("-p, --port=[PORT] 'Port to bind server to'"),
        );

    let matches = app.clone().get_matches();
    let options = parse_global_options(&matches);
    let log = log(&options);

    let res = match matches.subcommand_name() {
        Some("add") => subcommand_add(&log, &matches, &options),
        Some("api") => subcommand_api(&log, &matches, &options),
        Some("clean") => subcommand_clean(&log, &matches, &options),
        Some("crawl") => subcommand_crawl(&log, &matches, &options),
        Some("error") => subcommand_error(&log, &matches, &options),
        Some("migrate") => subcommand_migrate(&log, &matches, &options),
        Some("reingest") => subcommand_reingest(&log, &matches, &options),
        Some("search") => subcommand_search(&log, &matches, &options),
        Some("sleep") => subcommand_sleep(&log, &matches, &options),
        Some("upgrade-https") => subcommand_upgrade_https(&log, &matches, &options),
        Some("web") => subcommand_web(&log, &matches, &options),
        None => {
            app.print_help().unwrap();
            Ok(())
        }
        _ => unreachable!(),
    };
    if let Err(ref e) = res {
        handle_error(&log, e);
    };
}

//
// Subcommands
//

fn subcommand_api(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("api").unwrap();

    let pool = pool(log, options)?;

    let server = api::Server {
        log: log.clone(),
        num_sync_executors: options.num_connections,
        pool,
        port: server_port(matches),
    };
    server.run()?;
    Ok(())
}

fn subcommand_add(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("add").unwrap();
    let force = matches.is_present("force");

    let pool = pool(log, options)?;
    let conn = pool.get()?;

    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
        .build(&core.handle());
    let mut http_requester = HttpRequesterLive { client, core };

    for url in matches.values_of("URL").unwrap().collect::<Vec<_>>() {
        podcast_updater::Mediator {
            conn:             &*conn,
            disable_shortcut: force,
            feed_url:         url.to_owned().to_owned(),
            http_requester:   &mut http_requester,
        }.run(log)?;
    }
    Ok(())
}

fn subcommand_clean(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("clean").unwrap();
    let mut num_loops = 0;
    let run_once = matches.is_present("run-once");

    loop {
        let res = cleaner::Mediator {
            pool: pool(log, options)?.clone(),
        }.run(log)?;

        num_loops += 1;
        info!(log, "Finished work loop";
            "num_loops" => num_loops,
            "num_account_cleaned" => res.num_account_cleaned,
            "num_directory_podcast_cleaned" => res.num_directory_podcast_cleaned,
            "num_directory_search_cleaned" => res.num_directory_search_cleaned,
            "num_key_cleaned" => res.num_key_cleaned,
            "num_podcast_feed_content_cleaned" => res.num_podcast_feed_content_cleaned);

        if run_once {
            break (Ok(()));
        }

        if res.num_cleaned < 1 {
            info!(log, "No rows cleaned -- sleeping"; "seconds" => SLEEP_SECONDS);
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
    }
}

fn subcommand_crawl(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("crawl").unwrap();
    let mut num_loops = 0;
    let run_once = matches.is_present("run-once");

    loop {
        let res = podcast_crawler::Mediator {
            num_workers:            options.num_connections - 1,
            pool:                   pool(log, options)?.clone(),
            http_requester_factory: Box::new(HttpRequesterFactoryLive {}),
        }.run(log)?;

        num_loops += 1;
        info!(log, "Finished work loop"; "num_loops" => num_loops, "num_podcasts" => res.num_podcasts);

        if run_once {
            break (Ok(()));
        }

        if res.num_podcasts < 1 {
            info!(log, "No podcasts processed -- sleeping"; "seconds" => SLEEP_SECONDS);
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
    }
}

fn subcommand_error(_log: &Logger, matches: &ArgMatches, _options: &GlobalOptions) -> Result<()> {
    let _matches = matches.subcommand_matches("error").unwrap();

    // We chain some extra context on to add a little flavor and to help show what
    // output would look like
    Err(Error::from("Error triggered by user request")
        .chain_err(|| "Chained context 1")
        .chain_err(|| "Chained context 2"))
}

fn subcommand_migrate(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let _matches = matches.subcommand_matches("migrate").unwrap();
    let pool = pool(log, options)?;
    let conn = pool.get()?;

    info!(log, "Running migrations");

    if options.quiet {
        embedded_migrations::run(&*conn)
    } else {
        embedded_migrations::run_with_output(&*conn, &mut std::io::stdout())
    }.chain_err(|| "Error running migrations")?;

    info!(log, "Finished migrations");
    Ok(())
}

fn subcommand_reingest(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let _matches = matches.subcommand_matches("reingest").unwrap();

    podcast_reingester::Mediator {
        num_workers: options.num_connections - 1,
        pool:        pool(log, options)?.clone(),
    }.run(log)?;
    Ok(())
}

fn subcommand_search(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("search").unwrap();
    let pool = pool(log, options)?;
    let conn = pool.get()?;

    let core = Core::new().unwrap();
    let client = Client::configure()
        .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
        .build(&core.handle());
    let mut http_requester = HttpRequesterLive { client, core };

    let query = matches.value_of("QUERY").unwrap();
    directory_podcast_searcher::Mediator {
        conn:           &*conn,
        query:          query.to_owned(),
        http_requester: &mut http_requester,
    }.run(log)?;
    Ok(())
}

fn subcommand_sleep(log: &Logger, matches: &ArgMatches, _options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("sleep").unwrap();

    let sleep_seconds = matches
        .value_of("SLEEP_SECONDS")
        .unwrap()
        .parse::<u64>()
        .chain_err(|| "Error parsing integer")?;

    info!(log, "Starting sleep"; "seconds" => sleep_seconds);
    thread::sleep(Duration::from_secs(sleep_seconds));
    info!(log, "Finished sleep");

    Ok(())
}

fn subcommand_upgrade_https(
    log: &Logger,
    matches: &ArgMatches,
    options: &GlobalOptions,
) -> Result<()> {
    let _matches = matches.subcommand_matches("upgrade-https").unwrap();
    let pool = pool(log, options)?;
    let conn = pool.get()?;

    let res = podcast_feed_location_upgrader::Mediator { conn: &*conn }.run(log)?;

    info!(log, "Finished podcast HTTPS upgrade"; "num_upgraded" => res.num_upgraded);
    Ok(())
}

fn subcommand_web(log: &Logger, matches: &ArgMatches, options: &GlobalOptions) -> Result<()> {
    let matches = matches.subcommand_matches("web").unwrap();

    let assets_version = env::var("ASSETS_VERSION").unwrap_or_else(|_| "1".to_owned());
    let cookie_secret = env::var("COOKIE_SECRET").unwrap_or_else(|_| secure_random_string(32));
    let cookie_secure = env::var("COOKIE_SECURE")
        .map(|s| s.parse::<bool>().unwrap())
        .unwrap_or(true);

    if cookie_secret.len() < 32 {
        bail!("COOKIE_SECRET must be at least 32 characters long");
    }
    if cookie_secure {
        debug!(log, "Using secured cookies; they'll only work over HTTPS");
    }

    let pool = pool(log, options)?;

    let server = web::Server {
        assets_version,
        cookie_secret,
        cookie_secure,
        log: log.clone(),
        num_sync_executors: options.num_connections,
        pool,
        port: server_port(matches),
    };
    server.run()?;
    Ok(())
}

//
// Private types/functions
//

// Timeout after which to close idle database connections in the pool. In
// seconds.
const IDLE_TIMEOUT: u64 = 10;

const NUM_CONNECTIONS: u32 = 50;

// Default timeout for blocking on the database pool waiting for a connections.
// In seconds.
const POOL_TIMEOUT: u64 = 10;

// Default port to start servers on.
const SERVER_PORT: &str = "8080";

// For commands that loop, the number of seconds to sleep between iterations
// where no records were processed.
const SLEEP_SECONDS: u64 = 60;

struct GlobalOptions {
    log_async:       bool,
    num_connections: u32,
    pool_timeout:    Duration,
    quiet:           bool,
}

fn handle_error(log: &Logger, e: &Error) {
    error_helpers::print_error(log, e);

    if let Err(inner_e) = error_helpers::report_error(log, e) {
        error_helpers::print_error(log, &inner_e);
    }

    ::std::process::exit(1);
}

fn log(options: &GlobalOptions) -> Logger {
    if options.quiet {
        slog::Logger::root(slog::Discard, o!())
    } else if options.log_async {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        slog::Logger::root(drain, o!())
    } else {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        let async_drain = slog_async::Async::new(drain).build().fuse();
        slog::Logger::root(async_drain, o!())
    }
}

fn parse_global_options(matches: &ArgMatches) -> GlobalOptions {
    GlobalOptions {
        // Go async if we've been explicitly told to do so. Otherwise, detect whether we should go
        // async based on whether stdout is a terminal. Sync is okay for terminals, but quite bad
        // for server logs.
        log_async: if matches.is_present("log-async") {
            true
        } else {
            !stdout_isatty()
        },

        num_connections: matches
            .value_of("num-connections")
            .map(|s| s.parse::<u32>().unwrap())
            .unwrap_or_else(|| {
                env::var("NUM_CONNECTIONS")
                    .map(|s| s.parse::<u32>().unwrap())
                    .unwrap_or(NUM_CONNECTIONS)
            }),

        pool_timeout: Duration::from_secs(
            matches
                .value_of("pool-timeout")
                .map(|s| s.parse::<u64>().unwrap())
                .unwrap_or_else(|| {
                    env::var("POOL_TIMEOUT")
                        .map(|s| s.parse::<u64>().unwrap())
                        .unwrap_or(POOL_TIMEOUT)
                }),
        ),

        quiet: matches.is_present("quiet"),
    }
}

/// Initializes and returns a connection pool suitable for use across threads.
fn pool(log: &Logger, options: &GlobalOptions) -> Result<Pool<ConnectionManager<PgConnection>>> {
    debug!(log, "Initializing connection pool";
        "num_connections" => options.num_connections,
        "pool_timeout" => format!("{:?}", options.pool_timeout));

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    Pool::builder()
        .connection_timeout(options.pool_timeout)
        .idle_timeout(Some(Duration::from_secs(IDLE_TIMEOUT)))
        .max_size(options.num_connections)
        // If `min_idle` is not set, then `r2d2` will open a number of connections equal to
        // `max_size` on startup. We'd much prefer a more constrained number of connections than an
        // ultra-hot startup, so keep this set at 0.
        .min_idle(Some(0))
        .build(manager)
        .map_err(Error::from)
}

/// Generates a secure random string of the given length.
fn secure_random_string(len: usize) -> String {
    use rand::Rng;
    let mut rng = EntropyRng::new();
    iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .take(len)
        .collect()
}

/// Gets a port from the program's argument or falls back to a value in `PORT`
/// or falls back to 8080.
fn server_port(matches: &ArgMatches) -> String {
    matches
        .value_of("port")
        .map(|p| p.to_owned())
        .unwrap_or_else(|| env::var("PORT").unwrap_or_else(|_| SERVER_PORT.to_owned()))
}
