use errors::*;
use mediators::common;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct PodcastFeedLocationUpgrader<'a> {
    pub conn: &'a PgConnection,
}

impl<'a> PodcastFeedLocationUpgrader<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
        })
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        Ok(RunResult {
            num_upgraded: Self::insert_https_feed_locations(log, self.conn)?,
        })
    }

    //
    // Steps
    //

    fn insert_https_feed_locations(log: &Logger, conn: &PgConnection) -> Result<i64> {
        let res = common::log_timed(
            &log.new(o!("step" => "insert_https_feed_locations")),
            |_log| {
                // We select into a custom type because Diesel's query DSL cannot handle
                // subselects.
                diesel::sql_query(include_str!(
                    "../sql/podcast_feed_location_upgrader_insert.sql"
                )).execute(conn)
            },
        )?;

        Ok(res as i64)
    }
}

pub struct RunResult {
    pub num_upgraded: i64,
}

#[cfg(test)]
mod tests {
    use http_requester::HTTPRequesterPassThrough;
    use mediators::podcast_feed_location_upgrader::*;
    use mediators::podcast_updater::PodcastUpdater;
    use model;
    use schema;
    use test_helpers;

    use std::sync::Arc;

    #[test]
    fn test_upgrades_location() {
        // Establish one connection with an open transaction for which data will live
        // across this whole test.
        let conn = test_helpers::connection();

        let mut bootstrap = TestBootstrapWithConn::new(&*conn);

        // Insert one feed with HTTPS. This will allow our query to discover that
        // example.com supports encrypted connections, and upgraded any other
        // non-HTTPS URLs that it discovers at that domain.
        let _secured_podcast = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "https://example.com/secured.xml",
        );

        // And insert another podcast that's not secured, but at the same domain as our
        // archetype.
        let unsecured_podcast = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "http://example.com/feed.xml",
        );

        let feed_urls: Vec<String> = schema::podcast_feed_location::table
            .filter(schema::podcast_feed_location::podcast_id.eq(unsecured_podcast.id))
            .select(schema::podcast_feed_location::feed_url)
            .order(schema::podcast_feed_location::feed_url)
            .load(&*conn)
            .unwrap();
        assert_eq!(vec!["http://example.com/feed.xml"], feed_urls);

        let (mut mediator, log) = bootstrap.mediator();
        let res = mediator.run(&log).unwrap();

        let feed_urls: Vec<String> = schema::podcast_feed_location::table
            .filter(schema::podcast_feed_location::podcast_id.eq(unsecured_podcast.id))
            .select(schema::podcast_feed_location::feed_url)
            .order(schema::podcast_feed_location::feed_url)
            .load(&*conn)
            .unwrap();
        assert_eq!(
            vec![
                "http://example.com/feed.xml",
                "https://example.com/feed.xml",
            ],
            feed_urls
        );

        assert_eq!(1, res.num_upgraded);
    }

    //
    // Private types/functions
    //

    struct TestBootstrapWithConn<'a> {
        _common: test_helpers::CommonTestBootstrap,
        conn:    &'a PgConnection,
        log:     Logger,
    }

    impl<'a> TestBootstrapWithConn<'a> {
        fn new(conn: &'a PgConnection) -> TestBootstrapWithConn<'a> {
            TestBootstrapWithConn {
                _common: test_helpers::CommonTestBootstrap::new(),
                conn:    conn,
                log:     test_helpers::log(),
            }
        }

        fn mediator(&mut self) -> (PodcastFeedLocationUpgrader, Logger) {
            (
                PodcastFeedLocationUpgrader { conn: self.conn },
                self.log.clone(),
            )
        }
    }

    fn insert_podcast(log: &Logger, conn: &PgConnection, url: &str) -> model::Podcast {
        PodcastUpdater {
            conn:             conn,
            disable_shortcut: false,
            feed_url:         url.to_owned(),
            http_requester:   &mut HTTPRequesterPassThrough {
                data: Arc::new(test_helpers::MINIMAL_FEED.to_vec()),
            },
        }.run(log)
            .unwrap()
            .podcast
    }
}
