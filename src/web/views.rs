use actix_web::http::StatusCode;
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

                // curl -L -o assets/react.production.min.js https://unpkg.com/react@16/umd/react.production.min.js
                script(defer, src=format_args!("/assets/{}/react.production.min.js", view_model.assets_version)) {}

                // curl -L -o assets/react-dom.production.min.js https://unpkg.com/react-dom@16/umd/react-dom.production.min.js
                script(defer, src=format_args!("/assets/{}/react-dom.production.min.js", view_model.assets_version)) {}

                script(defer, src=format_args!("/assets/{}/app.js", view_model.assets_version)) {}
            }
            body {
                span {
                    @ if let Some(ref account) = view_model.account {
                        p: format_args!("Account ID: {}", account.id);
                        @ if account.ephemeral {
                            p {
                                a(href="/login"): "Login";
                                a(href="/signup"): "Signup";
                            }
                        } else {
                            p {
                                a(href="/logout"): "Logout";
                            }
                        }
                    } else {
                        p: "Not account set";
                        p {
                            a(href="/login"): "Login";
                            a(href="/signup"): "Signup";
                        }
                    }
                }
                container {
                    : Raw(content)
                }
            }
        }
    }).into_string()
        .map_err(Error::from)
}

//
// Errors
//

pub fn render_user_error(code: StatusCode, message: String) -> Result<String> {
    let title = format!("Error: {}", code);
    render_layout(
        // It's somewhat difficult to access `State` from here, so we just take advantage of the
        // fact that assets will respond for every version. Hopefully errors aren't shown so often
        // that some CSS staleness on error pages will be a problem.
        &endpoints::CommonViewModel {
            account:        None,
            assets_version: "1".to_owned(),
            title:          title.to_owned(),
        },
        (html! {
            h1: title;
            p: message;
        }).into_string()?
            .as_str(),
    )
}

//
// Views
//

pub mod account_get {
    use errors::*;
    use links;
    use web::endpoints::account_get::view_model;
    use web::endpoints::CommonViewModel;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Ok) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: "Your Account";
                ul {
                    @ for podcast in &view_model.podcasts {
                        li {
                            a(href=links::link_podcast(&podcast)) {
                                : podcast.title.as_str()
                            }
                        }
                    }
                }
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod episode_get {
    use errors::*;
    use web::endpoints::episode_get::view_model;
    use web::endpoints::CommonViewModel;
    use web::views;

    use horrorshow::prelude::*;
    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Ok) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: view_model.episode.title.as_str();
                div(id="favorited-toggle") {}
                div(id="played-toggle") {}
                p: view_model.episode.media_url.as_str();
                audio(controls) {
                    source(
                        src=view_model.episode.media_url.as_str(),
                        type=view_model.episode_media_type_or_default()
                    );
                }
                @ if let Some(ref description) = view_model.episode.description {
                    : Raw(description.as_str());
                    hr;
                    p: description.as_str();
                }
                script : Raw(views::react_element(
                    "AccountPodcastEpisodeFavoritedToggler",
                    "favorited-toggle",
                    &json!({
                        "episodeId": view_model.episode.id.to_string(),
                        "favorited": view_model.is_episode_favorited(),
                    }).to_string(),
                ));
                script : Raw(views::react_element(
                    "AccountPodcastEpisodePlayedToggler",
                    "played-toggle",
                    &json!({
                        "episodeId": view_model.episode.id.to_string(),
                        "played": view_model.is_episode_played(),
                    }).to_string(),
                ));
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod login_get {
    use errors::*;
    use web::endpoints::login_get::view_model;
    use web::endpoints::CommonViewModel;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Ok) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: "Login";
                @ if let Some(ref message) = view_model.message {
                    p(class="message"): message.as_str();
                }
                form(action="/login", method="post") {
                    input(type="email", name="email", placeholder="Email");
                    input(type="password", name="password", placeholder="Password");
                    input(type="submit", value="Login");
                }
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod podcast_get {
    use errors::*;
    use links;
    use web::endpoints::podcast_get::view_model;
    use web::endpoints::CommonViewModel;
    use web::views;

    use horrorshow::prelude::*;
    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Ok) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: view_model.podcast.title.as_str();
                div(id="subscribed-toggle") {}
                p {
                    : "Hello! This is <html />"
                }
                ul {
                    @ for episode in &view_model.episodes {
                        li {
                            a(href=links::link_episode(&view_model.podcast, &episode)) {
                                : episode.title.as_str()
                            }
                        }
                    }
                }
                script : Raw(views::react_element(
                    "AccountPodcastSubscribedToggler",
                    "subscribed-toggle",
                    &json!({
                        "podcastId": view_model.podcast.id.to_string(),
                        "subscribed": view_model.is_subscribed(),
                    }).to_string(),
                ));
            }).into_string()?
                .as_str(),
        )
    }
}

pub mod search_get {
    use errors::*;
    use links;
    use web::endpoints::search_get::view_model;
    use web::endpoints::CommonViewModel;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Ok) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: "Search";
                form(action="/search", method="get") {
                    input(type="text", name="q");
                    input(type="submit", value="Submit");
                }
                @ if let Some(ref query) = view_model.query {
                    p {
                        : format_args!("Query: {}", query.as_str());
                    }
                }
                @ if let Some(ref tuples) = view_model.directory_podcasts_and_podcasts {
                    ul {
                        @ for &(ref dir_podcast, ref podcast) in tuples {
                            li {
                                @ if let &Some(ref podcast) = podcast {
                                    a(href=links::link_podcast(podcast)) {
                                        : dir_podcast.title.as_str()
                                    }
                                } else {
                                    a(href=links::link_directory_podcast(dir_podcast)) {
                                        : dir_podcast.title.as_str()
                                    }
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

pub mod signup_get {
    use errors::*;
    use web::endpoints::signup_get::view_model;
    use web::endpoints::CommonViewModel;
    use web::views;

    use horrorshow::Template;

    pub fn render(common: &CommonViewModel, view_model: &view_model::Ok) -> Result<String> {
        views::render_layout(
            common,
            (html! {
                h1: "Signup";
                @ if let Some(ref message) = view_model.message {
                    p(class="message"): message.as_str();
                }
                form(action="/signup", method="post") {
                    input(type="email", name="email", placeholder="Email");
                    input(type="password", name="password", placeholder="Password");
                    input(type="password", name="password_confirm", placeholder="Confirm password");
                    input(type="submit", value="Create account");
                }
            }).into_string()?
                .as_str(),
        )
    }
}

//
// Other helpers
//

/// Generates a simple initializer for a React component targeting a specific
/// container in the DOM. Use of the `json!` macro is recommended to generate
/// properties.
///
/// This should probably be a macro, but I'm too lazy to write on right now.
#[inline]
pub fn react_element(component: &str, container: &str, properties: &str) -> String {
    // Our scripts use `defer` so make sure to only run this on the `load` event.
    format!(
        "window.addEventListener('load', function () {{ ReactDOM.render(React.createElement({}, {}), document.getElementById('{}')); }});",
        component, properties, container
    ).to_owned()
}
