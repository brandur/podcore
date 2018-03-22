use errors::*;
use model;
use model::insertable;
use schema;
use time_helpers;

use chrono::Utc;
use diesel;
use diesel::pg::PgConnection;
use diesel::pg::upsert::excluded;
use diesel::prelude::*;
use slog::Logger;

pub struct Mediator<'a> {
    pub account: &'a model::Account,
    pub conn:    &'a PgConnection,
    pub podcast: &'a model::Podcast,
}

impl<'a> Mediator<'a> {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        self.conn.transaction::<_, Error, _>(|| self.run_inner(log))
    }

    fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let account_podcast = self.upsert_account_podcast(log)?;
        Ok(RunResult { account_podcast })
    }

    //
    // Steps
    //

    fn upsert_account_podcast(&mut self, log: &Logger) -> Result<model::AccountPodcast> {
        let ins_account_podcast = insertable::AccountPodcast {
            account_id:      self.account.id,
            podcast_id:      self.podcast.id,
            subscribed_at:   Utc::now(),
            unsubscribed_at: None,
        };

        time_helpers::log_timed(&log.new(o!("step" => "upsert_account_podcast")), |_log| {
            diesel::insert_into(schema::account_podcast::table)
                .values(&ins_account_podcast)
                .on_conflict((
                    schema::account_podcast::account_id,
                    schema::account_podcast::podcast_id,
                ))
                .do_update()
                .set((
                    schema::account_podcast::subscribed_at
                        .eq(excluded(schema::account_podcast::subscribed_at)),
                    schema::account_podcast::unsubscribed_at
                        .eq(excluded(schema::account_podcast::unsubscribed_at)),
                ))
                .get_result(self.conn)
                .chain_err(|| "Error upserting account_podcast")
        })
    }
}

pub struct RunResult {
    pub account_podcast: model::AccountPodcast,
}

//
// Tests
//

#[cfg(test)]
mod tests {}
