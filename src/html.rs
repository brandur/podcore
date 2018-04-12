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

    if !dom.errors.is_empty() {
        println!("\nParse errors:");
        for err in dom.errors.into_iter() {
            println!("    {}", err);
        }
    }

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
                    "em" => {
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
        assert_eq!(
            "<em>emphasized</em>",
            sanitize_html("<em>emphasized</em>").as_str()
        );
    }
}
