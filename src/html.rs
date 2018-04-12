use std::default::Default;
use std::io::BufReader;
use std::iter::repeat;
use std::string::String;

use html5ever::parse_document;
use html5ever::rcdom::{Handle, NodeData, RcDom};
use html5ever::tendril::TendrilSink;

pub fn sanitize_html(s: &str) {
    let mut buf = BufReader::new(s.as_bytes());

    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut buf)
        .unwrap();
    walk(0, dom.document);

    if !dom.errors.is_empty() {
        println!("\nParse errors:");
        for err in dom.errors.into_iter() {
            println!("    {}", err);
        }
    }
}

//
// Private functions
//

// FIXME: Copy of str::escape_default from std, which is currently unstable
fn escape_default(s: &str) -> String {
    s.chars().flat_map(|c| c.escape_default()).collect()
}

fn walk(indent: usize, handle: Handle) {
    let node = handle;
    // FIXME: don't allocate
    print!("{}", repeat(" ").take(indent).collect::<String>());

    match node.data {
        NodeData::Document => println!("#Document"),

        NodeData::Doctype {
            ref name,
            ref public_id,
            ref system_id,
        } => println!("<!DOCTYPE {} \"{}\" \"{}\">", name, public_id, system_id),

        NodeData::Text { ref contents } => {
            println!("#text: {}", escape_default(&contents.borrow()))
        }

        NodeData::Comment { ref contents } => println!("<!-- {} -->", escape_default(contents)),

        NodeData::Element {
            ref name,
            ref attrs,
            ..
        } => {
            assert!(name.ns == ns!(html));
            print!("<{}", name.local);
            for attr in attrs.borrow().iter() {
                assert!(attr.name.ns == ns!());
                print!(" {}=\"{}\"", attr.name.local, attr.value);
            }
            println!(">");
        }

        NodeData::ProcessingInstruction { .. } => unreachable!(),
    }

    for child in node.children.borrow().iter() {
        walk(indent + 4, child.clone());
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
        sanitize_html("<em>emphasized");
    }
}
