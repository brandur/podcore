use errors::*;

fn slug(s: &str) -> String {
    let parts: Vec<&str> = s.split(char::is_whitespace).collect();

    let mut slug: Option<String> = None;
    for part in parts {
        let sanitized_part = part.to_lowercase()
            .replace(|c| !char::is_alphanumeric(c), "");
        if let Some(current) = slug {
            slug = Some(current + SLUG_SEPARATOR + sanitized_part.as_str());
        } else {
            slug = Some(sanitized_part);
        }
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

static SLUG_SEPARATOR: &str = "-";

#[cfg(test)]
mod test {
    use links::*;

    #[test]
    fn test_links_slug() {
        assert_eq!("hello-world", slug("hello, world").as_str());
        assert_eq!("martian-timeslip", slug("Martian Time-Slip").as_str());
        assert_eq!(
            "flow-my-tears-the-policeman-said",
            slug("Flow My Tears, the Policeman Said").as_str()
        );
        assert_eq!("alices-adventures", slug("Alice's Adventures").as_str());
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
