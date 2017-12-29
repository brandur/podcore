use errors::*;
use model;

use chrono::{DateTime, Utc};
use crypto::digest::Digest;
use crypto::sha2::Sha256;
use diesel;
use diesel::prelude::*;
use diesel::pg::PgConnection;
use futures::Stream;
use hyper;
use hyper::{Client, Uri};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use schema::{episodes, podcast_feed_contents, podcasts};
use slog::Logger;
use std::str;
use std::str::FromStr;
use tokio_core::reactor::Core;

pub struct URLFetcherStub<F: Fn(&str) -> Result<Vec<u8>>> {
    f: F,
}

impl<F: Fn(&str) -> Result<Vec<u8>>> URLFetcher for URLFetcherStub<F> {
    fn fetch(&mut self, raw_url: &str) -> Result<Vec<u8>> {
        (self.f)(raw_url)
    }
}

pub struct DirectoryPodcastUpdater<'a> {
    pub conn:        &'a PgConnection,
    pub dir_podcast: &'a mut model::DirectoryPodcast,
    pub log:         &'a Logger,
    pub url_fetcher: &'a mut URLFetcher,
}

impl<'a> DirectoryPodcastUpdater<'a> {
    pub fn run(&mut self) -> Result<()> {
        let log = self.log.new(o!("file" => file!()));

        info!(log, "Start");
        let res = self.conn
            .transaction::<_, Error, _>(|| self.run_inner())
            .chain_err(|| "Error in database transaction");
        info!(log, "Finish");
        res
    }

    fn content_hash(content: &Vec<u8>) -> String {
        let mut sha = Sha256::new();
        sha.input(content.clone().as_slice());
        sha.result_str()
    }

    fn run_inner(&mut self) -> Result<()> {
        let raw_url = self.dir_podcast.feed_url.clone().unwrap();
        let body = self.url_fetcher.fetch(raw_url.as_str())?;
        let sha256_hash = Self::content_hash(&body);
        let body_str = String::from_utf8(body).unwrap();

        let (podcast_xml, episode_xmls) = Self::parse_feed(body_str.as_str())?;
        let podcast_ins = podcast_xml.to_model()?;
        let podcast: model::Podcast = diesel::insert_into(podcasts::table)
            .values(&podcast_ins)
            .get_result(self.conn)
            .chain_err(|| "Error inserting podcast")?;

        let content_ins = model::PodcastFeedContentIns {
            content:      body_str,
            podcast_id:   podcast.id,
            retrieved_at: Utc::now(),
            sha256_hash:  sha256_hash,
        };
        diesel::insert_into(podcast_feed_contents::table)
            .values(&content_ins)
            .execute(self.conn)
            .chain_err(|| "Error inserting podcast feed contents")?;

        let mut episodes = Vec::with_capacity(episode_xmls.len());
        for episode_xml in episode_xmls {
            episodes.push(episode_xml
                .to_model(&podcast)
                .chain_err(|| format!("Failed to convert: {:?}", episode_xml))?);
        }
        diesel::insert_into(episodes::table)
            .values(&episodes)
            .execute(self.conn)
            .chain_err(|| "Error inserting podcast episodes")?;

        self.dir_podcast.feed_url = None;
        self.dir_podcast
            .save_changes::<model::DirectoryPodcast>(&self.conn)
            .chain_err(|| "Error saving changes to directory podcast")?;
        Ok(())
    }

    fn parse_feed(data: &str) -> Result<(XMLPodcast, Vec<XMLEpisode>)> {
        let mut episodes: Vec<XMLEpisode> = Vec::new();
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
                    match (depth, e.name()) {
                        (0, b"rss") => in_rss = true,
                        (1, b"channel") => in_channel = true,
                        (2, b"item") => {
                            in_item = true;

                            episodes.push(XMLEpisode {
                                description:  None,
                                explicit:     None,
                                media_type:   None,
                                media_url:    None,
                                guid:         None,
                                link_url:     None,
                                published_at: None,
                                title:        None,
                            });
                        }
                        _ => (),
                    }
                    depth += 1;
                    tag_name = Some(str::from_utf8(e.name()).unwrap().to_owned());
                }
                Ok(Event::Text(ref e)) => {
                    if in_rss && in_channel {
                        let val = e.unescape_and_decode(&reader).unwrap();
                        if !in_item {
                            match tag_name.clone().unwrap().as_str() {
                                "language" => podcast.language = Some(val),
                                "link" => podcast.link_url = Some(val),
                                "title" => podcast.title = Some(val),
                                _ => (),
                            };
                        } else {
                            let episode = episodes.last_mut().unwrap();
                            match tag_name.clone().unwrap().as_str() {
                                "description" => episode.description = Some(val),
                                "explicit" => episode.explicit = Some(val == "yes"),
                                "guid" => episode.guid = Some(val),
                                "link" => episode.link_url = Some(val),
                                "pubDate" => episode.published_at = Some(val),
                                "title" => episode.title = Some(val),
                                _ => (),
                            };
                        }
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    if in_channel {
                        if !in_item {
                            if e.name() == b"media:thumbnail" {
                                for r in e.attributes() {
                                    let kv = r.chain_err(|| "Error parsing XML attributes")?;
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
                        } else {
                            if e.name() == b"media:content" {
                                let episode = episodes.last_mut().unwrap();
                                for r in e.attributes() {
                                    let kv = r.chain_err(|| "Error parsing XML attributes")?;
                                    match kv.key {
                                        b"type" => {
                                            episode.media_type = Some(
                                                String::from_utf8(kv.value.into_owned())
                                                    .unwrap()
                                                    .to_owned(),
                                            );
                                            break;
                                        }
                                        b"url" => {
                                            episode.media_url = Some(
                                                String::from_utf8(kv.value.into_owned())
                                                    .unwrap()
                                                    .to_owned(),
                                            );
                                            break;
                                        }
                                        _ => (),
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
        //println!("podcast = {:?}", podcast);
        //println!("episodes = {:?}", episodes);

        Ok((podcast, episodes))
    }
}

pub trait URLFetcher {
    fn fetch(&mut self, raw_url: &str) -> Result<Vec<u8>>;
}

/*
let mut core = Core::new().unwrap();
let client = Client::new(&core.handle());
let mut url_fetcher = URLFetcherLive {
    client: &client,
    core:   &mut core,
};
*/
pub struct URLFetcherLive<'a> {
    client: &'a Client<hyper::client::HttpConnector, hyper::Body>,
    core:   &'a mut Core,
}

impl<'a> URLFetcher for URLFetcherLive<'a> {
    fn fetch(&mut self, raw_url: &str) -> Result<Vec<u8>> {
        let feed_url =
            Uri::from_str(raw_url).chain_err(|| format!("Error parsing feed URL: {}", raw_url))?;
        let res = self.core
            .run(self.client.get(feed_url))
            .chain_err(|| format!("Error fetching feed URL: {}", raw_url))?;
        let body = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", raw_url))?;
        Ok((*body).to_vec())
    }
}

#[derive(Debug)]
struct XMLPodcast {
    pub image_url: Option<String>,
    pub language:  Option<String>,
    pub link_url:  Option<String>,
    pub title:     Option<String>,
}

impl XMLPodcast {
    fn to_model(&self) -> Result<model::PodcastIns> {
        Ok(model::PodcastIns {
            image_url: self.image_url.clone(),
            language:  self.language.clone(),
            link_url:  self.link_url.clone(),
            title:     self.title
                .clone()
                .chain_err(|| "Error extracting title from podcast feed")?,
        })
    }
}

#[derive(Debug)]
struct XMLEpisode {
    pub description:  Option<String>,
    pub explicit:     Option<bool>,
    pub guid:         Option<String>,
    pub link_url:     Option<String>,
    pub media_type:   Option<String>,
    pub media_url:    Option<String>,
    pub published_at: Option<String>,
    pub title:        Option<String>,
}

impl XMLEpisode {
    fn to_model(&self, podcast: &model::Podcast) -> Result<model::EpisodeIns> {
        Ok(model::EpisodeIns {
            description:  self.description.clone(),
            explicit:     self.explicit.clone(),
            guid:         self.guid
                .clone()
                .chain_err(|| "Missing GUID from feed item")?,
            link_url:     self.link_url.clone(),
            media_url:    self.media_url
                .clone()
                .chain_err(|| "Missing media URL from feed item")?,
            media_type:   self.media_type.clone(),
            podcast_id:   podcast.id,
            published_at: parse_date_time(
                self.published_at
                    .clone()
                    .chain_err(|| "Missing publishing date from feed item")?
                    .as_str(),
            )?,
            title:        self.title
                .clone()
                .chain_err(|| "Missing title from feed item")?,
        })
    }
}

fn parse_date_time(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc2822(s)
        .chain_err(|| format!("Error parsing publishing date {:?} from feed item", s))?
        .with_timezone(&Utc))
}

#[test]
fn test_run() {
    use schema::directories_podcasts;
    use test_helpers;

    let conn = test_helpers::connection();
    let log = test_helpers::log();

    let mut url_fetcher = URLFetcherStub {
        f: (|u| match u {
            "http://feeds.feedburner.com/RoderickOnTheLine" => {
                Ok(include_bytes!("test_documents/feed.xml").to_vec())
            }
            _ => bail!("Unexpected url: {}", u),
        }),
    };

    let itunes = model::Directory::itunes(&conn).unwrap();
    let dir_podcast_ins = model::DirectoryPodcastIns {
        directory_id: itunes.id,
        feed_url:     Some("http://feeds.feedburner.com/RoderickOnTheLine".to_owned()),
        podcast_id:   None,
        vendor_id:    "471418144".to_owned(),
    };
    let mut dir_podcast = diesel::insert_into(directories_podcasts::table)
        .values(&dir_podcast_ins)
        .get_result(&conn)
        .unwrap();

    let mut updater = DirectoryPodcastUpdater {
        conn:        &conn,
        dir_podcast: &mut dir_podcast,
        log:         &log,
        url_fetcher: &mut url_fetcher,
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
