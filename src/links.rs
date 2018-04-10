use errors::*;

fn slug(s: &str) -> String {
    let parts: Vec<&str> = s.split(char::is_whitespace).collect();

    let mut slug: Option<String> = None;
    for part in parts {
        let sanitized_part = part.to_lowercase()
            .replace(|c| !char::is_alphanumeric(c), "");

        let new_slug = if let Some(current) = slug {
            // Handles the case of a long string. To avoid abrupt slug truncation we try to
            // break along the previous part boundary instead, so if adding the
            // new part would bring us over maximum length, just return what we
            // have now.
            if current.len() + 1 + sanitized_part.len() > SLUG_MAX_LENGTH {
                return current;
            }

            current + SLUG_SEPARATOR + sanitized_part.as_str()
        } else {
            // Handles the case of a single long string token that was not broken by any
            // whitespace. In this case we should flat out truncate it.
            if sanitized_part.len() >= SLUG_MAX_LENGTH {
                return sanitized_part
                    .chars()
                    .take(SLUG_MAX_LENGTH)
                    .collect::<String>();
            }

            sanitized_part
        };
        slug = Some(new_slug);
    }
    return slug.unwrap();
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

#[cfg(test)]
mod test {
    use links::*;

    use std;

    #[test]
    fn test_links_slug() {
        assert_eq!("hello-world", slug("hello, world").as_str());
        assert_eq!("martian-timeslip", slug("Martian Time-Slip").as_str());
        assert_eq!(
            "flow-my-tears-the-policeman-said",
            slug("Flow My Tears, the Policeman Said").as_str()
        );
        assert_eq!("alices-adventures", slug("Alice's Adventures").as_str());

        // Long string with a break. We'll end up just taking the first couple tokens
        // and discarding the long one at the end.
        let long_str = "hello world ".to_owned()
            + std::iter::repeat("x")
                .take(SLUG_MAX_LENGTH + 10)
                .collect::<String>()
                .as_str();
        assert_eq!("hello-world", slug(&long_str).as_str());

        // Long string without a break
        let unbroken_long_str = std::iter::repeat("x")
            .take(SLUG_MAX_LENGTH + 10)
            .collect::<String>();
        assert_eq!(SLUG_MAX_LENGTH, slug(&unbroken_long_str).len());
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
}
