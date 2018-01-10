use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use errors::*;
use model;
use url_fetcher::URLFetcher;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct DirectoryPodcastUpdater<'a> {
    pub conn: &'a PgConnection,
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
                conn: self.conn,
                disable_shortcut: false,
                feed_url: feed_url,
                url_fetcher: self.url_fetcher,
            }.run(&log)?;

        self.save_dir_podcast(&log)?;

        Ok(RunResult { dir_podcast: self.dir_podcast })
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
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use schema::directories_podcasts;
    use test_helpers;
    use url_fetcher::URLFetcherStub;

    use diesel;

    #[test]
    fn test_minimal_feed() {
        let mut bootstrap = bootstrap(br#"
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
</rss>"#);
        let mut mediator = bootstrap.mediator();
        let res = mediator.run(&test_helpers::log()).unwrap();

        // the mediator empties feed URL after the directory podcast has been handled and its moved
        // to a more accurate property
        assert_eq!(None, res.dir_podcast.feed_url);
    }

    // Private types/functions
    //

    // Encapsulates the structures that are needed for tests to run. One should only be obtained by
    // invoking bootstrap().
    struct TestBootstrap {
        conn: PooledConnection<ConnectionManager<PgConnection>>,
        dir_podcast: model::DirectoryPodcast,
        url_fetcher: URLFetcherStub,
    }

    impl TestBootstrap {
        fn mediator(&mut self) -> DirectoryPodcastUpdater {
            DirectoryPodcastUpdater {
                conn: &*self.conn,
                dir_podcast: &mut self.dir_podcast,
                url_fetcher: &mut self.url_fetcher,
            }
        }
    }

    // Initializes the data required to get tests running.
    fn bootstrap(data: &[u8]) -> TestBootstrap {
        let conn = test_helpers::connection();
        let url = "https://example.com/feed.xml";

        let url_fetcher = URLFetcherStub { map: map!(url => data.to_vec()) };

        let itunes = model::Directory::itunes(&conn).unwrap();
        let dir_podcast_ins = insertable::DirectoryPodcast {
            directory_id: itunes.id,
            feed_url: Some(url.to_owned()),
            podcast_id: None,
            vendor_id: "471418144".to_owned(),
        };
        let dir_podcast = diesel::insert_into(directories_podcasts::table)
            .values(&dir_podcast_ins)
            .get_result(&*conn)
            .unwrap();

        TestBootstrap {
            conn: conn,
            dir_podcast: dir_podcast,
            url_fetcher: url_fetcher,
        }
    }
}
