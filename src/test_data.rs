extern crate rand;

use http_requester::HttpRequesterPassThrough;
use mediators::podcast_updater;
use model;
use test_helpers;

use diesel::pg::PgConnection;
use rand::Rng;
use slog::Logger;
use std::sync::Arc;

pub fn insert_podcast(log: &Logger, conn: &PgConnection) -> model::Podcast {
    let mut rng = rand::thread_rng();
    podcast_updater::Mediator {
        conn:             conn,
        disable_shortcut: false,

        // Add a little randomness to feed URLs so that w don't just insert one podcast and
        // update it over and over.
        feed_url: format!("https://example.com/feed-{}.xml", rng.gen::<u64>()).to_string(),

        http_requester: &mut HttpRequesterPassThrough {
            data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
        },
    }.run(log)
        .unwrap()
        .podcast
}
