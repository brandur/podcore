use std::default::Default;
use std::io::BufReader;
use std::string::String;

use html5ever::parse_document;
use html5ever::rcdom::{Handle, NodeData, RcDom};
use html5ever::tendril::TendrilSink;

pub fn sanitize(s: &str) -> String {
    let mut buf = BufReader::new(s.as_bytes());
    let mut out = String::new();

    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut buf)
        .unwrap();
    walk(dom.document, &mut out);

    // Errors are accessible with `dom.errors`, but it's very likely that the right
    // choice there is to just throw everything out.

    out
}

//
// Private functions
//

fn walk(handle: Handle, out: &mut String) {
    let node = handle;
    let mut close_tag: Option<String> = None;

    match node.data {
        // Strip any comments that were included.
        NodeData::Comment { .. } => (),

        // This is probably not included in the HTML we're processing, but include it for a
        // complete match.
        NodeData::Doctype { .. } => (),

        // Start of document. Ignore.
        NodeData::Document => (),

        NodeData::Element {
            ref name,
            ref attrs,
            ..
        } => {
            if name.ns == ns!(html) {
                match name.local.as_ref() {
                    // Allow links, but discard any attribute except `href` and add
                    // `rel="nofollow"`.
                    "a" => {
                        let mut href: Option<String> = None;

                        for attr in attrs.borrow().iter() {
                            if attr.name.ns != ns!() {
                                continue;
                            }

                            if attr.name.local.as_ref() == "href" {
                                href = Some(attr.value.as_ref().to_owned());
                            }
                        }

                        if let Some(href) = href {
                            out.push_str(&format!("<a href=\"{}\" rel=\"nofollow\">", href));
                            close_tag = Some("</a>".to_owned());
                        }
                    }

                    // All these elements are allowed (but their attributes are stripped)
                    tag @ "blockquote"
                    | tag @ "code"
                    | tag @ "em"
                    | tag @ "li"
                    | tag @ "ol"
                    | tag @ "p"
                    | tag @ "strong"
                    | tag @ "sub"
                    | tag @ "sup"
                    | tag @ "ul" => {
                        out.push_str(&format!("<{}>", tag));
                        close_tag = Some(format!("</{}>", tag));
                    }

                    // <hr> is special because we don't bother with an end tag
                    "hr" => out.push_str("<hr>"),

                    // Convert these to elements that are more semantically correct
                    "bold" => {
                        out.push_str("<strong>");
                        close_tag = Some("</strong>".to_owned());
                    }
                    "i" => {
                        out.push_str("<em>");
                        close_tag = Some("</em>".to_owned());
                    }

                    _ => (),
                }
            }
        }

        NodeData::ProcessingInstruction { .. } => unreachable!(),

        // Push through standard content.
        NodeData::Text { ref contents } => {
            out.push_str(&contents.borrow());
        }
    }

    for child in node.children.borrow().iter() {
        walk(child.clone(), out);
    }

    if let Some(tag) = close_tag {
        out.push_str(&tag);
    }
}

//
// Private functions
//

#[cfg(test)]
mod tests {
    use html::*;

    #[test]
    fn test_sanitize() {
        // No HTML
        assert_eq!("x", sanitize("x").as_str());

        // With newlines. We get a double slash produced on the left, which I'm not
        // entirely sure is right. If not and it's a problem, I'll fix it later.
        assert_eq!("x\n\ny", sanitize("x\n\ny").as_str());

        // Unicode
        assert_eq!(
            "It wasn&#8217;t three-appropriate.",
            sanitize("It wasn&amp;#8217;t three-appropriate.").as_str()
        );

        // Allowed elements
        assert_eq!(
            "<blockquote>x</blockquote>",
            sanitize("<blockquote>x</blockquote>").as_str()
        );
        assert_eq!("<code>x</code>", sanitize("<code>x</code>").as_str());
        assert_eq!("<em>x</em>", sanitize("<em>x</em>").as_str());
        assert_eq!(
            "<ol><li>x</li></ol>",
            sanitize("<ol><li>x</li></ol>").as_str()
        );
        assert_eq!("<p>x</p>", sanitize("<p>x</p>").as_str());
        assert_eq!("<sub>x</sub>", sanitize("<sub>x</sub>").as_str());
        assert_eq!("<sup>x</sup>", sanitize("<sup>x</sup>").as_str());
        assert_eq!(
            "<strong>x</strong>",
            sanitize("<strong>x</strong>").as_str()
        );
        assert_eq!(
            "<ul><li>x</li></ul>",
            sanitize("<ul><li>x</li></ul>").as_str()
        );

        // Allowed element with attributes stripped
        assert_eq!("<em>x</em>", sanitize("<em class=\"y\">x</em>").as_str());

        // Elements converted to more semantically correct elements
        assert_eq!("<em>x</em>", sanitize("<i>x</i>").as_str());
        assert_eq!("<hr>", sanitize("<hr>").as_str());
        assert_eq!("<strong>x</strong>", sanitize("<bold>x</bold>").as_str());

        // Link
        assert_eq!(
            "<a href=\"https://example.com\" rel=\"nofollow\">x</a>",
            sanitize("<a href=\"https://example.com\" attr=\"other\">x</a>").as_str()
        );

        // Link without href
        assert_eq!("x", sanitize("<a>x</a>").as_str());

        // Multiple elements
        assert_eq!(
            "<code>x</code> hello <em><strong>x</strong></em>",
            sanitize("<code>x</code> hello <em><strong>x</strong></em>").as_str()
        );

        // Disallowed element
        assert_eq!("foo ", sanitize("foo <img src=\"tracker.png\">").as_str());

        // Unclosed element (just so we know the behavior here)
        assert_eq!("<em>x</em>", sanitize("<em>x").as_str());
    }
}
