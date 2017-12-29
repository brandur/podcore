use errors::*;
use model;

use diesel::prelude::*;
use diesel::pg::PgConnection;
use futures::Stream;
use hyper;
use hyper::{Client, Uri};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::str;
use std::str::FromStr;
use tokio_core::reactor::Core;

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

        Self::parse_feed(str::from_utf8(&*body).unwrap())?;

        self.dir_podcast.feed_url = None;
        self.dir_podcast
            .save_changes::<model::DirectoryPodcast>(&self.conn)
            .chain_err(|| "Error saving changes to directory podcast")?;
        Ok(())
    }

    fn parse_feed(data: &str) -> Result<()> {
        let mut podcast = XMLPodcast {
            image_url: None,
            language:  None,
            link_url:  None,
            title:     None,
        };

        let mut reader = Reader::from_str(data);
        reader.trim_text(true);

        let mut depth = 0;
        let mut in_channel = false;
        let mut in_item = false;
        let mut in_rss = false;
        let mut buf = Vec::new();
        let mut tag_name: Option<String> = None;

        loop {
            match reader.read_event(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    //if depth <= 2 {
                    //println!( "depth = {} e = {:?}", depth, str::from_utf8(e.name()).unwrap());
                    //}
                    match (depth, e.name()) {
                        (0, b"rss") => in_rss = true,
                        (1, b"channel") => in_channel = true,
                        (2, b"item") => in_item = true,
                        _ => (),
                    }
                    depth += 1;
                    tag_name = Some(str::from_utf8(e.name()).unwrap().to_owned());
                }
                Ok(Event::Text(ref e)) => {
                    if in_rss && in_channel {
                        if !in_item {
                            let val = e.unescape_and_decode(&reader).unwrap();
                            match tag_name.clone().unwrap().as_str() {
                                "language" => podcast.language = Some(val),
                                "link" => podcast.link_url = Some(val),
                                "title" => podcast.title = Some(val),
                                _ => (),
                            };
                        } else {
                        }
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    if in_channel {
                        if !in_item {
                            if e.name() == b"media:thumbnail" {
                                for r in e.attributes() {
                                    let kv = r.chain_err(|| "Error parsing XML attributes")?;
                                    println!("key = {:?}", str::from_utf8(kv.key).unwrap());
                                    if kv.key == b"url" {
                                        podcast.image_url = Some(
                                            String::from_utf8(kv.value.into_owned())
                                                .unwrap()
                                                .to_owned(),
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    match (depth, e.name()) {
                        (0, b"rss") => in_rss = false,
                        (1, b"channel") => in_channel = false,
                        (2, b"item") => in_item = false,
                        _ => (),
                    }
                    depth -= 1;
                    tag_name = None;
                }
                Ok(Event::Eof) => break,
                Err(e) => bail!("Error at position {}: {:?}", reader.buffer_position(), e),
                _ => (),
            };
        }
        buf.clear();
        println!("podcast = {:?}", podcast);

        Ok(())
    }
}

#[derive(Debug)]
struct XMLPodcast {
    pub image_url: Option<String>,
    pub language:  Option<String>,
    pub link_url:  Option<String>,
    pub title:     Option<String>,
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
