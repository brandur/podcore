use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use errors::*;
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
        let feed_url = self.dir_podcast.feed_url.clone().unwrap();

        PodcastUpdater {
            conn:             self.conn,
            disable_shortcut: false,
            feed_url:         feed_url,
            url_fetcher:      self.url_fetcher,
        }.run(&log)?;

        self.save_dir_podcast(&log)?;

        Ok(RunResult {
            dir_podcast: self.dir_podcast,
        })
    }

    // Steps
    //

    fn save_dir_podcast(&mut self, log: &Logger) -> Result<()> {
        common::log_timed(&log.new(o!("step" => "save_dir_podcast")), |ref _log| {
            self.dir_podcast.feed_url = None;
            self.dir_podcast
                .save_changes::<model::DirectoryPodcast>(&self.conn)
                .chain_err(|| "Error saving changes to directory podcast")
        })?;

        Ok(())
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
    use schema::directories_podcasts;
    use test_helpers;
    use url_fetcher::URLFetcherPassThrough;

    use diesel;
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;

    #[test]
    fn test_minimal_feed() {
        let mut bootstrap = TestBootstrap::new(MINIMAL_FEED);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // the mediator empties feed URL after the directory podcast has been handled and its moved
        // to a more accurate property
        assert_eq!(None, res.dir_podcast.feed_url);
    }

    // Private types/functions
    //

    const MINIMAL_FEED: &[u8] = br#"
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
</rss>"#;

    // Encapsulates the structures that are needed for tests to run. One should only be obtained by
    // invoking bootstrap().
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
                feed_url:     Some(url.to_owned()),
                podcast_id:   None,
                vendor_id:    "471418144".to_owned(),
            };
            let dir_podcast = diesel::insert_into(directories_podcasts::table)
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
