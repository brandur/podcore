use errors::*;
use mediators::common;
use model;
use model::insertable;
use url_fetcher::URLFetcher;

use chrono::Utc;
use diesel;
use diesel::prelude::*;
use diesel::pg::PgConnection;
use schema::{directories_podcasts, directories_podcasts_directory_searches, directory_searches};
use serde_json;
use slog::Logger;
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

        let (body, _final_url) =
            common::log_timed(&log.new(o!("step" => "fetch_results")), |ref _log| {
                self.url_fetcher
                    .fetch(format!("https://itunes.apple.com/search?{}", encoded))
            })?;

        let sample = &body[0..100].to_vec();
        info!(log, "Response body (sample)";
            "body" => format!("{}...", String::from_utf8_lossy(sample).replace("\n", "")));

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
        let ins_podcasts: Vec<insertable::DirectoryPodcast> = results
            .iter()
            .filter(|ref p| p.feed_url.is_some())
            .map(|ref p| insertable::DirectoryPodcast {
                directory_id: directory.id,
                feed_url:     Some(p.feed_url.clone().unwrap()),
                podcast_id:   None,
                vendor_id:    p.collection_id.to_string(),
            })
            .collect();

        common::log_timed(&log.new(o!("step" => "upsert_episodes")), |ref _log| {
            Ok(diesel::insert_into(directories_podcasts::table)
                .values(&ins_podcasts)
                .on_conflict((
                    directories_podcasts::directory_id,
                    directories_podcasts::vendor_id,
                ))
                .do_nothing()
                .get_results(self.conn)
                .chain_err(|| "Error upserting directory podcasts")?)
        })
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

    #[test]
    fn test_results_deserialization() {
        let encoded = include_bytes!("../test_documents/itunes_search_history.json");
        let decoded: SearchResultWrapper = serde_json::from_slice(encoded).unwrap();
        assert_ne!(0, decoded.result_count);
        assert_eq!(decoded.result_count, decoded.results.len());
    }
}
