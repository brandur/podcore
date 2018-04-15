extern crate rand;

use http_requester::HttpRequesterPassThrough;
use model;
use model::insertable;
use schema;
use test_helpers;

use chrono::{DateTime, Utc};
use diesel;
use diesel::pg::PgConnection;
use rand::distributions::Alphanumeric;
use slog::Logger;
use std::iter;
use std::sync::Arc;

pub mod account {
    use mediators::account_creator;
    use test_data::*;

    pub struct Args<'a> {
        pub email:     Option<&'a str>,
        pub ephemeral: bool,
        pub mobile:    bool,
    }

    pub fn insert(log: &Logger, conn: &PgConnection) -> model::Account {
        insert_args(
            log,
            conn,
            Args {
                email:     None,
                ephemeral: true,
                mobile:    false,
            },
        )
    }

    pub fn insert_args(log: &Logger, conn: &PgConnection, args: Args) -> model::Account {
        account_creator::Mediator {
            conn,
            email: args.email,
            ephemeral: args.ephemeral,
            last_ip: "1.2.3.4",
            mobile: args.mobile,
            password: match args.email {
                Some(_) => Some("password123"),
                None => None,
            },
        }.run(log)
            .unwrap()
            .account
    }
}

pub mod account_podcast {
    use mediators::account_podcast_subscriber;
    use test_data::*;

    #[derive(Default)]
    pub struct Args<'a> {
        pub account: Option<&'a model::Account>,
        pub podcast: Option<&'a model::Podcast>,
    }

    #[allow(dead_code)]
    pub fn insert(log: &Logger, conn: &PgConnection) -> model::AccountPodcast {
        insert_args(log, conn, Args::default())
    }

    pub fn insert_args(log: &Logger, conn: &PgConnection, args: Args) -> model::AccountPodcast {
        let account = if args.account.is_none() {
            Some(super::account::insert(log, conn))
        } else {
            None
        };

        let podcast = if args.podcast.is_none() {
            Some(super::podcast::insert(log, conn))
        } else {
            None
        };

        account_podcast_subscriber::Mediator {
            account: args.account.unwrap_or_else(|| account.as_ref().unwrap()),
            conn,
            podcast: args.podcast.unwrap_or_else(|| podcast.as_ref().unwrap()),
            subscribed: true,
        }.run(log)
            .unwrap()
            .account_podcast
            .unwrap()
    }
}

pub mod account_podcast_episode {
    use mediators::account_podcast_episode_upserter;
    use test_data::*;

    #[derive(Default)]
    pub struct Args<'a> {
        pub account: Option<&'a model::Account>,
        pub episode: Option<&'a model::Episode>,
    }

    #[allow(dead_code)]
    fn insert(log: &Logger, conn: &PgConnection) -> model::AccountPodcastEpisode {
        insert_args(log, conn, Args::default())
    }

    pub fn insert_args(
        log: &Logger,
        conn: &PgConnection,
        args: Args,
    ) -> model::AccountPodcastEpisode {
        let account = if args.account.is_none() {
            Some(super::account::insert(log, conn))
        } else {
            None
        };
        let account_ref = args.account.unwrap_or_else(|| account.as_ref().unwrap());

        let episode: Option<model::Episode> = if args.episode.is_none() {
            let podcast = super::podcast::insert(log, conn);
            Some(super::episode::first(log, conn, &podcast))
        } else {
            None
        };
        let episode_ref = args.episode.unwrap_or_else(|| episode.as_ref().unwrap());

        account_podcast_episode_upserter::Mediator {
            account: account_ref,
            conn,
            episode: episode_ref,
            listened_seconds: None,
            played: true,
        }.run(log)
            .unwrap()
            .account_podcast_episode
    }
}

pub mod directory_podcast {
    use test_data::*;

    use diesel::prelude::*;
    use rand::Rng;

    #[derive(Default)]
    pub struct Args<'a> {
        pub podcast: Option<&'a model::Podcast>,
    }

    pub fn insert(log: &Logger, conn: &PgConnection) -> model::DirectoryPodcast {
        insert_args(log, conn, Args::default())
    }

    pub fn insert_args(log: &Logger, conn: &PgConnection, args: Args) -> model::DirectoryPodcast {
        let mut rng = rand::thread_rng();

        let directory = model::Directory::itunes(log, &conn).unwrap();

        let dir_podcast_ins = insertable::DirectoryPodcast {
            directory_id: directory.id,
            feed_url:     "https://example.com/feed.xml".to_owned(),
            image_url:    Some("https://example.com/image.jpg".to_owned()),
            podcast_id:   args.podcast.map(|p| p.id),
            title:        "Example Podcast".to_owned(),
            vendor_id:    iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .take(50)
                .collect(),
        };

        diesel::insert_into(schema::directory_podcast::table)
            .values(&dir_podcast_ins)
            .get_result(conn)
            .unwrap()
    }
}

pub mod episode {
    use test_data::*;

    use diesel::prelude::*;

    // `test_data` to get an episode is different because episodes are normally
    // created as a podcast feed is ingested from an XML file. We therefore
    // require here that a podcast is passed and we'll simply select the first
    // episode for it.
    pub fn first(_log: &Logger, conn: &PgConnection, podcast: &model::Podcast) -> model::Episode {
        schema::episode::table
            .filter(schema::episode::podcast_id.eq(podcast.id))
            .first(conn)
            .unwrap()
    }
}

pub mod key {
    use mediators::key_creator;
    use test_data::*;

    #[derive(Default)]
    pub struct Args<'a> {
        pub account:   Option<&'a model::Account>,
        pub expire_at: Option<DateTime<Utc>>,
    }

    #[allow(dead_code)]
    pub fn insert(log: &Logger, conn: &PgConnection) -> model::Key {
        insert_args(log, conn, Args::default())
    }

    pub fn insert_args(log: &Logger, conn: &PgConnection, args: Args) -> model::Key {
        let account = if args.account.is_none() {
            Some(super::account::insert(log, conn))
        } else {
            None
        };

        key_creator::Mediator {
            account: args.account.unwrap_or_else(|| account.as_ref().unwrap()),
            conn,
            expire_at: args.expire_at,
        }.run(log)
            .unwrap()
            .key
    }
}

pub mod podcast {
    use mediators::podcast_updater;
    use test_data::*;

    use rand::Rng;

    #[derive(Default)]
    pub struct Args {
        pub feed_url: Option<String>,
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
            conn,
            disable_shortcut: false,
            feed_url,
            http_requester: &mut HttpRequesterPassThrough {
                data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
            },
        }.run(log)
            .unwrap()
            .podcast
    }
}
