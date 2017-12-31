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
use quick_xml::events::{BytesText, Event};
use quick_xml::reader::Reader;
use schema::{episodes, podcast_feed_contents, podcasts};
use slog::Logger;
use std::io::BufRead;
use std::str;
use std::str::FromStr;
use time::precise_time_ns;
use tokio_core::reactor::Core;

pub struct DirectoryPodcastUpdater<'a> {
    pub conn:        &'a PgConnection,
    pub dir_podcast: &'a mut model::DirectoryPodcast,
    pub url_fetcher: &'a mut URLFetcher,
}

#[inline]
fn unit(ns: u64) -> (f64, &'static str) {
    if ns >= 1_000_000_000 {
        (1_000_000_000_f64, "s")
    } else if ns >= 1_000_000 {
        (1_000_000_f64, "ms")
    } else if ns >= 1_000 {
        (1_000_f64, "µs")
    } else {
        (1_f64, "ns")
    }
}

#[test]
fn test_unit() {
    assert_eq!((1_f64, "ns"), unit(2_u64));
    assert_eq!((1_000_f64, "µs"), unit(2_000_u64));
    assert_eq!((1_000_000_f64, "ms"), unit(2_000_000_u64));
    assert_eq!((1_000_000_000_f64, "s"), unit(2_000_000_000_u64));
}

#[inline]
fn log_timed<T, F>(log: &Logger, f: F) -> T
where
    F: FnOnce(&Logger) -> T,
{
    let start = precise_time_ns();
    info!(log, "Start");
    let res = f(&log);
    let elapsed = precise_time_ns() - start;
    let (div, unit) = unit(elapsed);
    info!(log, "Finish"; "elapsed" => format!("{:.*}{}", 3, ((elapsed as f64) / div), unit));
    res
}

pub struct RunResult {
    pub episodes: Vec<model::Episode>,
    pub podcast:  model::Podcast,
}

impl<'a> DirectoryPodcastUpdater<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.conn
                .transaction::<_, Error, _>(|| self.run_inner(&log))
                .chain_err(|| "Error in database transaction")
        })
    }

    fn content_hash(content: &Vec<u8>) -> String {
        let mut sha = Sha256::new();
        sha.input(content.clone().as_slice());
        sha.result_str()
    }

    fn convert_episodes(
        log: &Logger,
        podcast: &model::Podcast,
        episode_xmls: Vec<XMLEpisode>,
    ) -> Result<Vec<model::EpisodeIns>> {
        log_timed(&log.new(o!("step" => "convert_episodes")), |ref _log| {
            let mut episodes_ins = Vec::with_capacity(episode_xmls.len());
            for episode_xml in episode_xmls {
                episodes_ins.push(episode_xml
                    .to_model(&podcast)
                    .chain_err(|| format!("Failed to convert: {:?}", episode_xml))?);
            }
            Ok(episodes_ins)
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let raw_url = self.dir_podcast.feed_url.clone().unwrap();

        let body = log_timed(&log.new(o!("step" => "fetch_feed")), |ref _log| {
            self.url_fetcher.fetch(raw_url.as_str())
        })?;

        let sha256_hash = Self::content_hash(&body);
        let body_str = String::from_utf8(body).unwrap();

        let (podcast_xml, episode_xmls) = Self::parse_feed(&log, body_str.as_str())?;

        let podcast_ins = log_timed(&log.new(o!("step" => "convert_podcast")), |ref _log| {
            podcast_xml.to_model()
        })?;

        let podcast: model::Podcast =
            log_timed(&log.new(o!("step" => "insert_podcast")), |ref _log| {
                diesel::insert_into(podcasts::table)
                    .values(&podcast_ins)
                    .get_result(self.conn)
                    .chain_err(|| "Error inserting podcast")
            })?;

        let content_ins = model::PodcastFeedContentIns {
            content:      body_str,
            podcast_id:   podcast.id,
            retrieved_at: Utc::now(),
            sha256_hash:  sha256_hash,
        };
        log_timed(
            &log.new(o!("step" => "insert_podcast_feed_contents")),
            |ref _log| {
                diesel::insert_into(podcast_feed_contents::table)
                    .values(&content_ins)
                    .execute(self.conn)
                    .chain_err(|| "Error inserting podcast feed contents")
            },
        )?;

        let episodes_ins = Self::convert_episodes(&log, &podcast, episode_xmls)?;
        let episodes: Vec<model::Episode> =
            log_timed(&log.new(o!("step" => "insert_episodes")), |ref _log| {
                diesel::insert_into(episodes::table)
                    .values(&episodes_ins)
                    .get_results(self.conn)
                    .chain_err(|| "Error inserting podcast episodes")
            })?;

        log_timed(&log.new(o!("step" => "save_dir_podcast")), |ref _log| {
            self.dir_podcast.feed_url = None;
            self.dir_podcast
                .save_changes::<model::DirectoryPodcast>(&self.conn)
                .chain_err(|| "Error saving changes to directory podcast")
        })?;

        Ok(RunResult {
            episodes: episodes,
            podcast:  podcast,
        })
    }

    // The idea here is to produce a tolerant form of quick-xml's function that is tolerant to as
    // wide of a variety of possibly misencoded podcast feeds as possible.
    pub fn safe_unescape_and_decode<'b, B: BufRead>(
        log: &Logger,
        bytes: &BytesText<'b>,
        reader: &Reader<B>,
    ) -> String {
        // quick-xml's unescape might fail if it runs into an improperly encoded '&' with something
        // like this:
        //
        //     Some(Error(Escape("Cannot find \';\' after \'&\'", 486..1124) ...
        //
        // The idea here is that we try to unescape: If we can, great, continue to decode. If we
        // can't, then we just ignore the error (it goes to logs, but nothing else) and continue to
        // decode.
        //
        // Eventually this would probably be better served by completely reimplementing quick-xml's
        // unescaped so that we just don't balk when we see certain things that we know to be
        // problems. Just do as good of a job as possible in the same style as a web browser with
        // HTML.
        match bytes.unescaped() {
            Ok(bytes) => reader.decode(&*bytes).into_owned(),
            Err(e) => {
                error!(log, "Unescape failed"; "error" => e.description());
                reader.decode(&*bytes).into_owned()
            }
        }
    }

    fn parse_feed(log: &Logger, data: &str) -> Result<(XMLPodcast, Vec<XMLEpisode>)> {
        log_timed(&log.new(o!("step" => "parse_feed")), |ref _log| {
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
                            let val = Self::safe_unescape_and_decode(log, e, &reader);
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
                                    "guid" => episode.guid = Some(val),
                                    "itunes:explicit" => episode.explicit = Some(val == "yes"),
                                    "link" => episode.link_url = Some(val),
                                    "pubDate" => episode.published_at = Some(val),
                                    "title" => episode.title = Some(val),
                                    _ => (),
                                };
                            }
                        }
                    }
                    // Totally duplicated from "Text" above: modularize
                    Ok(Event::CData(ref e)) => {
                        if in_rss && in_channel {
                            let val = Self::safe_unescape_and_decode(log, e, &reader);
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
                                    "guid" => episode.guid = Some(val),
                                    "itunes:explicit" => episode.explicit = Some(val == "yes"),
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
                                        }
                                    }
                                }
                            } else {
                                // Either of these tags might be used for a media URL in podcast
                                // feeds that you see around.
                                if e.name() == b"enclosure" || e.name() == b"media:content" {
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
                                            }
                                            b"url" => {
                                                episode.media_url = Some(
                                                    String::from_utf8(kv.value.into_owned())
                                                        .unwrap()
                                                        .to_owned(),
                                                );
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
        })
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

#[cfg(test)]
mod tests {
    use mediators::*;
    use model;
    use schema::directories_podcasts;
    use test_helpers;

    use chrono::prelude::*;
    use std::collections::HashMap;

    #[test]
    fn test_ideal_feed() {
        let mut bootstrap = bootstrap(
            br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <language>en-US</language>
    <link>https://example.com/podcast</link>
    <media:thumbnail url="https://example.com/podcast-image-url.jpg"/>
    <title>Title</title>
    <item>
      <description><![CDATA[Item 1 description]]></description>
      <guid>1</guid>
      <itunes:explicit>yes</itunes:explicit>
      <media:content url="https://example.com/item-1" type="audio/mpeg"/>
      <pubDate>Sun, 24 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 1 Title</title>
    </item>
  </channel>
</rss>"#,
        );

        let mut med = DirectoryPodcastUpdater {
            conn:        &bootstrap.conn,
            dir_podcast: &mut bootstrap.dir_podcast,
            url_fetcher: &mut bootstrap.url_fetcher,
        };
        let res = med.run(&test_helpers::log()).unwrap();

        assert_ne!(0, res.podcast.id);
        assert_eq!(
            Some("https://example.com/podcast-image-url.jpg".to_owned()),
            res.podcast.image_url
        );
        assert_eq!(Some("en-US".to_owned()), res.podcast.language);
        assert_eq!(
            Some("https://example.com/podcast".to_owned()),
            res.podcast.link_url
        );
        assert_eq!("Title", res.podcast.title);

        assert_eq!(1, res.episodes.len());

        let episode = &res.episodes[0];
        assert_ne!(0, episode.id);
        assert_eq!(Some("Item 1 description".to_owned()), episode.description);
        assert_eq!(Some(true), episode.explicit);
        assert_eq!("1", episode.guid);
        assert_eq!(Some("audio/mpeg".to_owned()), episode.media_type);
        assert_eq!("https://example.com/item-1", episode.media_url);
        assert_eq!(res.podcast.id, episode.podcast_id);
        assert_eq!(
            Utc.ymd(2017, 12, 24).and_hms(21, 37, 32),
            episode.published_at
        );
    }

    #[test]
    fn test_minimal_feed() {
        let mut bootstrap = bootstrap(
            br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <title>Title</title>
    <item>
      <guid>1</guid>
      <media:content url="https://example.com/item-1" type="audio/mpeg"/>
      <pubDate>Sun, 24 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 1 Title</title>
    </item>
  </channel>
</rss>"#,
        );

        let mut med = DirectoryPodcastUpdater {
            conn:        &bootstrap.conn,
            dir_podcast: &mut bootstrap.dir_podcast,
            url_fetcher: &mut bootstrap.url_fetcher,
        };
        let res = med.run(&test_helpers::log()).unwrap();

        assert_eq!("Title", res.podcast.title);

        assert_eq!(1, res.episodes.len());

        let episode = &res.episodes[0];
        assert_eq!("1", episode.guid);
        assert_eq!("https://example.com/item-1", episode.media_url);
        assert_eq!(
            Utc.ymd(2017, 12, 24).and_hms(21, 37, 32),
            episode.published_at
        );
    }

    #[test]
    fn test_real_feed() {
        let mut bootstrap = bootstrap(include_bytes!("test_documents/feed_waking_up.xml"));

        let mut med = DirectoryPodcastUpdater {
            conn:        &bootstrap.conn,
            dir_podcast: &mut bootstrap.dir_podcast,
            url_fetcher: &mut bootstrap.url_fetcher,
        };
        med.run(&test_helpers::log()).unwrap();
    }

    //
    // Test helpers
    //

    // Encapsulates the structures that are needed for tests to run. One should only be obtained by
    // invoking bootstrap().
    struct TestBootstrap {
        conn:        PgConnection,
        dir_podcast: model::DirectoryPodcast,
        url_fetcher: URLFetcherStub,
    }

    pub struct URLFetcherStub {
        map: HashMap<&'static str, Vec<u8>>,
    }

    impl URLFetcher for URLFetcherStub {
        fn fetch(&mut self, url: &str) -> Result<Vec<u8>> {
            Ok(self.map.get(url).unwrap().clone())
        }
    }

    // Initializes the data required to get tests running.
    fn bootstrap(data: &[u8]) -> TestBootstrap {
        let conn = test_helpers::connection();
        let url = "https://example.com/feed.xml";

        let url_fetcher = URLFetcherStub {
            map: map!(url => data.to_vec()),
        };

        let itunes = model::Directory::itunes(&conn).unwrap();
        let dir_podcast_ins = model::DirectoryPodcastIns {
            directory_id: itunes.id,
            feed_url:     Some(url.to_owned()),
            podcast_id:   None,
            vendor_id:    "471418144".to_owned(),
        };
        let dir_podcast = diesel::insert_into(directories_podcasts::table)
            .values(&dir_podcast_ins)
            .get_result(&conn)
            .unwrap();

        TestBootstrap {
            conn:        conn,
            dir_podcast: dir_podcast,
            url_fetcher: url_fetcher,
        }
    }
}

/*
struct PodcastUpdater {
    pub podcast: &Podcast,
}

impl PodcastUpdater {
    fn run(&self) {}
}
*/
