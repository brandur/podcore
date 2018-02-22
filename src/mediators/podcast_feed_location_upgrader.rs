use errors::*;
use time_helpers;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use slog::Logger;

pub struct PodcastFeedLocationUpgrader<'a> {
    pub conn: &'a PgConnection,
}

impl<'a> PodcastFeedLocationUpgrader<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| {
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
        let res = time_helpers::log_timed(
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
    use model::insertable;
    use schema;
    use test_helpers;

    use chrono::Utc;
    use std::sync::Arc;

    #[test]
    fn test_upgrades_unsecured_location() {
        // Establish one connection with an open transaction for which data will live
        // across this whole test.
        let conn = test_helpers::connection();

        let mut bootstrap = TestBootstrapWithConn::new(&*conn);

        // Insert one feed with HTTPS. This will allow our query to discover that
        // example.com supports encrypted connections, and upgraded any other
        // non-HTTPS URLs that it discovers at that domain.
        let _ = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "https://example.com/secured.xml",
        );

        // And insert another podcast that's not secured, but at the same domain as our
        // archetype.
        let podcast = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "http://example.com/feed.xml",
        );

        assert_eq!(
            vec!["http://example.com/feed.xml"],
            select_feed_urls(&*conn, &podcast)
        );

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(1, res.num_upgraded);
        }

        assert_eq!(
            vec![
                "http://example.com/feed.xml",
                "https://example.com/feed.xml",
            ],
            select_feed_urls(&*conn, &podcast)
        );

        // Another run should have no effect
        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(0, res.num_upgraded);
        }
    }

    #[test]
    fn test_upgrades_whitelisted_host() {
        let conn = test_helpers::connection();
        let mut bootstrap = TestBootstrapWithConn::new(&*conn);

        // And insert another podcast that's not secured, but at the same domain as our
        // archetype.
        let podcast = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "http://example.libsyn.com/feed.xml",
        );

        assert_eq!(
            vec!["http://example.libsyn.com/feed.xml"],
            select_feed_urls(&*conn, &podcast)
        );

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(1, res.num_upgraded);
        }

        assert_eq!(
            vec![
                "http://example.libsyn.com/feed.xml",
                "https://example.libsyn.com/feed.xml",
            ],
            select_feed_urls(&*conn, &podcast)
        );
    }

    #[test]
    fn test_ignores_other_hosts() {
        let conn = test_helpers::connection();
        let mut bootstrap = TestBootstrapWithConn::new(&*conn);

        let _ = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "https://example.com/secured.xml",
        );

        // Insert an unsecured podcast, but at a different host (even a subdomain is a
        // different host). This should be ignored by the mediator's run
        // because we don't know whether or not it supports HTTPS.
        let podcast = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "http://subdomain.example.com/feed.xml",
        );

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(0, res.num_upgraded);
        }

        assert_eq!(
            vec!["http://subdomain.example.com/feed.xml"],
            select_feed_urls(&*conn, &podcast)
        );
    }

    #[test]
    fn test_ignores_secured_location() {
        let conn = test_helpers::connection();
        let mut bootstrap = TestBootstrapWithConn::new(&*conn);

        let _ = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "https://example.com/secured.xml",
        );

        let podcast = insert_podcast(
            &bootstrap.log,
            &*bootstrap.conn,
            "http://example.com/feed.xml",
        );

        // Unlike our previous example, here we insert an additional record for the
        // same podcast that is HTTPS.
        diesel::insert_into(schema::podcast_feed_location::table)
            .values(&insertable::PodcastFeedLocation {
                first_retrieved_at: Utc::now(),
                feed_url:           "https://example.com/feed.xml".to_owned(),
                last_retrieved_at:  Utc::now(),
                podcast_id:         podcast.id,
            })
            .execute(&*conn)
            .unwrap();

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(0, res.num_upgraded);
        }

        assert_eq!(
            vec![
                "http://example.com/feed.xml",
                "https://example.com/feed.xml",
            ],
            select_feed_urls(&*conn, &podcast)
        );
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

    fn select_feed_urls(conn: &PgConnection, podcast: &model::Podcast) -> Vec<String> {
        schema::podcast_feed_location::table
            .filter(schema::podcast_feed_location::podcast_id.eq(podcast.id))
            .select(schema::podcast_feed_location::feed_url)
            .order(schema::podcast_feed_location::feed_url)
            .load(&*conn)
            .unwrap()
    }
}
