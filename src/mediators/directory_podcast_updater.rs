use errors::*;
use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use model;
use url_fetcher::URLFetcher;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct DirectoryPodcastUpdater<'a> {
    pub conn:        &'a PgConnection,
    pub dir_podcast: &'a mut model::DirectoryPodcast,
    pub url_fetcher: &'a mut URLFetcher,
}

impl<'a> DirectoryPodcastUpdater<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |ref log| {
            self.conn
                .transaction::<_, Error, _>(move || self.run_inner(&log))
                .chain_err(|| "Error in database transaction")
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        PodcastUpdater {
            conn:             self.conn,
            disable_shortcut: false,
            feed_url:         self.dir_podcast.feed_url.clone(),
            url_fetcher:      self.url_fetcher,
        }.run(&log)?;

        Ok(RunResult {
            dir_podcast: self.dir_podcast,
        })
    }
}

pub struct RunResult<'a> {
    pub dir_podcast: &'a model::DirectoryPodcast,
}

// Tests
//

#[cfg(test)]
mod tests {
    use mediators::directory_podcast_updater::*;
    use model;
    use model::insertable;
    use schema::directory_podcast;
    use test_helpers;
    use url_fetcher::URLFetcherPassThrough;

    use diesel;
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;

    #[test]
    fn test_minimal_feed() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);
        let (mut mediator, log) = bootstrap.mediator();
        let _res = mediator.run(&log).unwrap();
    }

    //
    // Private types/functions
    //

    // Encapsulates the structures that are needed for tests to run. One should
    // only be obtained by invoking TestBootstrap::new().
    struct TestBootstrap {
        conn:        PooledConnection<ConnectionManager<PgConnection>>,
        dir_podcast: model::DirectoryPodcast,
        log:         Logger,
        url_fetcher: URLFetcherPassThrough,
    }

    impl TestBootstrap {
        fn new(data: &[u8]) -> TestBootstrap {
            let conn = test_helpers::connection();
            let url = "https://example.com/feed.xml";

            let itunes = model::Directory::itunes(&conn).unwrap();
            let dir_podcast_ins = insertable::DirectoryPodcast {
                directory_id: itunes.id,
                feed_url:     url.to_owned(),
                podcast_id:   None,
                title:        "Example Podcast".to_owned(),
                vendor_id:    "471418144".to_owned(),
            };
            let dir_podcast = diesel::insert_into(directory_podcast::table)
                .values(&dir_podcast_ins)
                .get_result(&*conn)
                .unwrap();

            TestBootstrap {
                conn:        conn,
                dir_podcast: dir_podcast,
                log:         test_helpers::log(),
                url_fetcher: URLFetcherPassThrough {
                    data: Arc::new(data.to_vec()),
                },
            }
        }

        fn mediator(&mut self) -> (DirectoryPodcastUpdater, Logger) {
            (
                DirectoryPodcastUpdater {
                    conn:        &*self.conn,
                    dir_podcast: &mut self.dir_podcast,
                    url_fetcher: &mut self.url_fetcher,
                },
                self.log.clone(),
            )
        }
    }
}
