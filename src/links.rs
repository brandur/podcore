use errors::*;
use model;

pub fn link_episode(podcast: &model::Podcast, episode: &model::Episode) -> String {
    format!(
        "/podcasts/{}/episodes/{}",
        slug_id(podcast.id, &podcast.title),
        slug_id(episode.id, &episode.title)
    ).to_owned()
}

pub fn link_podcast(podcast: &model::Podcast) -> String {
    format!("/podcasts/{}", slug_id(podcast.id, &podcast.title)).to_owned()
}

/// "Unslugs" an ID by extracting any digits found in the beginning of a string
/// and discarding the rest.
///
/// This is useful because for aesthetic reasons many resources are given
/// "pretty IDs" that are used in the URL, but most of which has no true
/// functional meaning to the system underneath. For example, in an ID like
/// `123-road-work`, the entirety of `-road-work` could be discarded and
/// just `123` used for lookup.
pub fn unslug_id(s: &str) -> Result<i64> {
    s.chars()
        .take_while(|c| c.is_numeric())
        // TODO: Try to make just &str
        .collect::<String>()
        .parse::<i64>()
        .chain_err(|| "Error parsing ID")
}

//
// Private constants
//

static SLUG_MAX_LENGTH: usize = 40;
static SLUG_SEPARATOR: &str = "-";

//
// Private functions
//

fn slug(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split(char::is_whitespace).collect();

    let mut slug: Option<String> = None;
    for part in parts {
        let sanitized_part = part.to_lowercase()
            .replace(|c| !char::is_alphanumeric(c), "");

        if sanitized_part.len() < 1 {
            continue;
        }

        let new_slug = if let Some(current) = slug {
            // Handles the case of a long string. To avoid abrupt slug truncation we try to
            // break along the previous part boundary instead, so if adding the
            // new part would bring us over maximum length, just return what we
            // have now.
            if current.len() + 1 + sanitized_part.len() > SLUG_MAX_LENGTH {
                return Some(current);
            }

            current + SLUG_SEPARATOR + sanitized_part.as_str()
        } else {
            // Handles the case of a single long string token that was not broken by any
            // whitespace. In this case we should flat out truncate it.
            if sanitized_part.len() >= SLUG_MAX_LENGTH {
                return Some(
                    sanitized_part
                        .chars()
                        .take(SLUG_MAX_LENGTH)
                        .collect::<String>(),
                );
            }

            sanitized_part
        };
        slug = Some(new_slug);
    }

    return slug;
}

/// Produces a URL-safe "slugged" identifier for a resource which combines its
/// ID along with some descriptive text about it.
///
/// When decoding the parameter on the other side, only the prefixed ID is used
/// and the rest of the string is discarded. Using this style of parameter is
/// for aesthetics alone.
fn slug_id(id: i64, title: &str) -> String {
    match slug(title) {
        Some(slug) => id.to_string() + SLUG_SEPARATOR + &slug,
        None => id.to_string(),
    }
}

#[cfg(test)]
mod test {
    use links::*;
    use test_data;
    use test_helpers;

    use diesel::pg::PgConnection;
    use r2d2::PooledConnection;
    use r2d2_diesel::ConnectionManager;
    use slog::Logger;
    use std;

    #[test]
    fn test_links_link_episode() {
        let bootstrap = TestBootstrap::new();
        let podcast = test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);
        let episode = test_data::episode::first(&bootstrap.log, &*bootstrap.conn, &podcast);
        assert_eq!(
            format!(
                "/podcasts/{}/episodes/{}",
                slug_id(podcast.id, &podcast.title),
                slug_id(episode.id, &episode.title)
            ),
            link_episode(&podcast, &episode).as_str()
        );
    }

    #[test]
    fn test_links_link_podcast() {
        let bootstrap = TestBootstrap::new();
        let podcast = test_data::podcast::insert(&bootstrap.log, &*bootstrap.conn);
        assert_eq!(
            format!("/podcasts/{}", slug_id(podcast.id, &podcast.title)),
            link_podcast(&podcast).as_str()
        );
    }

    #[test]
    fn test_links_slug() {
        assert_eq!("hello-world", slug("hello, world").unwrap().as_str());
        assert_eq!(
            "martian-timeslip",
            slug("Martian Time-Slip").unwrap().as_str()
        );
        assert_eq!(
            "flow-my-tears-the-policeman-said",
            slug("Flow My Tears, the Policeman Said").unwrap().as_str()
        );
        assert_eq!(
            "alices-adventures",
            slug("Alice's Adventures").unwrap().as_str()
        );
        assert_eq!("many-spaces", slug("many     spaces").unwrap().as_str());

        // In some cases there may be nothing usable in the string at all
        assert!(slug("").is_none());
        assert!(slug("    ").is_none());

        // Long string with a break. We'll end up just taking the first couple tokens
        // and discarding the long one at the end.
        let long_str =
            "hello world xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        assert_eq!("hello-world", slug(&long_str).unwrap().as_str());

        // Long string without a break
        let unbroken_long_str = std::iter::repeat("x")
            .take(SLUG_MAX_LENGTH + 10)
            .collect::<String>();
        assert_eq!(SLUG_MAX_LENGTH, slug(&unbroken_long_str).unwrap().len());

        /*
        let real_long_str =
            "Crazy Family Adventure | Stories about RV living, family travel, and working from the road";
        assert_eq!("crazy-family-adventure", slug(&real_long_str).as_str());
        */
    }

    #[test]
    fn test_links_slug_id() {
        assert_eq!("123-hello-world", slug_id(123, "hello, world").as_str());
        assert_eq!("123", slug_id(123, "   ").as_str());
    }

    #[test]
    fn test_links_unslug_id() {
        assert_eq!(123, unslug_id("123").unwrap());
        assert_eq!(1234567890, unslug_id("1234567890").unwrap());
        assert_eq!(123, unslug_id("123-hello").unwrap());
        assert_eq!(123, unslug_id("123-hello world").unwrap());
        assert_eq!(123, unslug_id("123-hello-world").unwrap());

        // Errors
        assert!(unslug_id("hello-123").is_err());
        assert!(unslug_id("").is_err());
    }

    //
    // Private types/functions
    //

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
    }

    impl TestBootstrap {
        fn new() -> TestBootstrap {
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                conn:    test_helpers::connection(),
                log:     test_helpers::log(),
            }
        }
    }
}
