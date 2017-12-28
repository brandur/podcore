use errors::*;
use model;

use diesel::prelude::*;
use diesel::pg::PgConnection;
use futures::Stream;
use hyper;
use hyper::{Client, Uri};
use serde_xml_rs;
use std::str;
use std::str::FromStr;
use tokio_core::reactor::Core;

// serde_xml_rs starts inside the root element (`<rss>`), so that's what this struct represents.
#[derive(Debug, Deserialize)]
struct PodcastFeed {
    pub channel: PodcastFeedChannel,
}

#[derive(Debug, Deserialize)]
struct PodcastFeedChannel {
    pub description: String,
    pub explicit:    String, // "yes" instead of a bool

    #[serde(rename = "item")]
    pub items: Vec<PodcastFeedItem>,

    pub language:        String,
    pub last_build_date: String, // not iso8601 -- needs parsing

    #[serde(rename = "link")]
    pub link_url: Vec<String>,

    #[serde(rename = "atom10:link")]
    pub xx: String,

    pub thumbnail: Option<PodcastFeedMediaThumbnail>, // media:thumbnail
    pub title:     String,
}

#[derive(Debug, Deserialize)]
struct PodcastFeedItem {
    pub title: String,
}

#[derive(Debug, Deserialize)]
struct PodcastFeedMediaThumbnail {
    pub url: String,
}

pub struct DirectoryPodcastUpdater<'a> {
    pub client:      &'a Client<hyper::client::HttpConnector, hyper::Body>,
    pub conn:        &'a PgConnection,
    pub core:        &'a mut Core,
    pub dir_podcast: &'a mut model::DirectoryPodcast,
}

impl<'a> DirectoryPodcastUpdater<'a> {
    pub fn run(&mut self) -> Result<()> {
        self.conn
            .transaction::<_, Error, _>(|| self.run_inner())
            .chain_err(|| "Error in database transaction")
    }

    fn run_inner(&mut self) -> Result<()> {
        let raw_url = self.dir_podcast.feed_url.clone().unwrap();
        let feed_url = Uri::from_str(raw_url.as_str())
            .chain_err(|| format!("Error parsing feed URL: {}", raw_url))?;
        let res = self.core
            .run(self.client.get(feed_url))
            .chain_err(|| format!("Error fetching feed URL: {}", raw_url))?;
        println!("Response: {}", res.status());
        let body = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", raw_url))?;
        let feed: PodcastFeed = serde_xml_rs::from_str(str::from_utf8(&*body).unwrap())
            .chain_err(|| "Error deserializing feed")?;
        println!("Feed: {:?}", feed);

        self.dir_podcast.feed_url = None;
        self.dir_podcast
            .save_changes::<model::DirectoryPodcast>(&self.conn)
            .chain_err(|| "Error saving changes to directory podcast")?;
        Ok(())
    }
}

#[test]
fn test_run() {
    use diesel;
    use schema::directories_podcasts;
    use test_helpers;

    let conn = test_helpers::connection();
    let mut core = Core::new().unwrap();
    let client = Client::new(&core.handle());

    let itunes = model::Directory::itunes(&conn).unwrap();
    let mut dir_podcast = model::DirectoryPodcast {
        id:           0,
        directory_id: itunes.id,
        feed_url:     Some("http://feeds.feedburner.com/RoderickOnTheLine".to_owned()),
        podcast_id:   None,
        vendor_id:    "471418144".to_owned(),
    };
    diesel::insert_into(directories_podcasts::table)
        .values(&dir_podcast)
        .execute(&conn)
        .unwrap();

    let mut updater = DirectoryPodcastUpdater {
        client:      &client,
        conn:        &conn,
        core:        &mut core,
        dir_podcast: &mut dir_podcast,
    };
    updater.run().unwrap();
}

/*
struct PodcastUpdater {
    pub podcast: &Podcast,
}

impl PodcastUpdater {
    fn run(&self) {}
}
*/
