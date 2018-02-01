use errors::*;
use mediators::common;
use model;
use model::insertable;
use url_fetcher::URLFetcher;

use diesel;
use diesel::prelude::*;
use diesel::pg::PgConnection;
use schema::directories_podcasts;
use serde_json;
use slog::Logger;
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
        let itunes = model::Directory::itunes(&self.conn)?;
        let body = self.fetch_results(log)?;
        let results = Self::parse_results(&log, body.as_slice())?;
        let _directory_podcasts = self.upsert_directory_podcasts(&log, &results, &itunes);
        Ok(RunResult {})
    }

    // Steps
    //

    fn fetch_results(&mut self, log: &Logger) -> Result<Vec<u8>> {
        let encoded: String = form_urlencoded::Serializer::new(String::new())
            .append_pair("media", "podcast")
            .append_pair("term", self.query.as_str())
            .finish();

        let (body, _final_url) =
            common::log_timed(&log.new(o!("step" => "fetch_results")), |ref _log| {
                self.url_fetcher
                    .fetch(format!("https://itunes.apple.com/search?{}", encoded))
            })?;
        Ok(body)
    }

    fn parse_results(log: &Logger, data: &[u8]) -> Result<Vec<SearchResult>> {
        let wrapper: SearchResultWrapper =
            common::log_timed(&log.new(o!("step" => "parse_results")), |ref _log| {
                serde_json::from_slice(data).chain_err(|| "Error parsing search results JSON")
            })?;
        Ok(wrapper.results)
    }

    fn upsert_directory_podcasts(
        &mut self,
        log: &Logger,
        results: &Vec<SearchResult>,
        directory: &model::Directory,
    ) -> Result<Vec<model::DirectoryPodcast>> {
        let ins_podcasts: Vec<insertable::DirectoryPodcast> = results
            .iter()
            .map(|ref p| insertable::DirectoryPodcast {
                directory_id: directory.id,
                feed_url:     Some(p.feed_url.clone()),
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

pub struct RunResult {}

//
// Private types
//

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    artwork_url_100: String,
    collection_id:   u32,
    collection_name: String,
    feed_url:        String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResultWrapper {
    results: Vec<SearchResult>,
}

//
// Private functions
//
