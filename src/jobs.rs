pub mod no_op {
    use errors::*;

    use slog::Logger;

    pub const NAME: &str = "no_op";

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
}
pub mod verification_mailer {
    use errors::*;
    use http_requester::HttpRequester;

    use slog::Logger;

    pub const NAME: &str = "verification_mailer";

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct Args {
        pub to:    String,
        pub token: String,
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
}
