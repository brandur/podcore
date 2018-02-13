use errors::*;
use mediators::common;
use model;
use schema;

use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use slog::Logger;
use std::io::prelude::*;

pub struct PodcastFeedContentCompressor<'a> {
    pub conn: &'a PgConnection,
}

impl<'a> PodcastFeedContentCompressor<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        common::log_timed(&log.new(o!("step" => file!())), |log| {
            self.conn
                .transaction::<_, Error, _>(|| self.run_inner(log))
                .chain_err(|| "Error in database transaction")
        })
    }

    fn run_inner(&mut self, _log: &Logger) -> Result<RunResult> {
        let uncompressed_contents: Vec<model::PodcastFeedContent> =
            schema::podcast_feed_content::table
                .filter(schema::podcast_feed_content::content_gzip.is_null())
                .load::<model::PodcastFeedContent>(self.conn)?;

        let num = uncompressed_contents.len() as i64;

        for content in uncompressed_contents {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(content.content.as_bytes())?;

            diesel::update(
                schema::podcast_feed_content::table
                    .filter(schema::podcast_feed_content::id.eq(content.id)),
            ).set(schema::podcast_feed_content::content_gzip.eq(encoder.finish()?))
                .execute(self.conn)?;
        }

        Ok(RunResult {
            num_compressed: num,
        })
    }
}

pub struct RunResult {
    pub num_compressed: i64,
}
