use error_helpers;
use errors::*;
use http_requester::HttpRequester;
use mediators::common;
use model;
use model::insertable;
use schema;
use time_helpers;

use chrono::{DateTime, Utc};
use crypto::digest::Digest;
use crypto::sha2::Sha256;
use diesel;
use diesel::pg::PgConnection;
use diesel::pg::upsert::excluded;
use diesel::prelude::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use hyper::{Method, Request, StatusCode, Uri};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesText, Event};
use quick_xml::reader::Reader;
use regex::Regex;
use slog::Logger;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::prelude::*;
use std::str;
use std::str::FromStr;

pub struct Mediator<'a> {
    pub conn: &'a PgConnection,

    /// The mediator may skip some parts of processing if it detects that this
    /// exact feed has already been processed. Setting this value to `true`
    /// will skip this check and force all processing.
    pub disable_shortcut: bool,

    pub feed_url:       String,
    pub http_requester: &'a mut HttpRequester,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            match self.conn.transaction::<_, Error, _>(|| self.run_inner(log)) {
                Ok(r) => Ok(r),
                Err(e) => {
                    // As a latch ditch effort, update the timestamp indicating when the podcast
                    // was last retrieved in the case of a previously succeeding podcast that now
                    // fails. Otherwise, the crawler will attempt to process it over and over again
                    // because the entire transaction rolled back. We also store an exception
                    // record to ease debugging.
                    self.run_recovery(log, &e);

                    // Return the original error because it's going to be more useful.
                    Err(e)
                }
            }
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        // Try to find a more up-to-date URL than the one we've been given to retrieve
        // the same podcast at. This is helpful in case a directory's URL has
        // become out-of-date, or we've manually inserted a record to say supersede an
        // `http` url with an `https` URL.
        //
        // In the case where only the given URL exists, the same URL will simply be
        // re-selected and returned with no harm done.
        //
        // In the case where no URLs existed, the function just returns the same URL
        // back to us.
        let latest_url = self.select_latest_url(log, self.feed_url.as_str())?;

        // the "final URL" is one that might include a permanent redirect
        let (body, final_url) = self.fetch_feed(log, latest_url.as_str())?;

        let sha256_hash = content_hash(&body);

        let body = String::from_utf8(body).chain_err(|| "Error decoding to UTF-8")?;
        let (raw_podcast, raw_episodes) = Self::parse_feed(log, body.as_str())?;

        // Convert raw podcast data into something that's database compatible.
        let ins_podcast = Self::convert_podcast(log, &raw_podcast)?;

        // To make runs of this mediator idempotent we first check whether there's an
        // existing podcast record that has a URL that matches the one we're
        // processing.
        //
        // Note that we keep a record of all a podcast's historical URLs, so even if two
        // directories have two entries for the same podcast with different URLs (say
        // one is the newer version that old one 301s to), this should still
        // work.
        //
        // As a side effect, the upsert also sets the podcast's `last_retrieved_at`
        // field so that the crawler will know not to try and update it again
        // right away even if the mediator short circuits early because it
        // already existed.
        let podcast = self.upsert_podcast(log, &ins_podcast, final_url.as_str())?;

        // The final URL of the feed may be different than what a directory gave us.
        // Whatever it is, make sure that it's associated with the podcast.
        let location = self.upsert_podcast_feed_location(log, &podcast, final_url)?;

        // Check to see if we already have a content record that matches our calculated
        // hash. If so, that means that we've already successfully processed
        // this podcast in the past and can save ourselves some work by
        // skipping it.
        if !self.disable_shortcut && self.already_processed(log, &podcast, sha256_hash.as_str())? {
            info!(
                log,
                "Already processed identical content -- short circuiting"
            );
            return Ok(RunResult {
                episodes: None,
                location,
                podcast,
            });
        }

        // Store the podcast's raw content. Note that this is a relatively expensive
        // operation because a feed's body can be quite large.
        self.upsert_podcast_feed_content(log, &podcast, body.as_str(), sha256_hash)?;

        let ins_episodes = Self::convert_episodes(log, raw_episodes, &podcast)?;

        let episodes = self.upsert_episodes(log, &ins_episodes)?;

        // Now that we've had a successful run, remove any existing exceptions.
        self.delete_exception(log, &podcast)?;

        Ok(RunResult {
            episodes: Some(episodes),
            location,
            podcast,
        })
    }

    fn run_recovery(&mut self, log: &Logger, e: &Error) {
        let res = self.conn.transaction::<_, Error, _>(|| {
            let res = query_podcast(log, self.conn, self.feed_url.as_str())?;
            if let Some(podcast_id) = res {
                self.update_podcast_last_retrieved_at(log, podcast_id)?;
                self.upsert_exception(log, podcast_id, e)?;
            }
            Ok(())
        });

        if let Err(e) = res {
            error_helpers::print_error(log, &e);
            if let Err(inner_e) = error_helpers::report_error(log, &e) {
                error_helpers::print_error(log, &inner_e);
            }
        }
    }

    //
    // Steps
    //

    fn already_processed(
        &mut self,
        log: &Logger,
        podcast: &model::Podcast,
        sha256_hash: &str,
    ) -> Result<bool> {
        let matching_content_count: i64 = time_helpers::log_timed(
            &log.new(o!("step" => "query_podcast_feed_content")),
            |_log| {
                schema::podcast_feed_content::table
                    .filter(
                        schema::podcast_feed_content::podcast_id
                            .eq(podcast.id)
                            .and(schema::podcast_feed_content::sha256_hash.eq(sha256_hash)),
                    )
                    .count()
                    .first(self.conn)
            },
        )?;

        Ok(matching_content_count > 0)
    }

    fn convert_episodes(
        log: &Logger,
        raws: Vec<raw::Episode>,
        podcast: &model::Podcast,
    ) -> Result<Vec<insertable::Episode>> {
        time_helpers::log_timed(&log.new(o!("step" => "convert_episodes")), |log| {
            let mut guids: HashSet<String> = HashSet::new();
            let num_candidates = raws.len();
            let mut episodes = Vec::with_capacity(num_candidates);

            for raw in raws {
                match validate_episode(&raw, podcast)
                    .chain_err(|| format!("Failed to convert: {:?}", raw))?
                {
                    EpisodeOrInvalid::Valid(e) => {
                        if guids.contains(&e.guid) {
                            info!(log, "Found duplicate item <guid> -- discarding episode";
                                "guid" => e.guid);
                        } else {
                            guids.insert(e.guid.clone());
                            episodes.push(e);
                        }
                    }
                    EpisodeOrInvalid::Invalid {
                        message: m,
                        guid: g,
                    } => error!(log, "Invalid episode in feed: {}", m;
                            "episode-guid" => g, "podcast" => podcast.id,
                            "podcast_title" => podcast.title.clone()),
                }
            }
            info!(log, "Converted episodes";
                "num_valid" => episodes.len(), "num_invalid" => num_candidates - episodes.len());

            Ok(episodes)
        })
    }

    fn convert_podcast(log: &Logger, raw_podcast: &raw::Podcast) -> Result<insertable::Podcast> {
        time_helpers::log_timed(
            &log.new(o!("step" => "convert_podcast")),
            |_log| -> Result<insertable::Podcast> {
                match validate_podcast(raw_podcast)
                    .chain_err(|| format!("Failed to convert: {:?}", raw_podcast))?
                {
                    PodcastOrInvalid::Valid(p) => Ok(p),
                    PodcastOrInvalid::Invalid { message: m } => Err(m.into()),
                }
            },
        )
    }

    fn delete_exception(&mut self, log: &Logger, podcast: &model::Podcast) -> Result<()> {
        time_helpers::log_timed(&log.new(o!("step" => "delete_exception")), |_log| {
            diesel::delete(
                schema::podcast_exception::table
                    .filter(schema::podcast_exception::podcast_id.eq(podcast.id)),
            ).execute(self.conn)
                .chain_err(|| "Error deleting podcast exception")
        })?;
        Ok(())
    }

    fn fetch_feed(&mut self, log: &Logger, url: &str) -> Result<(Vec<u8>, String)> {
        let (status, body, final_url) =
            time_helpers::log_timed(&log.new(o!("step" => "fetch_feed")), |_log| {
                self.http_requester.execute(
                    log,
                    Request::new(Method::Get, Uri::from_str(url).map_err(Error::from)?),
                )
            })?;
        common::log_body_sample(log, status, &body);
        if status != StatusCode::Ok {
            let string = String::from_utf8_lossy(body.as_slice()).replace("\n", "");
            info!(log, "Body of errored request";
                "body" => string.as_str(), "status" => format!("{}", status));

            if status == StatusCode::NotFound {
                bail!(error::bad_request(
                    "That podcast doesn't seem to exist on the host's servers (404)."
                ));
            } else {
                bail!(error::bad_request(format!(
                    "Error fetching podcast feed. Host responded with status: {}",
                    status
                )));
            }
        }
        Ok((body, final_url))
    }

    fn parse_feed(log: &Logger, data: &str) -> Result<(raw::Podcast, Vec<raw::Episode>)> {
        time_helpers::log_timed(&log.new(o!("step" => "parse_feed")), |log| {
            let mut buf = Vec::new();
            let mut skip_buf = Vec::new();

            let mut reader = Reader::from_str(data);
            reader.trim_text(true).expand_empty_elements(true);

            loop {
                match reader.read_event(&mut buf) {
                    Ok(Event::Start(ref e)) => match e.name() {
                        b"rss" => {
                            return parse_rss(log, &mut reader);
                        }
                        name => reader.read_to_end(name, &mut skip_buf)?,
                    },
                    Ok(Event::End(_e)) => break,
                    Ok(Event::Eof) => break,
                    _ => {}
                }
                buf.clear();
            }

            Err("No <rss> tag found".into())
        })
    }

    fn select_latest_url(&self, log: &Logger, url: &str) -> Result<String> {
        // First see if we have any record for this podcast in the database already,
        // and if so, get the most up-to-date URL that we have recorded for it
        // (not necessarily the one that a directory gave us).
        //
        // This should be runnable in only one query, but Diesel's query builder
        // doesn't currently support referencing the same table twice in a
        // single query so we have to fall back to two separate queries.
        let podcast_id: Option<i64> = query_podcast(log, self.conn, url)?;

        if let Some(podcast_id) = podcast_id {
            let latest_url: Option<String> =
                time_helpers::log_timed(&log.new(o!("step" => "select_latest_url")), |_log| {
                    schema::podcast_feed_location::table
                        .filter(schema::podcast_feed_location::podcast_id.eq(podcast_id))
                        .select(schema::podcast_feed_location::feed_url)
                        .order(schema::podcast_feed_location::last_retrieved_at.desc())
                        .limit(1)
                        .first(self.conn)
                        .optional()
                        .chain_err(|| "Error selecting latest URL")
                })?;
            if let Some(latest_url) = latest_url {
                info!(log, "Found newer URL to use"; "url" => latest_url.as_str());
                return Ok(latest_url);
            }
        }

        Ok(url.to_owned())
    }

    fn upsert_episodes(
        &mut self,
        log: &Logger,
        ins_episodes: &[insertable::Episode],
    ) -> Result<Vec<model::Episode>> {
        time_helpers::log_timed(&log.new(o!("step" => "upsert_episodes")), |_log| {
            Ok(diesel::insert_into(schema::episode::table)
                .values(ins_episodes)
                .on_conflict((schema::episode::podcast_id, schema::episode::guid))
                .do_update()
                .set((
                    schema::episode::description.eq(excluded(schema::episode::description)),
                    schema::episode::explicit.eq(excluded(schema::episode::explicit)),
                    schema::episode::link_url.eq(excluded(schema::episode::link_url)),
                    schema::episode::media_type.eq(excluded(schema::episode::media_type)),
                    schema::episode::media_url.eq(excluded(schema::episode::media_url)),
                    schema::episode::podcast_id.eq(excluded(schema::episode::podcast_id)),
                    schema::episode::published_at.eq(excluded(schema::episode::published_at)),
                    schema::episode::title.eq(excluded(schema::episode::title)),
                ))
                .get_results(self.conn)
                .chain_err(|| "Error upserting podcast episodes")?)
        })
    }

    fn upsert_exception(&mut self, log: &Logger, podcast_id: i64, e: &Error) -> Result<()> {
        let ins_ex = insertable::PodcastException {
            errors: error_strings(e),
            podcast_id,
            occurred_at: Utc::now(),
        };

        time_helpers::log_timed(&log.new(o!("step" => "upsert_exception")), |_log| {
            diesel::insert_into(schema::podcast_exception::table)
                .values(&ins_ex)
                .on_conflict(schema::podcast_exception::podcast_id)
                .do_update()
                .set((
                    schema::podcast_exception::errors
                        .eq(excluded(schema::podcast_exception::errors)),
                    schema::podcast_exception::occurred_at
                        .eq(excluded(schema::podcast_exception::occurred_at)),
                ))
                .execute(self.conn)
                .chain_err(|| "Error upserting podcast exception")
        })?;
        Ok(())
    }

    fn upsert_podcast(
        &mut self,
        log: &Logger,
        ins_podcast: &insertable::Podcast,
        final_url: &str,
    ) -> Result<model::Podcast> {
        let podcast_id: Option<i64> = query_podcast(log, self.conn, final_url)?;

        if let Some(podcast_id) = podcast_id {
            info!(log, "Found existing podcast ID {}", podcast_id);
            time_helpers::log_timed(&log.new(o!("step" => "update_podcast")), |_log| {
                diesel::update(schema::podcast::table.filter(schema::podcast::id.eq(podcast_id)))
                    .set(ins_podcast)
                    .get_result(self.conn)
                    .chain_err(|| "Error updating podcast")
            })
        } else {
            info!(log, "No existing podcast found; inserting new");
            time_helpers::log_timed(&log.new(o!("step" => "insert_podcast")), |_log| {
                diesel::insert_into(schema::podcast::table)
                    .values(ins_podcast)
                    .get_result(self.conn)
                    .chain_err(|| "Error inserting podcast")
            })
        }
    }

    fn update_podcast_last_retrieved_at(&mut self, log: &Logger, podcast_id: i64) -> Result<()> {
        time_helpers::log_timed(
            &log.new(o!("step" => "update_podcast_last_retrieved_at")),
            |log| {
                info!(
                    log,
                    "Podcast update failed -- backing off to updating `last_retrieved_at`"
                );
                let num_rows_updated: usize =
                    diesel::update(
                        schema::podcast::table.filter(schema::podcast::id.eq(podcast_id)),
                    ).set(schema::podcast::last_retrieved_at.eq(Utc::now()))
                        .execute(self.conn)?;
                info!(log, "Update complete"; "num_rows_updated" => num_rows_updated);
                Ok(())
            },
        )
    }

    fn upsert_podcast_feed_content(
        &mut self,
        log: &Logger,
        podcast: &model::Podcast,
        body: &str,
        sha256_hash: String,
    ) -> Result<()> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(body.as_bytes())?;

        let content_ins = insertable::PodcastFeedContent {
            content_gzip: encoder.finish()?,
            podcast_id: podcast.id,
            retrieved_at: Utc::now(),
            sha256_hash,
        };

        time_helpers::log_timed(
            &log.new(o!("step" => "upsert_podcast_feed_content")),
            |_log| {
                diesel::insert_into(schema::podcast_feed_content::table)
                    .values(&content_ins)
                    .on_conflict((
                        schema::podcast_feed_content::podcast_id,
                        schema::podcast_feed_content::sha256_hash,
                    ))
                    .do_update()
                    .set(
                        schema::podcast_feed_content::retrieved_at
                            .eq(excluded(schema::podcast_feed_content::retrieved_at)),
                    )
                    .execute(self.conn)
                    .chain_err(|| "Error upserting podcast feed content")
            },
        )?;

        Ok(())
    }

    fn upsert_podcast_feed_location(
        &mut self,
        log: &Logger,
        podcast: &model::Podcast,
        final_url: String,
    ) -> Result<model::PodcastFeedLocation> {
        let location_ins = insertable::PodcastFeedLocation {
            first_retrieved_at: Utc::now(),
            feed_url:           final_url,
            last_retrieved_at:  Utc::now(),
            podcast_id:         podcast.id,
        };
        time_helpers::log_timed(
            &log.new(o!("step" => "upsert_podcast_feed_location")),
            |_log| {
                diesel::insert_into(schema::podcast_feed_location::table)
                    .values(&location_ins)
                    .on_conflict((
                        schema::podcast_feed_location::podcast_id,
                        schema::podcast_feed_location::feed_url,
                    ))
                    .do_update()
                    .set(
                        schema::podcast_feed_location::last_retrieved_at
                            .eq(excluded(schema::podcast_feed_location::last_retrieved_at)),
                    )
                    .get_result(self.conn)
                    .chain_err(|| "Error upserting podcast feed location")
            },
        )
    }
}

pub struct RunResult {
    /// Episodes that were inserted or updated by the mediator.
    ///
    /// This value is optional because if the mediator has detected that the
    /// feed has already been processed, it may skip processing episodes.
    pub episodes: Option<Vec<model::Episode>>,

    pub location: model::PodcastFeedLocation,
    pub podcast:  model::Podcast,
}

//
// Private macros
//

// A macro that shortens the number of lines of code required to validate that
// a field is present in a raw episode and t return an "invalid" enum record if
// it isn't. It's probably not a good idea to use macros for a fairly trivial
// operation like this, but I'll unwind them if this gets any more complicated.
macro_rules! require_episode_field {
    // Variation for a check without including an episode GUID.
    ($raw_field:expr, $message:expr) => {
        if $raw_field.is_none() {
            return Ok(EpisodeOrInvalid::Invalid {
                message: concat!("Missing ", $message, " from episode"),
                guid:    None,
            });
        }
    };

    // Variation for a check that does include an episode GUID. Use this wherever possible.
    ($raw_field:expr, $message:expr, $guid:expr) => {
        if $raw_field.is_none() {
            return Ok(EpisodeOrInvalid::Invalid {
                message: concat!("Missing ", $message, " from episode"),
                guid:    $guid,
            });
        }
    };
}

// See comment on require_episode_field! above.
macro_rules! require_podcast_field {
    ($raw_field:expr, $message:expr) => {
        if $raw_field.is_none() {
            return Ok(PodcastOrInvalid::Invalid {
                message: concat!("Missing ", $message, " from podcast"),
            });
        }
    };
}

//
// Private structs
//

/// Represents a regex find and replac rule that we use to coerce datetime
/// formats that are not technically valid RFC 2822 into ones that are and
/// which we can parse.
struct DateTimeReplaceRule {
    find:    Regex,
    replace: &'static str,
}

/// Represents the result of an attempt to turn a raw episode (`raw::episode`)
/// parsed from a third party data source into a valid one that we can insert
/// into our database. An insertable episode is returned if the minimum set of
/// required fields was found, otherwise a value indicating an invalid episode
/// is returned along with an error message.
///
/// Note that we use this instead of the `Result` type because running into an
/// invalid episode in a feed is something that we should expect with some
/// frequency in the real world and shouldn't produce an error. Instead, we
/// should note it and proceed to parse the episodes from the same field that
/// were valid.
enum EpisodeOrInvalid {
    Valid(insertable::Episode),
    Invalid {
        message: &'static str,
        guid:    Option<String>,
    },
}

/// See comment on `EpisodeOrInvalid`.
enum PodcastOrInvalid {
    Valid(insertable::Podcast),
    Invalid { message: &'static str },
}

/// Contains database record equivalents that have been parsed from third party
/// sources and which are not necessarily valid and therefore have more lax
/// constraints on some field compared to their model:: counterparts. Another
/// set of functions attempts to coerce these data types into insertable rows
/// and indicate that the data source is invalid if it's not possible.
mod raw {
    #[derive(Debug)]
    pub struct Episode {
        pub description:  Option<String>,
        pub explicit:     Option<bool>,
        pub guid:         Option<String>,
        pub link_url:     Option<String>,
        pub media_type:   Option<String>,
        pub media_url:    Option<String>,
        pub published_at: Option<String>,
        pub title:        Option<String>,
    }

    impl Episode {
        pub fn new() -> Episode {
            Episode {
                description:  None,
                explicit:     None,
                media_type:   None,
                media_url:    None,
                guid:         None,
                link_url:     None,
                published_at: None,
                title:        None,
            }
        }
    }

    #[derive(Debug)]
    pub struct Podcast {
        pub image_url: Option<String>,
        pub language:  Option<String>,
        pub link_url:  Option<String>,
        pub title:     Option<String>,
    }

    impl Podcast {
        pub fn new() -> Podcast {
            Podcast {
                image_url: None,
                language:  None,
                link_url:  None,
                title:     None,
            }
        }
    }
}

//
// Private functions
//

// Extracts a value from an XML attribute.
//
// If any content is found, it's trimmed of whitespace.
fn attribute_text<R: BufRead>(
    _log: &Logger,
    reader: &mut Reader<R>,
    attr: &Attribute,
) -> Result<String> {
    Ok(attr.unescape_and_decode_value(reader)
        .chain_err(|| "Error unescaping and decoding attribute")?
        .trim()
        .to_owned())
}

fn content_hash(content: &[u8]) -> String {
    let mut sha = Sha256::new();
    sha.input(content);
    sha.result_str()
}

// Extracts any text or content in cdata tags found within the reader's current
// element.
//
// If any content is found, it's trimmed of whitespace.
fn element_text<R: BufRead>(log: &Logger, reader: &mut Reader<R>) -> Result<String> {
    let mut content: Option<String> = None;
    let mut buf = Vec::new();
    let mut skip_buf = Vec::new();

    loop {
        match reader.read_event(&mut buf)? {
            Event::Start(element) => {
                reader.read_to_end(element.name(), &mut skip_buf)?;
            }
            Event::CData(ref e) => {
                let val = reader.decode(&*e).into_owned();
                content = Some(val);
            }
            Event::Text(ref e) => {
                let val = safe_unescape_and_decode(log, e, reader);
                content = Some(val);
            }
            Event::End(_) | Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    match content {
        Some(s) => Ok(s.trim().to_owned()),
        None => Err("No content found in tag".into()),
    }
}

fn parse_channel<R: BufRead>(
    log: &Logger,
    reader: &mut Reader<R>,
) -> Result<(raw::Podcast, Vec<raw::Episode>)> {
    let mut buf = Vec::new();
    let mut episodes: Vec<raw::Episode> = Vec::new();
    let mut podcast = raw::Podcast::new();
    let mut skip_buf = Vec::new();

    loop {
        match reader.read_event(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name() {
                b"item" => episodes.push(parse_item(log, reader)?),
                b"language" => podcast.language = Some(element_text(log, reader)?),
                b"link" => podcast.link_url = Some(element_text(log, reader)?),
                b"media:thumbnail" => {
                    for attr in e.attributes().with_checks(false) {
                        if let Ok(attr) = attr {
                            if attr.key == b"url" {
                                podcast.image_url = Some(attribute_text(log, reader, &attr)?);
                            }
                        }
                    }
                    reader.read_to_end(e.name(), &mut skip_buf)?;
                }
                b"title" => {
                    podcast.title = Some(element_text(log, reader)?);
                    info!(log, "Parsed title"; "title" => podcast.title.clone());
                }
                name => reader.read_to_end(name, &mut skip_buf)?,
            },
            Ok(Event::End(_e)) => break,
            Ok(Event::Eof) => return Err(Error::from("Unexpected EOF while parsing <channel> tag")),
            _ => {}
        }
        buf.clear();
    }

    Ok((podcast, episodes))
}

#[cfg_attr(feature = "cargo-clippy", allow(trivial_regex))]
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

    // Try to parse a valid datetime first, then fall back and start moving into
    // various known problem cases.
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

fn parse_rss<R: BufRead>(
    log: &Logger,
    reader: &mut Reader<R>,
) -> Result<(raw::Podcast, Vec<raw::Episode>)> {
    let mut buf = Vec::new();
    let mut skip_buf = Vec::new();
    let mut res: Option<(raw::Podcast, Vec<raw::Episode>)> = None;

    loop {
        match reader.read_event(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name() {
                b"channel" => {
                    if res.is_none() {
                        res = Some(parse_channel(log, reader)?);
                    } else {
                        info!(
                            log,
                            "Found channel tag, but already parsed a channel -- skipping"
                        );
                    }
                }
                name => reader.read_to_end(name, &mut skip_buf)?,
            },
            Ok(Event::End(_e)) => break,
            Ok(Event::Eof) => return Err(Error::from("Unexpected EOF while parsing <rss> tag")),
            _ => {}
        }
        buf.clear();
    }

    match res {
        Some(t) => Ok(t),
        None => Err("No channel tag found in feed".into()),
    }
}

fn parse_item<R: BufRead>(log: &Logger, reader: &mut Reader<R>) -> Result<raw::Episode> {
    let mut buf = Vec::new();
    let mut episode = raw::Episode::new();
    let mut skip_buf = Vec::new();

    loop {
        match reader.read_event(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name() {
                b"description" => episode.description = Some(element_text(log, reader)?),
                b"enclosure" | b"media:content" => {
                    for attr in e.attributes().with_checks(false) {
                        if let Ok(attr) = attr {
                            match attr.key {
                                b"type" => {
                                    episode.media_type = Some(attribute_text(log, reader, &attr)?);
                                }
                                b"url" => {
                                    episode.media_url = Some(attribute_text(log, reader, &attr)?);
                                }
                                _ => (),
                            }
                        }
                    }
                    reader.read_to_end(e.name(), &mut skip_buf)?;
                }
                b"guid" => episode.guid = Some(element_text(log, reader)?),
                b"itunes:explicit" => episode.explicit = Some(element_text(log, reader)? == "yes"),
                b"link" => episode.link_url = Some(element_text(log, reader)?),
                b"pubDate" => episode.published_at = Some(element_text(log, reader)?),
                b"title" => episode.title = Some(element_text(log, reader)?),
                name => reader.read_to_end(name, &mut skip_buf)?,
            },
            Ok(Event::End(_e)) => break,
            Ok(Event::Eof) => return Err(Error::from("Unexpected EOF while parsing <item> tag")),
            _ => {}
        }
        buf.clear();
    }

    Ok(episode)
}

// The idea here is to produce a tolerant form of quick-xml's function that is
// tolerant to as wide of a variety of possibly misencoded podcast feeds as
// possible.
pub fn safe_unescape_and_decode<'b, B: BufRead>(
    log: &Logger,
    bytes: &BytesText<'b>,
    reader: &Reader<B>,
) -> String {
    // quick-xml's unescape might fail if it runs into an improperly encoded '&'
    // with something like this:
    //
    //     Some(Error(Escape("Cannot find \';\' after \'&\'", 486..1124) ...
    //
    // The idea here is that we try to unescape: If we can, great, continue to
    // decode. If we can't, then we just ignore the error (it goes to logs, but
    // nothing else) and continue to decode.
    //
    // Eventually this would probably be better served by completely reimplementing
    // quick-xml's unescaped so that we just don't balk when we see certain
    // things that we know to be problems. Just do as good of a job as possible
    // in the same style as a web browser with HTML.
    match bytes.unescaped() {
        Ok(bytes) => reader.decode(&*bytes).into_owned(),
        Err(e) => {
            error!(log, "Unescape failed"; "error" => e.description());
            reader.decode(&*bytes).into_owned()
        }
    }
}

fn query_podcast(log: &Logger, conn: &PgConnection, url: &str) -> Result<Option<i64>> {
    time_helpers::log_timed(&log.new(o!("step" => "query_podcast")), |_log| {
        schema::podcast_feed_location::table
            .filter(schema::podcast_feed_location::feed_url.eq(url))
            .select(schema::podcast_feed_location::podcast_id)
            .first(conn)
            .optional()
            .chain_err(|| "Error selecting podcast ID")
    })
}

fn validate_episode(raw: &raw::Episode, podcast: &model::Podcast) -> Result<EpisodeOrInvalid> {
    require_episode_field!(raw.guid, "GUID");
    require_episode_field!(raw.media_url, "media URL", raw.guid.clone());
    require_episode_field!(raw.published_at, "publish date", raw.guid.clone());
    require_episode_field!(raw.title, "title", raw.guid.clone());

    Ok(EpisodeOrInvalid::Valid(insertable::Episode {
        description:  raw.description.clone(),
        explicit:     raw.explicit,
        guid:         raw.guid.clone().unwrap(),
        link_url:     raw.link_url.clone(),
        media_url:    raw.media_url.clone().unwrap(),
        media_type:   raw.media_type.clone(),
        podcast_id:   podcast.id,
        published_at: parse_date_time(raw.published_at.clone().unwrap().as_str())?,
        title:        raw.title.clone().unwrap(),
    }))
}

fn validate_podcast(raw: &raw::Podcast) -> Result<PodcastOrInvalid> {
    require_podcast_field!(raw.title, "title");

    Ok(PodcastOrInvalid::Valid(insertable::Podcast {
        image_url:         raw.image_url.clone(),
        language:          raw.language.clone(),
        last_retrieved_at: Utc::now(),
        link_url:          raw.link_url.clone(),
        title:             raw.title.clone().unwrap(),
    }))
}

//
// Tests
//

#[cfg(test)]
mod tests {
    use http_requester::HttpRequesterPassThrough;
    use mediators::podcast_updater::*;
    use model;
    use schema;
    use test_helpers;

    use chrono::prelude::*;
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;
    use time::Duration;

    #[test]
    fn test_feed_ideal() {
        let mut bootstrap = TestBootstrap::new(test_helpers::IDEAL_FEED);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // Podcast
        //

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

        // Podcast feed location
        //

        assert_eq!(res.podcast.id, res.location.podcast_id);
        assert_eq!(bootstrap.feed_url.to_owned(), res.location.feed_url);

        // Episode
        //

        let episodes = res.episodes.unwrap();
        assert_eq!(2, episodes.len());

        let episode = &episodes[0];
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
        assert_eq!("Item 1 Title", episode.title);

        let episode = &episodes[1];
        assert_ne!(0, episode.id);
        assert_eq!(Some("Item 2 description".to_owned()), episode.description);
        assert_eq!(Some(true), episode.explicit);
        assert_eq!("2", episode.guid);
        assert_eq!(Some("audio/mpeg".to_owned()), episode.media_type);
        assert_eq!("https://example.com/item-2", episode.media_url);
        assert_eq!(res.podcast.id, episode.podcast_id);
        assert_eq!(
            Utc.ymd(2017, 12, 23).and_hms(21, 37, 32),
            episode.published_at
        );
        assert_eq!("Item 2 Title", episode.title);
    }

    #[test]
    fn test_feed_minimal() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!("Title", res.podcast.title);

        let episodes = res.episodes.unwrap();
        assert_eq!(1, episodes.len());

        let episode = &episodes[0];
        assert_eq!("1", episode.guid);
        assert_eq!("https://example.com/item-1", episode.media_url);
        assert_eq!(
            Utc.ymd(2017, 12, 24).and_hms(21, 37, 32),
            episode.published_at
        );
        assert_eq!("Item 1 Title", episode.title);
    }

    #[test]
    fn test_idempotency_with_shortcut() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);

        {
            let (mut mediator, log) = bootstrap.mediator();
            let _res = mediator.run(&log).unwrap();
            let _res = mediator.run(&log).unwrap();
        }

        // Make sure that we ended up with one of everything
        assert_eq!(
            Ok(1),
            schema::episode::table.count().first(&*bootstrap.conn)
        );
        assert_eq!(
            Ok(1),
            schema::podcast::table.count().first(&*bootstrap.conn)
        );
        assert_eq!(
            Ok(1),
            schema::podcast_feed_content::table
                .count()
                .first(&*bootstrap.conn)
        );
        assert_eq!(
            Ok(1),
            schema::podcast_feed_location::table
                .count()
                .first(&*bootstrap.conn)
        );
    }

    #[test]
    fn test_idempotency_without_shortcut() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);

        {
            let (mut mediator, log) = bootstrap.mediator();

            // Disable the shortcut that checks to see if content has already been
            // processed so that we can verify idempotency even if the mediator
            // is doing a complete end-to-end run.
            mediator.disable_shortcut = true;

            let _res = mediator.run(&log).unwrap();
            let _res = mediator.run(&log).unwrap();
        }

        // Make sure that we ended up with one of everything
        assert_eq!(
            Ok(1),
            schema::episode::table.count().first(&*bootstrap.conn)
        );
        assert_eq!(
            Ok(1),
            schema::podcast::table.count().first(&*bootstrap.conn)
        );
        assert_eq!(
            Ok(1),
            schema::podcast_feed_content::table
                .count()
                .first(&*bootstrap.conn)
        );
        assert_eq!(
            Ok(1),
            schema::podcast_feed_location::table
                .count()
                .first(&*bootstrap.conn)
        );
    }

    #[test]
    fn test_prefer_latest_url() {
        // Establish one connection with an open transaction for which data will live
        // across this whole test.
        let conn = test_helpers::connection();

        let mut bootstrap = TestBootstrapWithConn::new(test_helpers::MINIMAL_FEED, &*conn);

        let res = {
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap()
        };

        // Running the updater above will have inserted one location record in the
        // database for our bootstra's default URL. Here we add a second one
        // which will be preferred on our next run. To ensure this, we cheat a
        // bit by setting its freshness timestamp a little into the future to
        // make sure that ordering comes out right.
        let location_ins = insertable::PodcastFeedLocation {
            first_retrieved_at: Utc::now(),
            feed_url:           "https://example.com/new-feed.xml".to_owned(),
            last_retrieved_at:  Utc::now() + Duration::minutes(10),
            podcast_id:         res.podcast.id,
        };
        diesel::insert_into(schema::podcast_feed_location::table)
            .values(&location_ins)
            .execute(&*conn)
            .unwrap();

        // Now run it again.
        {
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        };

        // Make sure that our new URL wasn't superseded by the default one that comes
        // from the bootstrap.
        let latest_url: String = schema::podcast_feed_location::table
            .filter(schema::podcast_feed_location::podcast_id.eq(res.podcast.id))
            .select(schema::podcast_feed_location::feed_url)
            .order(schema::podcast_feed_location::last_retrieved_at.desc())
            .limit(1)
            .first(&*conn)
            .unwrap();
        assert_eq!("https://example.com/new-feed.xml", latest_url.as_str());
    }

    #[test]
    fn test_feed_duplicated_guids() {
        let mut bootstrap = TestBootstrap::new(
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
    <item>
      <guid>1</guid><!-- Note this is the same as the first! -->
      <media:content url="https://example.com/item-1" type="audio/mpeg"/>
      <pubDate>Sun, 24 Dec 2017 21:37:32 +0000</pubDate>
      <title>Item 1 Duplicate</title>
    </item>
  </channel>
</rss>"#,
        );
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // There were two episodes, but because they had duplicate GUIDs, one was
        // discarded.
        let episodes = res.episodes.unwrap();
        assert_eq!(1, episodes.len());
    }

    #[test]
    fn test_feed_truncated() {
        let mut bootstrap = TestBootstrap::new(
            br#"
<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <language>en-US</language>"#,
        );
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);

        assert_eq!(true, res.is_err());
        let err = res.err().unwrap();
        let mut err_iter = err.iter();

        assert_eq!(
            "Unexpected EOF while parsing <channel> tag",
            err_iter.next().unwrap().to_string()
        );
        assert_eq!(true, err_iter.next().is_none());
    }

    #[test]
    fn test_feed_invalid() {
        let mut bootstrap = TestBootstrap::new(b"not a feed");
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log);

        assert_eq!(true, res.is_err());
        let err = res.err().unwrap();
        let mut err_iter = err.iter();

        assert_eq!("No <rss> tag found", err_iter.next().unwrap().to_string());
        assert_eq!(true, err_iter.next().is_none());
    }

    #[test]
    fn test_podcast_exception_creation() {
        // Establish one connection with an open transaction for which data will live
        // across this whole test.
        let conn = test_helpers::connection();

        // Run once to get a podcast record.
        let res = {
            let mut bootstrap = TestBootstrapWithConn::new(test_helpers::MINIMAL_FEED, &*conn);
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap()
        };

        assert_eq!(Ok(1), schema::podcast::table.count().first(&*conn));

        // Run again with an invalid feed. `feed_url` is consistent across all
        // `TestBootstrap` instances, so this will target the same podcast.
        {
            let mut bootstrap = TestBootstrapWithConn::new(b"not a feed", &*conn);
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log);
            assert_eq!(true, res.is_err());
        }

        let podcast_ex: model::PodcastException =
            schema::podcast_exception::table.first(&*conn).unwrap();
        assert_eq!(res.podcast.id, podcast_ex.podcast_id);
    }

    #[test]
    fn test_podcast_exception_removal() {
        // Establish one connection with an open transaction for which data will live
        // across this whole test.
        let conn = test_helpers::connection();

        // Run once to get a podcast record to target.
        let res = {
            let mut bootstrap = TestBootstrapWithConn::new(test_helpers::MINIMAL_FEED, &*conn);
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap()
        };

        let podcast_ex_ins = insertable::PodcastException {
            errors:      vec!["a".to_owned(), "b".to_owned()],
            podcast_id:  res.podcast.id,
            occurred_at: Utc::now(),
        };
        diesel::insert_into(schema::podcast_exception::table)
            .values(&podcast_ex_ins)
            .execute(&*conn)
            .unwrap();

        {
            let mut bootstrap = TestBootstrapWithConn::new(test_helpers::MINIMAL_FEED, &*conn);
            let (mut mediator, log) = bootstrap.mediator();

            // If the shortcut is taken the exception isn't removed. We need a full run.
            mediator.disable_shortcut = true;

            let _res = mediator.run(&log).unwrap();
        }

        // Exception count should now be back down to zero
        assert_eq!(
            Ok(0),
            schema::podcast_exception::table.count().first(&*conn)
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

        // Never forget how uselessly pedantic Rust programmers are. A "-0000" is
        // technically considered missing even though it's obvious to anyone on
        // Earth what should be done with it. Our special implementation
        // handles it, so test this case specifically.
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
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_8_4_play.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_99_percent_invisible.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_adventure_zone.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_atp.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_bike_shed.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_common_sense.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_communion_after_dark.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_eaten_by_a_grue.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_flop_house.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_hardcore_history.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_history_of_rome.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_planet_money.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_radiolab.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_road_work.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_roderick_on_the_line.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap = TestBootstrap::new(include_bytes!(
                "../test_documents/feed_song_exploder.\
                 xml"
            ));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_startup.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }

        {
            let mut bootstrap =
                TestBootstrap::new(include_bytes!("../test_documents/feed_waking_up.xml"));
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap();
        }
    }

    #[test]
    fn test_validate_episode() {
        let podcast = model::Podcast {
            id:                1,
            image_url:         None,
            language:          None,
            last_retrieved_at: Utc::now(),
            link_url:          None,
            title:             "Title".to_owned(),
        };

        {
            let raw = valid_raw_episode();
            match validate_episode(&raw, &podcast).unwrap() {
                EpisodeOrInvalid::Valid(p) => {
                    assert_eq!(raw.guid.unwrap(), p.guid);
                    assert_eq!(raw.media_url.unwrap(), p.media_url);
                    assert_eq!(
                        parse_date_time(raw.published_at.unwrap().as_str()).unwrap(),
                        p.published_at
                    );
                    assert_eq!(raw.title.unwrap(), p.title);
                }
                EpisodeOrInvalid::Invalid {
                    message: m,
                    guid: _,
                } => panic!("Unexpected invalid episode; message: {}", m),
            }
        }

        {
            let mut raw = valid_raw_episode();
            raw.guid = None;
            match validate_episode(&raw, &podcast).unwrap() {
                EpisodeOrInvalid::Valid(_) => panic!("Unexpected valid episode"),
                EpisodeOrInvalid::Invalid {
                    message: m,
                    guid: g,
                } => {
                    assert_eq!("Missing GUID from episode", m);
                    assert_eq!(None, g);
                }
            }
        }

        {
            let mut raw = valid_raw_episode();
            raw.media_url = None;
            match validate_episode(&raw, &podcast).unwrap() {
                EpisodeOrInvalid::Valid(_) => panic!("Unexpected valid episode"),
                EpisodeOrInvalid::Invalid {
                    message: m,
                    guid: g,
                } => {
                    assert_eq!("Missing media URL from episode", m);
                    assert_eq!(raw.guid, g);
                }
            }
        }

        {
            let mut raw = valid_raw_episode();
            raw.published_at = None;
            match validate_episode(&raw, &podcast).unwrap() {
                EpisodeOrInvalid::Valid(_) => panic!("Unexpected valid episode"),
                EpisodeOrInvalid::Invalid {
                    message: m,
                    guid: g,
                } => {
                    assert_eq!("Missing publish date from episode", m);
                    assert_eq!(raw.guid, g);
                }
            }
        }

        {
            let mut raw = valid_raw_episode();
            raw.title = None;
            match validate_episode(&raw, &podcast).unwrap() {
                EpisodeOrInvalid::Valid(_) => panic!("Unexpected valid episode"),
                EpisodeOrInvalid::Invalid {
                    message: m,
                    guid: g,
                } => {
                    assert_eq!("Missing title from episode", m);
                    assert_eq!(raw.guid, g);
                }
            }
        }
    }

    #[test]
    fn test_validate_podcast() {
        {
            let raw = valid_raw_podcast();
            match validate_podcast(&raw).unwrap() {
                PodcastOrInvalid::Valid(p) => assert_eq!(raw.title.unwrap(), p.title),
                PodcastOrInvalid::Invalid { message: m } => {
                    panic!("Unexpected invalid podcast; message: {}", m)
                }
            }
        }

        {
            let mut raw = valid_raw_podcast();
            raw.title = None;
            match validate_podcast(&raw).unwrap() {
                PodcastOrInvalid::Valid(_) => panic!("Unexpected valid podcast"),
                PodcastOrInvalid::Invalid { message: m } => {
                    assert_eq!("Missing title from podcast", m);
                }
            }
        }
    }

    //
    // Private types/functions
    //

    // Encapsulates the structures that are needed for tests to run. One should
    // only be obtained by invoking TestBootstrap::new().
    struct TestBootstrap {
        _common:        test_helpers::CommonTestBootstrap,
        conn:           PooledConnection<ConnectionManager<PgConnection>>,
        feed_url:       &'static str,
        log:            Logger,
        http_requester: HttpRequesterPassThrough,
    }

    impl TestBootstrap {
        fn new(data: &[u8]) -> TestBootstrap {
            TestBootstrap {
                _common:        test_helpers::CommonTestBootstrap::new(),
                conn:           test_helpers::connection(),
                feed_url:       "https://example.com/feed.xml",
                log:            test_helpers::log(),
                http_requester: HttpRequesterPassThrough {
                    data: Arc::new(data.to_vec()),
                },
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:             &*self.conn,
                    disable_shortcut: false,
                    feed_url:         self.feed_url.to_owned(),
                    http_requester:   &mut self.http_requester,
                },
                self.log.clone(),
            )
        }
    }

    // The suite runs on test transactions that are connection-specific, so this
    // version of `TestBootStrap` is useful for sharing state across multiple
    // bootstraps.
    struct TestBootstrapWithConn<'a> {
        _common:        test_helpers::CommonTestBootstrap,
        conn:           &'a PgConnection,
        feed_url:       &'static str,
        log:            Logger,
        http_requester: HttpRequesterPassThrough,
    }

    impl<'a> TestBootstrapWithConn<'a> {
        fn new(data: &[u8], conn: &'a PgConnection) -> TestBootstrapWithConn<'a> {
            TestBootstrapWithConn {
                _common:        test_helpers::CommonTestBootstrap::new(),
                conn:           conn,
                feed_url:       "https://example.com/feed.xml",
                log:            test_helpers::log(),
                http_requester: HttpRequesterPassThrough {
                    data: Arc::new(data.to_vec()),
                },
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:             self.conn,
                    disable_shortcut: false,
                    feed_url:         self.feed_url.to_owned(),
                    http_requester:   &mut self.http_requester,
                },
                self.log.clone(),
            )
        }
    }

    fn valid_raw_episode() -> raw::Episode {
        let mut raw = raw::Episode::new();
        raw.guid = Some("unique-guid".to_owned());
        raw.media_url = Some("https://example.com/podcast-url".to_owned());
        raw.published_at = Some("Sun, 24 Dec 2017 21:37:32 +0000".to_owned());
        raw.title = Some("Title".to_owned());
        raw
    }

    fn valid_raw_podcast() -> raw::Podcast {
        let mut raw = raw::Podcast::new();
        raw.title = Some("Title".to_owned());
        raw
    }
}
