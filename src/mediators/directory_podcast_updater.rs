use error_helpers;
use errors::*;
use mediators::common;
use mediators::podcast_updater::PodcastUpdater;
use model;
use model::insertable;
use schema;
use url_fetcher::URLFetcher;

use chrono::Utc;
use diesel;
use diesel::pg::PgConnection;
use diesel::pg::upsert::excluded;
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
        let res = PodcastUpdater {
            conn:             self.conn,
            disable_shortcut: false,
            feed_url:         self.dir_podcast.feed_url.clone(),
            url_fetcher:      self.url_fetcher,
        }.run(&log);

        match res {
            Ok(res) => {
                self.save_dir_podcast(&log, &res.podcast)?;
                self.delete_exception(&log)?;

                Ok(RunResult {
                    dir_podcast:    self.dir_podcast,
                    dir_podcast_ex: None,
                    podcast:        Some(res.podcast),
                })
            }
            Err(e) => {
                let ex = self.upsert_exception(&log, &e)?;
                if let Err(inner_e) = error_helpers::report_error(&log, &e) {
                    error_helpers::print_error(&log, &inner_e);
                }

                Ok(RunResult {
                    dir_podcast:    self.dir_podcast,
                    dir_podcast_ex: Some(ex),
                    podcast:        None,
                })
            }
        }
    }

    //
    // Steps
    //

    fn delete_exception(&mut self, log: &Logger) -> Result<()> {
        common::log_timed(&log.new(o!("step" => "save_dir_podcast")), |ref _log| {
            diesel::delete(schema::directory_podcast_exception::table.filter(
                schema::directory_podcast_exception::directory_podcast_id.eq(self.dir_podcast.id),
            )).execute(self.conn)
                .chain_err(|| "Error deleting directory podcast exception")
        })?;
        Ok(())
    }

    fn save_dir_podcast(&mut self, log: &Logger, podcast: &model::Podcast) -> Result<()> {
        common::log_timed(&log.new(o!("step" => "save_dir_podcast")), |ref _log| {
            self.dir_podcast.podcast_id = Some(podcast.id);
            self.dir_podcast
                .save_changes::<model::DirectoryPodcast>(&self.conn)
                .chain_err(|| "Error saving changes to directory podcast")
        })?;
        Ok(())
    }

    fn upsert_exception(
        &mut self,
        log: &Logger,
        err: &Error,
    ) -> Result<model::DirectoryPodcastException> {
        let ins_ex = insertable::DirectoryPodcastException {
            directory_podcast_id: self.dir_podcast.id,
            errors:               error_strings(err),
            occurred_at:          Utc::now(),
        };

        common::log_timed(&log.new(o!("step" => "upsert_exception")), |ref _log| {
            Ok(
                diesel::insert_into(schema::directory_podcast_exception::table)
                    .values(&ins_ex)
                    .on_conflict((schema::directory_podcast_exception::directory_podcast_id))
                    .do_update()
                    .set((
                        schema::directory_podcast_exception::errors
                            .eq(excluded(schema::directory_podcast_exception::errors)),
                        schema::directory_podcast_exception::occurred_at
                            .eq(excluded(schema::directory_podcast_exception::occurred_at)),
                    ))
                    .get_result(self.conn)
                    .chain_err(|| "Error upserting directory podcast exception")?,
            )
        })
    }
}

pub struct RunResult<'a> {
    pub dir_podcast:    &'a model::DirectoryPodcast,
    pub dir_podcast_ex: Option<model::DirectoryPodcastException>,
    pub podcast:        Option<model::Podcast>,
}

// Tests
//

#[cfg(test)]
mod tests {
    use mediators::directory_podcast_updater::*;
    use model;
    use model::insertable;
    use schema;
    use test_helpers;
    use url_fetcher::URLFetcherPassThrough;

    use diesel;
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;

    #[test]
    fn test_directory_podcast_hydration_success() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // We should have a podcast and no exception
        assert_eq!(true, res.dir_podcast_ex.is_none());
        assert_eq!(true, res.podcast.is_some());

        // Check that the directory podcast has been updated
        let podcast = res.podcast.unwrap();
        assert_eq!(Some(podcast.id), res.dir_podcast.podcast_id);
    }

    #[test]
    fn test_directory_podcast_hydration_failure() {
        let mut bootstrap = TestBootstrap::new(b"not a feed");
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // We should have an exception and no podcast
        assert_eq!(true, res.dir_podcast_ex.is_some());
        assert_eq!(true, res.podcast.is_none());

        let ex = res.dir_podcast_ex.unwrap();
        assert_eq!(res.dir_podcast.id, ex.directory_podcast_id);
        assert_ne!(0, ex.errors.len());
    }

    // An old exception should be removed on a new success
    #[test]
    fn test_directory_podcast_exception_removal() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);

        let dir_podcast_ex_ins = insertable::DirectoryPodcastException {
            directory_podcast_id: bootstrap.dir_podcast.id,
            errors:               vec!["a".to_owned(), "b".to_owned()],
            occurred_at:          Utc::now(),
        };
        diesel::insert_into(schema::directory_podcast_exception::table)
            .values(&dir_podcast_ex_ins)
            .execute(&*bootstrap.conn)
            .unwrap();

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(true, res.dir_podcast_ex.is_none());
        }

        // Exception count should now be back down to zero
        assert_eq!(
            Ok(0),
            schema::directory_podcast_exception::table
                .count()
                .first(&*bootstrap.conn)
        );
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
            let dir_podcast = diesel::insert_into(schema::directory_podcast::table)
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
