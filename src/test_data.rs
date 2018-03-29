extern crate rand;

use http_requester::HttpRequesterPassThrough;
use mediators::podcast_updater;
use model;
use test_helpers;

use diesel::pg::PgConnection;
use slog::Logger;
use std::sync::Arc;

pub mod podcast {
    use test_data::*;

    use rand::Rng;

    #[derive(Default)]
    pub struct Args {
        feed_url: Option<String>,
    }

    pub fn insert(log: &Logger, conn: &PgConnection) -> model::Podcast {
        insert_args(log, conn, Args::default())
    }

    pub fn insert_args(log: &Logger, conn: &PgConnection, args: Args) -> model::Podcast {
        let mut rng = rand::thread_rng();

        let feed_url = match args.feed_url {
            Some(feed_url) => feed_url,

            // Add a little randomness to feed URLs so that w don't just insert one podcast and
            // update it over and over.
            None => format!("https://example.com/feed-{}.xml", rng.gen::<u64>()).to_string(),
        };

        podcast_updater::Mediator {
            conn:             conn,
            disable_shortcut: false,
            feed_url:         feed_url,
            http_requester:   &mut HttpRequesterPassThrough {
                data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
            },
        }.run(log)
            .unwrap()
            .podcast
    }
}
