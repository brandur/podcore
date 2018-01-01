use mediators::common;
use errors::*;
use model;

use chrono::{DateTime, Utc};
use crypto::digest::Digest;
use crypto::sha2::Sha256;
use diesel;
use diesel::prelude::*;
use diesel::pg::PgConnection;
use quick_xml::events::{BytesText, Event};
use quick_xml::reader::Reader;
use regex::Regex;
use schema::{episodes, podcast_feed_contents, podcasts};
use slog::Logger;
use std::io::BufRead;
use std::str;

pub struct DirectoryPodcastUpdater<'a> {
    pub conn:        &'a PgConnection,
    pub dir_podcast: &'a mut model::DirectoryPodcast,
    pub url_fetcher: &'a mut common::URLFetcher,
}

impl<'a> DirectoryPodcastUpdater<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.conn
                .transaction::<_, Error, _>(|| self.run_inner(&log))
                .chain_err(|| "Error in database transaction")
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let raw_url = self.dir_podcast.feed_url.clone().unwrap();

        let body = common::log_timed(&log.new(o!("step" => "fetch_feed")), |ref _log| {
            self.url_fetcher.fetch(raw_url.as_str())
        })?;

        let sha256_hash = content_hash(&body);
        let body_str = String::from_utf8(body).unwrap();

        let (podcast_xml, episode_xmls) = parse_feed(&log, body_str.as_str())?;

        let podcast_ins = common::log_timed(
            &log.new(o!("step" => "convert_podcast")),
            |ref _log| podcast_xml.to_model(),
        )?;

        let podcast: model::Podcast =
            common::log_timed(&log.new(o!("step" => "insert_podcast")), |ref _log| {
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
        common::log_timed(
            &log.new(o!("step" => "insert_podcast_feed_contents")),
            |ref _log| {
                diesel::insert_into(podcast_feed_contents::table)
                    .values(&content_ins)
                    .execute(self.conn)
                    .chain_err(|| "Error inserting podcast feed contents")
            },
        )?;

        let episodes_ins = convert_episodes(&log, &podcast, episode_xmls)?;
        let episodes: Vec<model::Episode> =
            common::log_timed(&log.new(o!("step" => "insert_episodes")), |ref _log| {
                diesel::insert_into(episodes::table)
                    .values(&episodes_ins)
                    .get_results(self.conn)
                    .chain_err(|| "Error inserting podcast episodes")
            })?;

        common::log_timed(&log.new(o!("step" => "save_dir_podcast")), |ref _log| {
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
}

pub struct RunResult {
    pub episodes: Vec<model::Episode>,
    pub podcast:  model::Podcast,
}

//
// Private types
//

// Represents a regex find and replac rule that we use to coerce datetime formats that are not
// technically valid RFC 2822 into ones that are and which we can parse.
struct DateTimeReplaceRule {
    find:    Regex,
    replace: &'static str,
}

enum EpisodeInsOrInvalid {
    Valid(model::EpisodeIns),
    Invalid {
        message: &'static str,
        guid:    Option<String>,
    },
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
    fn to_model(&self, podcast: &model::Podcast) -> Result<EpisodeInsOrInvalid> {
        if self.guid.is_none() {
            return Ok(EpisodeInsOrInvalid::Invalid {
                message: "Missing GUID from feed item",
                guid:    None,
            });
        }

        let guid = self.guid.clone().unwrap();
        if self.media_url.is_none() {
            return Ok(EpisodeInsOrInvalid::Invalid {
                message: "Missing media URL from feed item",
                guid:    Some(guid.clone()),
            });
        }
        if self.published_at.is_none() {
            return Ok(EpisodeInsOrInvalid::Invalid {
                message: "Missing publishing date from feed item",
                guid:    Some(guid.clone()),
            });
        }
        if self.title.is_none() {
            return Ok(EpisodeInsOrInvalid::Invalid {
                message: "Missing title from feed item",
                guid:    Some(guid.clone()),
            });
        }

        Ok(EpisodeInsOrInvalid::Valid(model::EpisodeIns {
            description:  self.description.clone(),
            explicit:     self.explicit.clone(),
            guid:         guid,
            link_url:     self.link_url.clone(),
            media_url:    self.media_url.clone().unwrap(),
            media_type:   self.media_type.clone(),
            podcast_id:   podcast.id,
            published_at: parse_date_time(self.published_at.clone().unwrap().as_str())?,
            title:        self.title.clone().unwrap(),
        }))
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

//
// Private functions
//

fn convert_episodes(
    log: &Logger,
    podcast: &model::Podcast,
    episode_xmls: Vec<XMLEpisode>,
) -> Result<Vec<model::EpisodeIns>> {
    common::log_timed(&log.new(o!("step" => "convert_episodes")), |ref log| {
        let num_candidates = episode_xmls.len();
        let mut episodes = Vec::with_capacity(num_candidates);

        for episode_xml in episode_xmls {
            match episode_xml
                .to_model(&podcast)
                .chain_err(|| format!("Failed to convert: {:?}", episode_xml))?
            {
                EpisodeInsOrInvalid::Valid(e) => episodes.push(e),
                EpisodeInsOrInvalid::Invalid {
                    message: m,
                    guid: g,
                } => error!(log, "Invalid episode in feed: {}", m;
                            "episode-guid" => g, "podcast" => podcast.id.clone(),
                            "podcast_title" => podcast.title.clone()),
            }
        }
        info!(log, "Converted episodes";
            "num_valid" => episodes.len(), "num_invalid" => num_candidates - episodes.len());

        Ok(episodes)
    })
}

fn content_hash(content: &Vec<u8>) -> String {
    let mut sha = Sha256::new();
    sha.input(content.clone().as_slice());
    sha.result_str()
}

fn parse_date_time(s: &str) -> Result<DateTime<Utc>> {
    lazy_static! {
        static ref RULES: Vec<DateTimeReplaceRule> = vec!(
            // The "-0000" timezone is not considered valid by true pedants
            DateTimeReplaceRule { find: Regex::new(r"-0000$").unwrap(), replace: "+0000", },

            // Like: "Mon, 27 Mar 2017 9:42:00 EST" (technically need two digits everywhere to be
            // valid)
            DateTimeReplaceRule { find: Regex::new(r"\b(?P<h>\d):").unwrap(), replace: "0$h:", },
        );
    }

    // Try to parse a valid datetime first, then fall back and start moving into various known
    // problem cases.
    match DateTime::parse_from_rfc2822(s) {
        Ok(d) => Ok(d.with_timezone(&Utc)),
        _ => {
            let mut s = s.to_owned();
            for r in RULES.iter() {
                s = r.find.replace(s.as_str(), r.replace).into_owned();
            }
            Ok(DateTime::parse_from_rfc2822(s.as_str())
                .chain_err(|| format!("Error parsing publishing date {:?} from feed item", s))?
                .with_timezone(&Utc))
        }
    }
}

fn parse_feed(log: &Logger, data: &str) -> Result<(XMLPodcast, Vec<XMLEpisode>)> {
    common::log_timed(&log.new(o!("step" => "parse_feed")), |ref _log| {
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
                        let val = safe_unescape_and_decode(log, e, &reader);
                        if !in_item {
                            match tag_name.clone().unwrap().as_str() {
                                "language" => podcast.language = Some(val),
                                "link" => podcast.link_url = Some(val),
                                "title" => {
                                    info!(log, "Parsed title"; "title" => val.clone());
                                    podcast.title = Some(val)
                                }
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
                        let val = safe_unescape_and_decode(log, e, &reader);
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

// The idea here is to produce a tolerant form of quick-xml's function that is tolerant to as wide
// of a variety of possibly misencoded podcast feeds as possible.
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
    // The idea here is that we try to unescape: If we can, great, continue to decode. If we can't,
    // then we just ignore the error (it goes to logs, but nothing else) and continue to decode.
    //
    // Eventually this would probably be better served by completely reimplementing quick-xml's
    // unescaped so that we just don't balk when we see certain things that we know to be problems.
    // Just do as good of a job as possible in the same style as a web browser with HTML.
    match bytes.unescaped() {
        Ok(bytes) => reader.decode(&*bytes).into_owned(),
        Err(e) => {
            error!(log, "Unescape failed"; "error" => e.description());
            reader.decode(&*bytes).into_owned()
        }
    }
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::directory_podcast_updater::*;
    use model;
    use schema::directories_podcasts;
    use test_helpers;

    use chrono::prelude::*;

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
        let mut mediator = bootstrap.mediator();
        let res = mediator.run(&test_helpers::log()).unwrap();

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
        let mut mediator = bootstrap.mediator();
        let res = mediator.run(&test_helpers::log()).unwrap();

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
    fn test_parse_date_time() {
        // Valid RFC 2822
        assert_eq!(
            Utc.ymd(2017, 12, 24).and_hms(21, 37, 32),
            parse_date_time("Sun, 24 Dec 2017 21:37:32 +0000").unwrap()
        );

        // Also valid -- check use of named timezones
        assert_eq!(
            FixedOffset::west(5 * 3600) // EST-0500
                .ymd(2017, 12, 24)
                .and_hms(21, 37, 32)
                .with_timezone(&Utc),
            parse_date_time("Sun, 24 Dec 2017 21:37:32 EST").unwrap()
        );

        // Never forget how uselessly pedantic Rust programmers are. A "-0000" is technically
        // considered missing even though it's obvious to anyone on Earth what should be done with
        // it. Our special implementation handles it, so test this case specifically.
        assert_eq!(
            Utc.ymd(2017, 12, 24).and_hms(21, 37, 32),
            parse_date_time("Sun, 24 Dec 2017 21:37:32 -0000").unwrap()
        );

        // Notice the truncated "0:" -- seen on Communion After Dark
        assert_eq!(
            FixedOffset::west(5 * 3600) // EST-0500
                .ymd(2017, 12, 24)
                .and_hms(0, 37, 32)
                .with_timezone(&Utc),
            parse_date_time("Sun, 24 Dec 2017 0:37:32 EST").unwrap()
        );
    }

    #[test]
    fn test_real_feed() {
        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_8_4_play.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!(
                "../test_documents/feed_99_percent_invisible.xml"
            ));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap =
                bootstrap(include_bytes!("../test_documents/feed_adventure_zone.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_atp.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_bike_shed.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap =
                bootstrap(include_bytes!("../test_documents/feed_common_sense.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!(
                "../test_documents/feed_communion_after_dark.xml"
            ));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap =
                bootstrap(include_bytes!("../test_documents/feed_eaten_by_a_grue.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_flop_house.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!(
                "../test_documents/feed_hardcore_history.xml"
            ));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap =
                bootstrap(include_bytes!("../test_documents/feed_history_of_rome.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap =
                bootstrap(include_bytes!("../test_documents/feed_planet_money.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_radiolab.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_road_work.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!(
                "../test_documents/feed_roderick_on_the_line.xml"
            ));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap =
                bootstrap(include_bytes!("../test_documents/feed_song_exploder.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_startup.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }

        {
            let mut bootstrap = bootstrap(include_bytes!("../test_documents/feed_waking_up.xml"));
            let mut mediator = bootstrap.mediator();
            mediator.run(&test_helpers::log()).unwrap();
        }
    }

    //
    // Private types/functions
    //

    // Encapsulates the structures that are needed for tests to run. One should only be obtained by
    // invoking bootstrap().
    struct TestBootstrap {
        conn:        PgConnection,
        dir_podcast: model::DirectoryPodcast,
        url_fetcher: common::URLFetcherStub,
    }

    impl TestBootstrap {
        fn mediator(&mut self) -> DirectoryPodcastUpdater {
            DirectoryPodcastUpdater {
                conn:        &self.conn,
                dir_podcast: &mut self.dir_podcast,
                url_fetcher: &mut self.url_fetcher,
            }
        }
    }

    // Initializes the data required to get tests running.
    fn bootstrap(data: &[u8]) -> TestBootstrap {
        let conn = test_helpers::connection();
        let url = "https://example.com/feed.xml";

        let url_fetcher = common::URLFetcherStub {
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
