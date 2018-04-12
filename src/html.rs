use std::default::Default;
use std::io::BufReader;
use std::string::String;

use html5ever::parse_document;
use html5ever::rcdom::{Handle, NodeData, RcDom};
use html5ever::tendril::TendrilSink;

pub fn sanitize_html(s: &str) -> String {
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

// FIXME: Copy of str::escape_default from std, which is currently unstable
fn escape_default(s: &str) -> String {
    s.chars().flat_map(|c| c.escape_default()).collect()
}

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
                    tag @ "code"
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

            /*

            assert!(name.ns == ns!(html));
            print!("<{}", name.local);
            for attr in attrs.borrow().iter() {
                assert!(attr.name.ns == ns!());
                print!(" {}=\"{}\"", attr.name.local, attr.value);
            }
            println!(">");
            */
        }

        NodeData::ProcessingInstruction { .. } => unreachable!(),

        // Push through standard content.
        NodeData::Text { ref contents } => {
            out.push_str(&escape_default(&contents.borrow()));
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
    fn test_sanitize_html() {
        // No HTML
        assert_eq!("x", sanitize_html("x").as_str());

        // With newlines. We get a double slash produced on the left, which I'm not
        // entirely sure is right. If not and it's a problem, I'll fix it later.
        assert_eq!("x\\n\\ny", sanitize_html("x\n\ny").as_str());

        // Allowed elements
        assert_eq!("<code>x</code>", sanitize_html("<code>x</code>").as_str());
        assert_eq!("<em>x</em>", sanitize_html("<em>x</em>").as_str());
        assert_eq!(
            "<ol><li>x</li></ol>",
            sanitize_html("<ol><li>x</li></ol>").as_str()
        );
        assert_eq!("<p>x</p>", sanitize_html("<p>x</p>").as_str());
        assert_eq!("<sub>x</sub>", sanitize_html("<sub>x</sub>").as_str());
        assert_eq!("<sup>x</sup>", sanitize_html("<sup>x</sup>").as_str());
        assert_eq!(
            "<strong>x</strong>",
            sanitize_html("<strong>x</strong>").as_str()
        );
        assert_eq!(
            "<ul><li>x</li></ul>",
            sanitize_html("<ul><li>x</li></ul>").as_str()
        );

        // Allowed element with attributes stripped
        assert_eq!(
            "<em>x</em>",
            sanitize_html("<em class=\"y\">x</em>").as_str()
        );

        // Elements converted to more semantically correct elements
        assert_eq!("<em>x</em>", sanitize_html("<i>x</i>").as_str());
        assert_eq!(
            "<strong>x</strong>",
            sanitize_html("<bold>x</bold>").as_str()
        );

        // Link

        // Link without href

        // Multiple elements
        assert_eq!(
            "<code>x</code> hello <em><strong>x</strong></em>",
            sanitize_html("<code>x</code> hello <em><strong>x</strong></em>").as_str()
        );

        // Disallowed element
        assert_eq!(
            "foo ",
            sanitize_html("foo <img src=\"tracker.png\">").as_str()
        );

        // Unclosed element (just so we know the behavior here)
        assert_eq!("<em>x</em>", sanitize_html("<em>x").as_str());
    }
}
