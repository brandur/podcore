use errors::*;
use schema::{directories, directories_podcasts};

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::pg::PgConnection;

//
// Database models for the application.
//
// Note that models are separately into `Queryable` and `Insertable` versions (with the latter
// located in the `insertable` module) so that we can insert rows with default values like we'd
// want to for an autoincrementing primary key. See here for details:
//
// https://github.com/diesel-rs/diesel/issues/1440
//

#[allow(dead_code)]
#[derive(Queryable)]
pub struct Directory {
    pub id:   i64,
    pub name: String,
}

impl Directory {
    #[allow(dead_code)]
    pub fn itunes(conn: &PgConnection) -> Result<Self> {
        Self::load_dir(conn, "Apple iTunes")
    }

    #[allow(dead_code)]
    fn load_dir(conn: &PgConnection, name: &str) -> Result<Self> {
        directories::table
            .filter(directories::name.eq(name))
            .first::<Directory>(conn)
            .chain_err(|| format!("Error loading {} directory record", name))
    }
}

#[derive(AsChangeset, Identifiable, Queryable)]
#[table_name = "directories_podcasts"]
pub struct DirectoryPodcast {
    pub id:           i64,
    pub directory_id: i64,
    pub feed_url:     Option<String>,
    pub podcast_id:   Option<i64>,
    pub vendor_id:    String,
}

#[allow(dead_code)]
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

#[derive(Queryable)]
pub struct Podcast {
    pub id:        i64,
    pub image_url: Option<String>,
    pub language:  Option<String>,
    pub link_url:  Option<String>,
    pub title:     String,
}

#[allow(dead_code)]
#[derive(Queryable)]
pub struct PodcastFeedContent {
    pub id:           i64,
    pub content:      String,
    pub podcast_id:   i64,
    pub retrieved_at: DateTime<Utc>,
    pub sha256_hash:  String,
}

#[allow(dead_code)]
#[derive(Queryable)]
pub struct PodcastFeedLocation {
    pub id:                i64,
    pub discovered_at:     DateTime<Utc>,
    pub feed_url:          String,
    pub last_retrieved_at: DateTime<Utc>,
    pub podcast_id:        i64,
}

pub mod insertable {
    use schema::{directories_podcasts, directory_searches, episodes, podcast_feed_contents,
                 podcast_feed_locations, podcasts};

    use chrono::{DateTime, Utc};

    #[derive(Insertable)]
    #[table_name = "directories_podcasts"]
    pub struct DirectoryPodcast {
        pub directory_id: i64,
        pub feed_url:     Option<String>,
        pub podcast_id:   Option<i64>,
        pub vendor_id:    String,
    }

    #[allow(dead_code)]
    #[derive(Insertable)]
    #[table_name = "directory_searches"]
    pub struct DirectorySearch {
        pub directory_id: i64,
        pub query:        String,
        pub retrieved_at: DateTime<Utc>,
    }

    #[derive(Insertable)]
    #[table_name = "episodes"]
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
    #[table_name = "podcasts"]
    pub struct Podcast {
        pub image_url: Option<String>,
        pub language:  Option<String>,
        pub link_url:  Option<String>,
        pub title:     String,
    }

    #[allow(dead_code)]
    #[derive(Insertable)]
    #[table_name = "podcast_feed_contents"]
    pub struct PodcastFeedContent {
        pub content:      String,
        pub podcast_id:   i64,
        pub retrieved_at: DateTime<Utc>,
        pub sha256_hash:  String,
    }

    #[derive(Insertable)]
    #[table_name = "podcast_feed_locations"]
    pub struct PodcastFeedLocation {
        pub discovered_at:     DateTime<Utc>,
        pub feed_url:          String,
        pub last_retrieved_at: DateTime<Utc>,
        pub podcast_id:        i64,
    }
}
