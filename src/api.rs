use iron::{typemap, AfterMiddleware, BeforeMiddleware};
use iron::prelude::*;
use mount::Mount;
use slog::Logger;
use time::precise_time_ns;

pub fn chain(log: &Logger, mount: Mount) -> Chain {
    let mut chain = Chain::new(mount);

    // Tried to pass in `log` as a reference here, but ran into serious trouble giving a middleware
    // a lifetime like 'a because all the Iron traits require a static lifetime. I don't really
    // understand why.
    chain.link_before(ResponseTime { log: log.clone() });
    chain.link_after(ResponseTime { log: log.clone() });

    chain
}

// HTTP abstractions
//

struct ResponseTime {
    log: Logger,
}

impl typemap::Key for ResponseTime {
    type Value = u64;
}

impl BeforeMiddleware for ResponseTime {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<ResponseTime>(precise_time_ns());
        Ok(())
    }
}

impl AfterMiddleware for ResponseTime {
    fn after(&self, req: &mut Request, res: Response) -> IronResult<Response> {
        let delta = precise_time_ns() - *req.extensions.get::<ResponseTime>().unwrap();
        info!(self.log, "Request finished"; "time_ms" => (delta as f64) / 1000000.0);
        Ok(res)
    }
}
