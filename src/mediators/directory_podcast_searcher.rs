use errors::*;
use mediators::common;
use model;
use model::insertable;
use url_fetcher::URLFetcher;

use chrono::Utc;
use diesel;
use diesel::prelude::*;
use diesel::pg::PgConnection;
use diesel::pg::upsert::excluded;
use hyper::{Method, Request, StatusCode, Uri};
use schema::{directories_podcasts, directories_podcasts_directory_searches, directory_searches,
             podcast_feed_locations, podcasts};
use serde_json;
use slog::Logger;
use std::collections::HashMap;
use std::str::FromStr;
use time::Duration;
use url::form_urlencoded;

pub struct DirectoryPodcastSearcher<'a> {
    pub conn:        &'a PgConnection,
    pub query:       String,
    pub url_fetcher: &'a mut URLFetcher,
}

impl<'a> DirectoryPodcastSearcher<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.conn
                .transaction::<_, Error, _>(|| self.run_inner(&log))
                .chain_err(|| "Error in database transaction")
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let directory = model::Directory::itunes(&self.conn)?;
        let directory_search = match self.select_directory_search(&log, &directory)? {
            Some(search) => {
                // The cache is fresh. Retrieve directory podcasts and search results, then return
                // early.
                if search.retrieved_at > Utc::now() - Duration::hours(1) {
                    info!(log, "Query cached and fresh";
                        "retrieved_at" => search.retrieved_at.to_rfc3339());

                    let (directory_podcasts, joins) = self.select_cached_results(&log, &search)?;
                    return Ok(RunResult {
                        cached:             true,
                        directory_podcasts: directory_podcasts,
                        directory_search:   search,
                        joins:              joins,
                    });
                }

                info!(log, "Query cached, but stale";
                    "retrieved_at" => search.retrieved_at.to_rfc3339());

                // The cache is stale. We can reuse the search row (after updating its retrieval
                // time), but we'll redo all the normal work below.
                self.update_directory_search(&log, &search)?
            }
            None => {
                info!(log, "Query not cached");
                self.insert_directory_search(&log, &directory)?
            }
        };
        let body = self.fetch_results(&log)?;
        let results = Self::parse_results(&log, body.as_slice())?;
        let directory_podcasts = self.upsert_directory_podcasts(&log, &results, &directory)?;
        let joins = self.refresh_joins(&log, &directory_search, &directory_podcasts)?;
        Ok(RunResult {
            cached:             false,
            directory_podcasts: directory_podcasts,
            directory_search:   directory_search,
            joins:              joins,
        })
    }

    //
    // Steps
    //

    fn fetch_results(&mut self, log: &Logger) -> Result<Vec<u8>> {
        let encoded: String = form_urlencoded::Serializer::new(String::new())
            .append_pair("media", "podcast")
            .append_pair("term", self.query.as_str())
            .finish();
        info!(log, "Encoded query"; "query" => encoded.clone());

        let (status, body, _final_url) =
            common::log_timed(&log.new(o!("step" => "fetch_results")), |ref _log| {
                self.url_fetcher.fetch(Request::new(
                    Method::Get,
                    Uri::from_str(format!("https://itunes.apple.com/search?{}", encoded).as_str())
                        .map_err(Error::from)?,
                ))
            })?;
        common::log_body_sample(log, status, &body);

        if status != StatusCode::Ok {
            bail!(
                "Unexpected status while fetching search results: {}",
                status
            )
        }

        Ok(body)
    }

    fn insert_directory_search(
        &mut self,
        log: &Logger,
        directory: &model::Directory,
    ) -> Result<model::DirectorySearch> {
        common::log_timed(
            &log.new(o!("step" => "insert_directory_search")),
            |ref _log| {
                diesel::insert_into(directory_searches::table)
                    .values(&insertable::DirectorySearch {
                        directory_id: directory.id,
                        query:        self.query.clone(),
                        retrieved_at: Utc::now(),
                    })
                    .get_result(self.conn)
                    .chain_err(|| "Error inserting directory podcast")
            },
        )
    }

    fn parse_results(log: &Logger, data: &[u8]) -> Result<Vec<SearchResult>> {
        let wrapper: SearchResultWrapper =
            common::log_timed(&log.new(o!("step" => "parse_results")), |ref _log| {
                serde_json::from_slice(data).chain_err(|| "Error parsing search results JSON")
            })?;
        info!(log, "Parsed results"; "count" => wrapper.results.len());
        Ok(wrapper.results)
    }

    fn refresh_joins(
        &mut self,
        log: &Logger,
        directory_search: &model::DirectorySearch,
        directory_podcasts: &Vec<model::DirectoryPodcast>,
    ) -> Result<Vec<model::DirectoryPodcastDirectorySearch>> {
        common::log_timed(&log.new(o!("step" => "delete_joins")), |ref _log| {
            diesel::delete(
                directories_podcasts_directory_searches::table.filter(
                    directories_podcasts_directory_searches::directory_searches_id
                        .eq(directory_search.id),
                ),
            ).execute(self.conn)
                .chain_err(|| "Error selecting directory podcast")
        })?;

        let ins_joins: Vec<insertable::DirectoryPodcastDirectorySearch> = directory_podcasts
            .iter()
            .map(|ref p| insertable::DirectoryPodcastDirectorySearch {
                directories_podcasts_id: p.id,
                directory_searches_id:   directory_search.id,
            })
            .collect();

        common::log_timed(&log.new(o!("step" => "insert_joins")), |ref _log| {
            diesel::insert_into(directories_podcasts_directory_searches::table)
                .values(&ins_joins)
                .get_results(self.conn)
                .chain_err(|| "Error inserting directory podcast")
                as Result<Vec<model::DirectoryPodcastDirectorySearch>>
        })
    }

    fn select_cached_results(
        &mut self,
        log: &Logger,
        search: &model::DirectorySearch,
    ) -> Result<
        (
            Vec<model::DirectoryPodcast>,
            Vec<model::DirectoryPodcastDirectorySearch>,
        ),
    > {
        let joins = common::log_timed(&log.new(o!("step" => "select_joins")), |ref _log| {
            directories_podcasts_directory_searches::table
                .filter(
                    directories_podcasts_directory_searches::directory_searches_id.eq(search.id),
                )
                .order(directories_podcasts_directory_searches::id)
                .load::<model::DirectoryPodcastDirectorySearch>(self.conn)
                .chain_err(|| "Error loading joins")
        })?;

        let directory_podcasts = common::log_timed(
            &log.new(o!("step" => "select_directory_podcasts")),
            |ref _log| {
                directories_podcasts::table
                    .filter(
                        directories_podcasts::id.eq_any(
                            joins
                                .iter()
                                .map(|j| j.directories_podcasts_id)
                                .collect::<Vec<i64>>(),
                        ),
                    )
                    .load::<model::DirectoryPodcast>(self.conn)
                    .chain_err(|| "Error loading directory podcasts")
            },
        )?;

        Ok((directory_podcasts, joins))
    }

    fn select_directory_search(
        &mut self,
        log: &Logger,
        directory: &model::Directory,
    ) -> Result<Option<model::DirectorySearch>> {
        common::log_timed(
            &log.new(o!("step" => "select_directory_search")),
            |ref _log| {
                directory_searches::table
                    .filter(directory_searches::directory_id.eq(directory.id))
                    .filter(directory_searches::query.eq(self.query.as_str()))
                    .first(self.conn)
                    .optional()
                    .chain_err(|| "Error selecting directory podcast")
            },
        )
    }

    fn update_directory_search(
        &mut self,
        log: &Logger,
        search: &model::DirectorySearch,
    ) -> Result<model::DirectorySearch> {
        common::log_timed(
            &log.new(o!("step" => "update_directory_search")),
            |ref _log| {
                diesel::update(
                    directory_searches::table.filter(directory_searches::id.eq(search.id)),
                ).set(directory_searches::retrieved_at.eq(Utc::now()))
                    .get_result(self.conn)
                    .chain_err(|| "Error updating search retrieval time")
            },
        )
    }

    fn upsert_directory_podcasts(
        &mut self,
        log: &Logger,
        results: &Vec<SearchResult>,
        directory: &model::Directory,
    ) -> Result<Vec<model::DirectoryPodcast>> {
        let mut ins_podcasts: Vec<insertable::DirectoryPodcast> = results
            .iter()
            .filter(|ref p| p.feed_url.is_some())
            .map(|ref p| insertable::DirectoryPodcast {
                directory_id: directory.id,
                feed_url:     p.feed_url.clone().unwrap(),
                podcast_id:   None,
                title:        p.collection_name.clone(),
                vendor_id:    p.collection_id.to_string(),
            })
            .collect();

        // Retrieve any IDs for podcasts that are already in database and have a previous location
        // that matches one returned by our directory.
        let podcast_id_tuples: Vec<(String, i64)> = podcasts::table
            .inner_join(podcast_feed_locations::table)
            .filter(
                podcast_feed_locations::feed_url
                    .eq_any(ins_podcasts.iter().map(|ref p| p.feed_url.clone())),
            )
            .select((podcast_feed_locations::feed_url, podcasts::id))
            .load(self.conn)?;

        // Maps feed URLs to podcast IDs.
        let podcast_id_map: HashMap<_, _> = podcast_id_tuples.into_iter().collect();

        for mut ins_podcast in &mut ins_podcasts {
            ins_podcast.podcast_id = podcast_id_map.get(&ins_podcast.feed_url).cloned();
        }

        common::log_timed(
            &log.new(o!("step" => "upsert_directory_podcasts")),
            |ref _log| {
                Ok(diesel::insert_into(directories_podcasts::table)
                    .values(&ins_podcasts)
                    .on_conflict((
                        directories_podcasts::directory_id,
                        directories_podcasts::vendor_id,
                    ))
                    .do_update()
                    .set((
                        directories_podcasts::feed_url.eq(excluded(directories_podcasts::feed_url)),
                        directories_podcasts::title.eq(excluded(directories_podcasts::title)),
                    ))
                    .get_results(self.conn)
                    .chain_err(|| "Error upserting directory podcasts")?)
            },
        )
    }
}

pub struct RunResult {
    pub cached:             bool,
    pub directory_podcasts: Vec<model::DirectoryPodcast>,
    pub directory_search:   model::DirectorySearch,
    pub joins:              Vec<model::DirectoryPodcastDirectorySearch>,
}

//
// Private types
//

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    artwork_url_100: String,
    collection_id:   u32,
    collection_name: String,

    // Set as optional because believe it or not, iTunes will occasionally respond with podcasts
    // that have no feed URL. We'll filter these on our side.
    feed_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResultWrapper {
    result_count: usize,
    results:      Vec<SearchResult>,
}

//
// Private functions
//

//
// Tests
//

#[cfg(test)]
mod tests {
    use mediators::directory_podcast_searcher::*;
    use test_helpers;
    use url_fetcher::URLFetcherPassThrough;

    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;

    #[test]
    fn test_new_search() {
        let mut bootstrap = TestBootstrap::new(DIRECTORY_SEARCH_RESULTS);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(false, res.cached);

        // Directory podcast
        assert_eq!(1, res.directory_podcasts.len());
        let directory_podcast = &res.directory_podcasts[0];
        assert_eq!("https://example.com/feed.xml", directory_podcast.feed_url);
        assert_eq!("Example Podcast", directory_podcast.title);
        assert_eq!("123", directory_podcast.vendor_id);

        // Directory search
        assert_eq!(DIRECTORY_SEARCH_QUERY, res.directory_search.query);

        // Join row
        assert_eq!(1, res.joins.len());
        let join = &res.joins[0];
        assert_eq!(directory_podcast.id, join.directories_podcasts_id);
        assert_eq!(res.directory_search.id, join.directory_searches_id);
    }

    #[test]
    fn test_cached_search_fresh() {
        let mut bootstrap = TestBootstrap::new(DIRECTORY_SEARCH_RESULTS);

        // First run (inserts original results)
        let _res = {
            let (mut mediator, log) = bootstrap.mediator();
            mediator.run(&log).unwrap()
        };

        // Second run (hits cached results)
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(true, res.cached);
    }

    #[test]
    fn test_cached_search_stale() {
        // First run (inserts original results)
        {
            let mut bootstrap = TestBootstrap::new(DIRECTORY_SEARCH_RESULTS);

            let res = {
                let (mut mediator, log) = bootstrap.mediator();
                mediator.run(&log).unwrap()
            };

            // Update the search's timestamp, thereby invalidating the cache.
            diesel::update(
                directory_searches::table
                    .filter(directory_searches::id.eq(res.directory_search.id)),
            ).set(directory_searches::retrieved_at.eq(Utc::now() - Duration::days(1)))
                .execute(&*bootstrap.conn)
                .unwrap();
        }

        // Second run. Notice that we're using the alternate set of results.
        let mut bootstrap = TestBootstrap::new(DIRECTORY_SEARCH_RESULTS_ALT);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        assert_eq!(false, res.cached);

        // Directory podcast
        assert_eq!(1, res.directory_podcasts.len());
        let directory_podcast = &res.directory_podcasts[0];
        assert_eq!(
            "https://example.com/alternate.xml",
            directory_podcast.feed_url
        );
        assert_eq!("Alternate Podcast", directory_podcast.title);
        assert_eq!("124", directory_podcast.vendor_id);

        // Directory search
        assert_eq!(DIRECTORY_SEARCH_QUERY, res.directory_search.query);

        // Join row
        assert_eq!(1, res.joins.len());
        let join = &res.joins[0];
        assert_eq!(directory_podcast.id, join.directories_podcasts_id);
        assert_eq!(res.directory_search.id, join.directory_searches_id);
    }

    #[test]
    fn test_results_deserialization() {
        let encoded = include_bytes!("../test_documents/itunes_search.json");
        let decoded: SearchResultWrapper = serde_json::from_slice(encoded).unwrap();
        assert_ne!(0, decoded.result_count);
        assert_eq!(decoded.result_count, decoded.results.len());
    }

    //
    // Private types/functions
    //
    const DIRECTORY_SEARCH_QUERY: &str = "History";

    const DIRECTORY_SEARCH_RESULTS: &[u8] = br#"{
  "resultCount": 1,
  "results": [
    {
      "artworkUrl100": "https://example.com/artwork_100x100.jpg",
      "collectionId": 123,
      "collectionName": "Example Podcast",
      "kind": "podcast",
      "feedUrl": "https://example.com/feed.xml"
    }
  ]
}"#;

    const DIRECTORY_SEARCH_RESULTS_ALT: &[u8] = br#"{
  "resultCount": 1,
  "results": [
    {
      "artworkUrl100": "https://example.com/artwork_100x100.jpg",
      "collectionId": 124,
      "collectionName": "Alternate Podcast",
      "kind": "podcast",
      "feedUrl": "https://example.com/alternate.xml"
    }
  ]
}"#;

    // Encapsulates the structures that are needed for tests to run. One should only be obtained by
    // invoking bootstrap().
    struct TestBootstrap {
        conn:        PooledConnection<ConnectionManager<PgConnection>>,
        log:         Logger,
        url_fetcher: URLFetcherPassThrough,
    }

    impl TestBootstrap {
        fn new(data: &[u8]) -> TestBootstrap {
            let conn = test_helpers::connection();

            TestBootstrap {
                conn:        conn,
                log:         test_helpers::log(),
                url_fetcher: URLFetcherPassThrough {
                    data: Arc::new(data.to_vec()),
                },
            }
        }

        fn mediator(&mut self) -> (DirectoryPodcastSearcher, Logger) {
            (
                DirectoryPodcastSearcher {
                    conn:        &*self.conn,
                    query:       DIRECTORY_SEARCH_QUERY.to_owned(),
                    url_fetcher: &mut self.url_fetcher,
                },
                self.log.clone(),
            )
        }
    }
}
