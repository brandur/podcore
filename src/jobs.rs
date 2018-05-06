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
        pub requester: &'a HttpRequester,
    }

    impl<'a> Job<'a> {
        pub fn run(&self, _log: &Logger) -> Result<()> {
            Ok(())
        }
    }

    //
    // Tests
    //

    #[cfg(test)]
    mod tests {
        use http_requester::HttpRequesterPassThrough;
        use jobs::verification_mailer::*;
        use test_helpers;

        use std::sync::Arc;

        #[test]
        fn test_job_verification_mailer_run() {
            Job {
                args:      Args {
                    to:                   "foo@example.com".to_owned(),
                    verification_code_id: 0,
                },
                requester: &HttpRequesterPassThrough {
                    data: Arc::new(Vec::new()),
                },
            }.run(&test_helpers::log())
                .unwrap();
        }
    }
}
