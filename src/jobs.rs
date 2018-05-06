/// Creates a per-job enqueue helper function.
macro_rules! enqueue {
    () => {
        pub fn enqueue(_log: &Logger, conn: &PgConnection, args: &Args) -> Result<model::Job> {
            use model::insertable;
            use schema;

            use chrono::Utc;
            use diesel;
            use diesel::prelude::*;
            use serde_json;

            diesel::insert_into(schema::job::table)
                .values(&insertable::Job {
                    args:   serde_json::to_value(args)?,
                    name:   NAME.to_owned(),
                    try_at: Utc::now(),
                })
                .get_result(conn)
                .chain_err(|| "Error inserting job")
        }
    };
}
pub mod no_op {
    use errors::*;

    use slog::Logger;

    //
    // Public constants
    //

    pub const NAME: &str = "no_op";

    //
    // Public types
    //

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct Args {
        pub message: String,
    }

    pub struct Job {
        pub args: Args,
    }

    impl Job {
        pub fn run(&self, log: &Logger) -> Result<()> {
            info!(log, "No-op job: {}", self.args.message);
            Ok(())
        }
    }

    //
    // Public functions
    //

    // Currently not used anywhere.
    //enqueue!();

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use jobs::no_op::*;
        use test_helpers;

        #[test]
        fn test_job_no_op_run() {
            Job {
                args: Args {
                    message: "Hello, world".to_owned(),
                },
            }.run(&test_helpers::log())
                .unwrap();
        }
    }
}

pub mod verification_mailer {
    use errors::*;
    use http_requester::HttpRequester;
    use model;
    use schema;
    use time_helpers;

    use diesel::pg::PgConnection;
    use diesel::prelude::*;
    use r2d2::Pool;
    use r2d2_diesel::ConnectionManager;
    use slog::Logger;

    //
    // Public constants
    //

    pub const NAME: &str = "verification_mailer";

    //
    // Public types
    //

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct Args {
        pub to:                   String,
        pub verification_code_id: i64,
    }

    pub struct Job<'a> {
        pub args:      Args,
        pub pool:      &'a Pool<ConnectionManager<PgConnection>>,
        pub requester: &'a HttpRequester,
    }

    impl<'a> Job<'a> {
        pub fn run(&self, log: &Logger) -> Result<()> {
            let _code = select_code(log, self.pool, self.args.verification_code_id)?;
            Ok(())
        }
    }

    //
    // Public functions
    //

    enqueue!();

    //
    // Private functions
    //

    // Select a verification code from the database. We pass a pool instead of a
    // connection so that we can hold onto a connection for as short of a time
    // as possible.
    fn select_code(
        log: &Logger,
        pool: &Pool<ConnectionManager<PgConnection>>,
        code_id: i64,
    ) -> Result<model::VerificationCode> {
        let conn = pool.get()?;
        time_helpers::log_timed(&log.new(o!("step" => "select_code")), |_log| {
            schema::verification_code::table
                .filter(schema::verification_code::id.eq(code_id))
                .first(&*conn)
                .chain_err(|| "Error selecting code")
        })
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use http_requester::HttpRequesterPassThrough;
        use jobs::verification_mailer::*;
        use test_data;
        use test_helpers;

        use r2d2::{Pool, PooledConnection};
        use r2d2_diesel::ConnectionManager;
        use std::sync::Arc;

        #[ignore]
        #[test]
        fn test_job_verification_mailer_run() {
            let mut bootstrap = TestBootstrap::new();
            let (job, log) = bootstrap.job();
            job.run(&log).unwrap();
        }

        //
        // Private constants/types/functions
        //

        struct TestBootstrap {
            _common:   test_helpers::CommonTestBootstrap,
            conn:      PooledConnection<ConnectionManager<PgConnection>>,
            log:       Logger,
            pool:      Pool<ConnectionManager<PgConnection>>,
            requester: HttpRequesterPassThrough,
        }

        impl TestBootstrap {
            fn new() -> Self {
                let pool = test_helpers::pool();
                let conn = pool.get().map_err(Error::from).unwrap();
                TestBootstrap {
                    _common:   test_helpers::CommonTestBootstrap::new(),
                    conn:      conn,
                    log:       test_helpers::log_sync(),
                    pool:      pool,
                    requester: HttpRequesterPassThrough {
                        data: Arc::new(Vec::new()),
                    },
                }
            }

            fn job(&mut self) -> (Job, Logger) {
                (
                    Job {
                        args:      Args {
                            to:                   test_helpers::EMAIL.to_owned(),
                            verification_code_id: test_data::verification_code::insert(
                                &self.log, &self.conn,
                            ).id,
                        },
                        pool:      &self.pool,
                        requester: &self.requester,
                    },
                    self.log.clone(),
                )
            }
        }

        impl Drop for TestBootstrap {
            fn drop(&mut self) {
                test_helpers::clean_database(&self.log, &*self.conn);
            }
        }
    }
}
