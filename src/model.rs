//! The application's data layer containing models that will be queried from
//! and inserted into the database.
//!
//! Simple helper functions are allowed, but they should be kept extremely
//! simple, with preference for any and all domain logic to be offloaded to a
//! mediator.
//!
//! Insertable models are found in the `Insertable` module. These are distinct
//! from queryable models so that we can take advantage of default values
//! provided by the database (the best example being ID sequences, but applies
//! to any field with a `DEFAULT`).

use errors::*;
use schema;
use schema::directory_podcast;
use time_helpers;

use chrono::{DateTime, Utc};
use diesel;
use diesel::pg::PgConnection;
use diesel::pg::upsert::excluded;
use diesel::prelude::*;
use slog::Logger;

#[derive(Clone, Debug, Queryable)]
pub struct Account {
    pub id:           i64,
    pub created_at:   DateTime<Utc>,
    pub email:        Option<String>,
    pub ephemeral:    bool,
    pub last_ip:      String,
    pub last_seen_at: DateTime<Utc>,
    pub mobile:       bool,
}

#[derive(Default, Queryable)]
pub struct AccountPodcast {
    pub id:              i64,
    pub account_id:      i64,
    pub podcast_id:      i64,
    pub subscribed_at:   Option<DateTime<Utc>>,
    pub unsubscribed_at: Option<DateTime<Utc>>,
}

impl AccountPodcast {
    pub fn is_subscribed(&self) -> bool {
        self.subscribed_at.is_some() && self.unsubscribed_at.is_none()
    }
}

#[derive(Queryable)]
pub struct AccountPodcastEpisode {
    pub id:                 i64,
    pub account_podcast_id: i64,
    pub episode_id:         i64,
    pub favorited:          bool,
    pub listened_seconds:   Option<i64>,
    pub played:             bool,
    pub updated_at:         DateTime<Utc>,
}

#[derive(Queryable)]
pub struct Directory {
    pub id:   i64,
    pub name: String,
}

impl Directory {
    pub fn itunes(log: &Logger, conn: &PgConnection) -> Result<Self> {
        Self::load_dir(
            log,
            conn,
            &insertable::Directory {
                name: "Apple iTunes".to_owned(),
            },
        )
    }

    fn load_dir(
        log: &Logger,
        conn: &PgConnection,
        ins_dir: &insertable::Directory,
    ) -> Result<Self> {
        // We `SELECT` first because it's probably faster that way and we can pretty
        // much always expect the directory to exist, but if we want to take
        // all race conditions to zero, we could change this whole function to
        // only upsert.
        let dir = schema::directory::table
            .filter(schema::directory::name.eq(ins_dir.name.as_str()))
            .first::<Directory>(conn)
            .optional()
            .chain_err(|| format!("Error loading {} directory record", ins_dir.name.as_str()))?;

        if dir.is_some() {
            return Ok(dir.unwrap());
        }

        // If the directory was missing, upsert it.
        time_helpers::log_timed(&log.new(o!("step" => "upsert_directory")), |_log| {
            diesel::insert_into(schema::directory::table)
                .values(ins_dir)
                .on_conflict(schema::directory::name)
                .do_update()
                .set(schema::directory::name.eq(excluded(schema::directory::name)))
                .get_result(conn)
                .chain_err(|| "Error upserting directory")
        })
    }
}

#[changeset_options(treat_none_as_null = "true")]
#[derive(AsChangeset, Identifiable, Queryable)]
#[table_name = "directory_podcast"]
pub struct DirectoryPodcast {
    pub id:           i64,
    pub directory_id: i64,
    pub feed_url:     String,
    pub podcast_id:   Option<i64>,
    pub title:        String,
    pub vendor_id:    String,
    pub image_url:    Option<String>,
}

#[derive(Queryable)]
pub struct DirectoryPodcastException {
    pub id:                   i64,
    pub directory_podcast_id: i64,
    pub errors:               Vec<String>,
    pub occurred_at:          DateTime<Utc>,
}

#[derive(Queryable)]
pub struct DirectoryPodcastDirectorySearch {
    pub id:                   i64,
    pub directory_podcast_id: i64,
    pub directory_search_id:  i64,
    pub position:             i32,
}

#[derive(Queryable)]
pub struct DirectorySearch {
    pub id:           i64,
    pub directory_id: i64,
    pub query:        String,
    pub retrieved_at: DateTime<Utc>,
}

#[derive(Queryable)]
pub struct Episode {
    pub id:           i64,
    pub description:  Option<String>,
    pub explicit:     Option<bool>,
    pub guid:         String,
    pub link_url:     Option<String>,
    pub media_type:   Option<String>,
    pub media_url:    String,
    pub podcast_id:   i64,
    pub published_at: DateTime<Utc>,
    pub title:        String,
}

#[derive(Debug, Queryable)]
pub struct Key {
    pub id:         i64,
    pub account_id: i64,
    pub created_at: DateTime<Utc>,
    pub expire_at:  Option<DateTime<Utc>>,
    pub secret:     String,
}

#[derive(Queryable)]
pub struct Podcast {
    pub id:                i64,
    pub image_url:         Option<String>,
    pub language:          Option<String>,
    pub last_retrieved_at: DateTime<Utc>,
    pub link_url:          Option<String>,
    pub title:             String,
    pub description:       Option<String>,
}

#[allow(dead_code)]
#[derive(Queryable)]
pub struct PodcastException {
    pub id:          i64,
    pub podcast_id:  i64,
    pub errors:      Vec<String>,
    pub occurred_at: DateTime<Utc>,
}

#[allow(dead_code)]
#[derive(Queryable)]
pub struct PodcastFeedContent {
    pub id:           i64,
    pub podcast_id:   i64,
    pub retrieved_at: DateTime<Utc>,
    pub sha256_hash:  String,
    pub content_gzip: Option<Vec<u8>>,
}

#[allow(dead_code)]
#[derive(Queryable)]
pub struct PodcastFeedLocation {
    pub id:                 i64,
    pub first_retrieved_at: DateTime<Utc>,
    pub feed_url:           String,
    pub last_retrieved_at:  DateTime<Utc>,
    pub podcast_id:         i64,
}

#[cfg(test)]
mod tests {
    use model::*;

    #[test]
    fn test_podcast_is_subscribed() {
        let mut account_podcast = AccountPodcast::default();
        assert!(!account_podcast.is_subscribed());

        account_podcast.subscribed_at = Some(Utc::now());
        assert!(account_podcast.is_subscribed());

        account_podcast.unsubscribed_at = Some(Utc::now());
        assert!(!account_podcast.is_subscribed());
    }
}

pub mod insertable {
    use schema::{account, account_podcast, account_podcast_episode, directory, directory_podcast,
                 directory_podcast_directory_search, directory_podcast_exception,
                 directory_search, episode, key, podcast, podcast_exception, podcast_feed_content,
                 podcast_feed_location};

    use chrono::{DateTime, Utc};

    #[derive(Insertable)]
    #[table_name = "account"]
    pub struct Account {
        pub email:     Option<String>,
        pub ephemeral: bool,
        pub last_ip:   String,
        pub mobile:    bool,
    }

    #[derive(Insertable)]
    #[table_name = "account_podcast"]
    pub struct AccountPodcast {
        pub account_id:      i64,
        pub podcast_id:      i64,
        pub subscribed_at:   Option<DateTime<Utc>>,
        pub unsubscribed_at: Option<DateTime<Utc>>,
    }

    #[derive(Insertable)]
    #[table_name = "account_podcast_episode"]
    pub struct AccountPodcastEpisode {
        pub account_podcast_id: i64,
        pub episode_id:         i64,
        pub listened_seconds:   Option<i64>,
        pub played:             bool,
        pub updated_at:         DateTime<Utc>,
    }

    #[derive(Insertable)]
    #[table_name = "account_podcast_episode"]
    pub struct AccountPodcastEpisodeFavorite {
        pub account_podcast_id: i64,
        pub episode_id:         i64,
        pub favorited:          bool,
        pub listened_seconds:   Option<i64>,
        pub updated_at:         DateTime<Utc>,
    }

    #[derive(Insertable)]
    #[table_name = "directory"]
    pub struct Directory {
        pub name: String,
    }

    #[derive(Insertable)]
    #[table_name = "directory_podcast"]
    pub struct DirectoryPodcast {
        pub directory_id: i64,
        pub feed_url:     String,
        pub podcast_id:   Option<i64>,
        pub title:        String,
        pub vendor_id:    String,
        pub image_url:    Option<String>,
    }

    #[derive(Insertable)]
    #[table_name = "directory_podcast_exception"]
    pub struct DirectoryPodcastException {
        pub directory_podcast_id: i64,
        pub errors:               Vec<String>,
        pub occurred_at:          DateTime<Utc>,
    }

    #[derive(Insertable)]
    #[table_name = "directory_podcast_directory_search"]
    pub struct DirectoryPodcastDirectorySearch {
        pub directory_podcast_id: i64,
        pub directory_search_id:  i64,
        pub position:             i32,
    }

    #[derive(Insertable)]
    #[table_name = "directory_search"]
    pub struct DirectorySearch {
        pub directory_id: i64,
        pub query:        String,
        pub retrieved_at: DateTime<Utc>,
    }

    #[derive(Insertable)]
    #[table_name = "episode"]
    pub struct Episode {
        pub description:  Option<String>,
        pub explicit:     Option<bool>,
        pub guid:         String,
        pub link_url:     Option<String>,
        pub media_type:   Option<String>,
        pub media_url:    String,
        pub podcast_id:   i64,
        pub published_at: DateTime<Utc>,
        pub title:        String,
    }

    #[derive(Insertable)]
    #[table_name = "key"]
    pub struct Key {
        pub account_id: i64,
        pub expire_at:  Option<DateTime<Utc>>,
        pub secret:     String,
    }

    #[changeset_options(treat_none_as_null = "true")]
    #[derive(AsChangeset, Insertable)]
    #[table_name = "podcast"]
    pub struct Podcast {
        pub image_url:         Option<String>,
        pub language:          Option<String>,
        pub last_retrieved_at: DateTime<Utc>,
        pub link_url:          Option<String>,
        pub title:             String,
        pub description:       Option<String>,
    }

    #[derive(Insertable)]
    #[table_name = "podcast_exception"]
    pub struct PodcastException {
        pub podcast_id:  i64,
        pub errors:      Vec<String>,
        pub occurred_at: DateTime<Utc>,
    }

    #[derive(Insertable)]
    #[table_name = "podcast_feed_content"]
    pub struct PodcastFeedContent {
        pub content_gzip: Vec<u8>,
        pub podcast_id:   i64,
        pub retrieved_at: DateTime<Utc>,
        pub sha256_hash:  String,
    }

    #[derive(Insertable)]
    #[table_name = "podcast_feed_location"]
    pub struct PodcastFeedLocation {
        pub first_retrieved_at: DateTime<Utc>,
        pub feed_url:           String,
        pub last_retrieved_at:  DateTime<Utc>,
        pub podcast_id:         i64,
    }
}
