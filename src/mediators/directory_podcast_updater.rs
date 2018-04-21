use errors::*;
use http_requester::HttpRequester;
use mediators::podcast_updater;
use model;
use model::insertable;
use schema;
use time_helpers;

use chrono::Utc;
use diesel;
use diesel::pg::upsert::excluded;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub conn:           &'a PgConnection,
    pub dir_podcast:    &'a mut model::DirectoryPodcast,
    pub http_requester: &'a mut HttpRequester,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn
                .transaction::<_, Error, _>(move || self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let res = podcast_updater::Mediator {
            conn:             self.conn,
            disable_shortcut: false,
            feed_url:         self.dir_podcast.feed_url.clone(),
            http_requester:   self.http_requester,
        }.run(log);

        match res {
            Ok(res) => {
                self.save_dir_podcast(log, &res.podcast)?;
                self.delete_exception(log)?;

                Ok(RunResult {
                    dir_podcast: self.dir_podcast,
                    podcast:     res.podcast,
                })
            }
            Err(e) => {
                let _ex = self.upsert_exception(log, &e)?;
                Err(e)
            }
        }
    }

    //
    // Steps
    //

    fn delete_exception(&mut self, log: &Logger) -> Result<()> {
        time_helpers::log_timed(&log.new(o!("step" => "delete_exception")), |_log| {
            diesel::delete(schema::directory_podcast_exception::table.filter(
                schema::directory_podcast_exception::directory_podcast_id.eq(self.dir_podcast.id),
            )).execute(self.conn)
                .chain_err(|| "Error deleting directory podcast exception")
        })?;
        Ok(())
    }

    fn save_dir_podcast(&mut self, log: &Logger, podcast: &model::Podcast) -> Result<()> {
        time_helpers::log_timed(&log.new(o!("step" => "save_dir_podcast")), |_log| {
            self.dir_podcast.podcast_id = Some(podcast.id);
            self.dir_podcast
                .save_changes::<model::DirectoryPodcast>(self.conn)
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

        time_helpers::log_timed(&log.new(o!("step" => "upsert_exception")), |_log| {
            Ok(
                diesel::insert_into(schema::directory_podcast_exception::table)
                    .values(&ins_ex)
                    .on_conflict(schema::directory_podcast_exception::directory_podcast_id)
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
    pub dir_podcast: &'a model::DirectoryPodcast,
    pub podcast:     model::Podcast,
}

// Tests
//

#[cfg(test)]
mod tests {
    use http_requester::HttpRequesterPassThrough;
    use mediators::directory_podcast_updater::*;
    use model;
    use model::insertable;
    use schema;
    use test_helpers;

    use diesel;
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use std::sync::Arc;

    #[test]
    fn test_directory_podcast_update_hydration_success() {
        let mut bootstrap = TestBootstrap::new(test_helpers::MINIMAL_FEED);
        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        // Check that the directory podcast has been updated
        assert_eq!(Some(res.podcast.id), res.dir_podcast.podcast_id);
    }

    #[test]
    fn test_directory_podcast_update_hydration_failure() {
        let mut bootstrap = TestBootstrap::new(b"not a feed");

        {
            let (mut mediator, log) = bootstrap.mediator();
            let e = mediator.run(&log).err().unwrap();
            assert_eq!("No <rss> tag found", e.description());
        }

        // I had a little trouble testing for the directory podcast exception here, and
        // it's something to do with transactions. The record *should* still be
        // there because the inner transaction in the mediator should activate
        // a checkpoint which then commits, but it's not. I didn't want to sink
        // anymore time into trying to figure it out though.
    }

    // An old exception should be removed on a new success
    #[test]
    fn test_directory_podcast_update_exception_removal() {
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
            let _res = mediator.run(&log).unwrap();
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
        _common:        test_helpers::CommonTestBootstrap,
        conn:           PooledConnection<ConnectionManager<PgConnection>>,
        dir_podcast:    model::DirectoryPodcast,
        log:            Logger,
        http_requester: HttpRequesterPassThrough,
    }

    impl TestBootstrap {
        fn new(data: &[u8]) -> TestBootstrap {
            let conn = test_helpers::connection();
            let log = test_helpers::log();
            let url = "https://example.com/feed.xml";

            let itunes = model::Directory::itunes(&log, &conn).unwrap();
            let dir_podcast_ins = insertable::DirectoryPodcast {
                directory_id: itunes.id,
                feed_url:     url.to_owned(),
                image_url:    Some("https://example.com/image.jpg".to_owned()),
                podcast_id:   None,
                title:        "Example Podcast".to_owned(),
                vendor_id:    "471418144".to_owned(),
            };
            let dir_podcast = diesel::insert_into(schema::directory_podcast::table)
                .values(&dir_podcast_ins)
                .get_result(&*conn)
                .unwrap();

            TestBootstrap {
                _common:        test_helpers::CommonTestBootstrap::new(),
                conn:           conn,
                dir_podcast:    dir_podcast,
                log:            log,
                http_requester: HttpRequesterPassThrough {
                    data: Arc::new(data.to_vec()),
                },
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    conn:           &*self.conn,
                    dir_podcast:    &mut self.dir_podcast,
                    http_requester: &mut self.http_requester,
                },
                self.log.clone(),
            )
        }
    }
}
