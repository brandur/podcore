use errors::*;
use web::common;

use actix;
use actix_web::{HttpRequest, HttpResponse, StatusCode};
use diesel::pg::PgConnection;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use slog::Logger;

//
// Traits
//

pub trait Params {}
pub trait Response {}

pub trait ViewModel {
    type Response: Response;
    type State: common::State;

    fn build(req: &HttpRequest<Self::State>, response: Self::Response) -> Self;
    fn render(&self, req: &HttpRequest<Self::State>) -> Result<HttpResponse>;
}

//
// Structs
//

pub struct CommonViewModel {
    pub assets_version: String,
    pub title:          String,
}

pub struct Message<P: Params> {
    pub log:    Logger,
    pub params: P,
}

pub struct StateImpl {
    pub assets_version: String,
    pub log:            Logger,
    pub pool:           Pool<ConnectionManager<PgConnection>>,
    pub sync_addr:      actix::prelude::SyncAddress<SyncExecutor>,
}

impl common::State for StateImpl {
    fn log(&self) -> &Logger {
        &self.log
    }
}

pub struct SyncExecutor {
    pub pool: Pool<ConnectionManager<PgConnection>>,
}

impl actix::Actor for SyncExecutor {
    type Context = actix::SyncContext<Self>;
}

//
// Error handlers
//

pub fn handle_404() -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::NOT_FOUND)
        .content_type("text/html; charset=utf-8")
        .body("404!")?)
}

//
// Endpoints
//

pub mod directory_podcast_show {
    use errors::*;
    use http_requester::HTTPRequesterLive;
    use mediators::directory_podcast_updater::DirectoryPodcastUpdater;
    use model;
    use schema;
    use web::endpoints;

    use actix;
    use actix_web::{HttpRequest, HttpResponse, StatusCode};
    use diesel::prelude::*;
    use hyper::Client;
    use hyper_tls::HttpsConnector;
    use tokio_core::reactor::Core;

    pub struct Params {
        pub id: i64,
    }

    impl endpoints::Params for Params {}

    // TODO: `ResponseType` will change to `Message`
    impl actix::prelude::ResponseType for endpoints::Message<Params> {
        type Item = Response;
        type Error = Error;
    }

    pub enum Response {
        Exception(model::DirectoryPodcastException),
        NotFound,
        Podcast(model::Podcast),
    }

    impl endpoints::Response for Response {}

    pub struct ViewModel {
        _common:  endpoints::CommonViewModel,
        response: Response,
    }

    impl endpoints::ViewModel for ViewModel {
        type Response = Response;
        type State = endpoints::StateImpl;

        fn build(req: &HttpRequest<Self::State>, response: Self::Response) -> Self {
            ViewModel {
                _common:  endpoints::CommonViewModel {
                    assets_version: req.state().assets_version.clone(),
                    title:          "".to_owned(),
                },
                response: response,
            }
        }

        fn render(&self, _req: &HttpRequest<Self::State>) -> Result<HttpResponse> {
            match self.response {
                Response::Exception(ref _dir_podcast_ex) => {
                    Err(Error::from("Couldn't expand directory podcast").into())
                }
                Response::NotFound => Ok(endpoints::handle_404()?),
                Response::Podcast(ref podcast) => {
                    Ok(HttpResponse::build(StatusCode::PERMANENT_REDIRECT)
                        .header("Location", format!("/podcasts/{}", podcast.id).as_str())
                        .finish()?)
                }
            }
        }
    }

    impl actix::prelude::Handler<endpoints::Message<Params>> for endpoints::SyncExecutor {
        type Result = actix::prelude::MessageResult<endpoints::Message<Params>>;

        fn handle(
            &mut self,
            message: endpoints::Message<Params>,
            _: &mut Self::Context,
        ) -> Self::Result {
            let conn = self.pool.get()?;
            let log = message.log;

            let core = Core::new().unwrap();
            let client = Client::configure()
                .connector(HttpsConnector::new(4, &core.handle()).map_err(Error::from)?)
                .build(&core.handle());
            let mut http_requester = HTTPRequesterLive { client, core };

            let dir_podcast: Option<model::DirectoryPodcast> = schema::directory_podcast::table
                .filter(schema::directory_podcast::id.eq(message.params.id))
                .first(&*conn)
                .optional()?;
            match dir_podcast {
                Some(mut dir_podcast) => {
                    let mut mediator = DirectoryPodcastUpdater {
                        conn:           &*conn,
                        dir_podcast:    &mut dir_podcast,
                        http_requester: &mut http_requester,
                    };
                    let res = mediator.run(&log)?;

                    if let Some(dir_podcast_ex) = res.dir_podcast_ex {
                        return Ok(Response::Exception(dir_podcast_ex));
                    }

                    return Ok(Response::Podcast(res.podcast.unwrap()));
                }
                None => Ok(Response::NotFound),
            }
        }
    }
}
