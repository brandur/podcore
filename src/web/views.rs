use errors::*;
use web::endpoints;

use horrorshow::helper::doctype;
use horrorshow::prelude::*;

//
// Layouts
//

pub fn render_layout(view_model: &endpoints::CommonViewModel, content: &str) -> Result<String> {
    (html! {
        : doctype::HTML;
        html {
            head {
                title: view_model.title.as_str();

                meta(content="text/html; charset=utf-8", http-equiv="Content-Type");

                link(href=format_args!("/assets/{}/app.css", view_model.assets_version), media="screen", rel="stylesheet", type="text/css");
            }
            body {
                : Raw(content)
            }
        }
    }).into_string()
        .map_err(Error::from)
}

//
// Errors
//

pub fn render_500(common: &endpoints::CommonViewModel, error: &str) -> Result<String> {
    render_layout(
        common,
        (html! {
            h1: "Error";
            p: error;
        }).into_string()?
            .as_str(),
    )
}

//
// Views
//

pub mod episode_show {
    use errors::*;
    use web::endpoints::CommonViewModel;
    use web::endpoints::episode_show::view_model;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Found) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: view_model.episode.title.as_str();
                p: view_model.episode.media_url.as_str();
                @ if let Some(ref description) = view_model.episode.description {
                    p: description.as_str();
                }
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod podcast_show {
    use errors::*;
    use web::endpoints::CommonViewModel;
    use web::endpoints::podcast_show::view_model;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Found) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: view_model.podcast.title.as_str();
                p {
                    : "Hello! This is <html />"
                }
                ul {
                    @ for episode in &view_model.episodes {
                        li {
                            a(href=format_args!("/podcasts/{}/episodes/{}", episode.podcast_id, episode.id)) {
                                : episode.title.as_str()
                            }
                        }
                    }
                }
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod search_new_show {
    use errors::*;
    use web::endpoints::CommonViewModel;
    use web::endpoints::search_new_show::ViewModel;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, _view_model: &ViewModel) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: "Search";
                form(action="/search", method="get") {
                    input(type="text", name="q");
                    input(type="submit", value="Submit");
                }
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod search_show {
    use errors::*;
    use web::endpoints::CommonViewModel;
    use web::endpoints::search_show::view_model;
    use web::views;

    use horrorshow::Template;

    pub fn render(
        common: &CommonViewModel,
        view_model: &view_model::SearchResults,
    ) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                p {
                    : format_args!("Query: {}", view_model.query);
                }
                ul {
                    @ for dir_podcast in &view_model.directory_podcasts {
                        li {
                            @ if let Some(podcast_id) = dir_podcast.podcast_id {
                                a(href=format_args!("/podcasts/{}", podcast_id)) {
                                    : dir_podcast.title.as_str()
                                }
                            } else {
                                a(href=format_args!("/directory-podcasts/{}", dir_podcast.id)) {
                                    : dir_podcast.title.as_str()
                                }
                            }
                        }
                    }
                }
            }).into_string()?
                .as_str(),
        )
    }
}
