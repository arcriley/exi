//! DOM string storage, stable node addresses and cross-document
//! ownership.
#![cfg(feature = "dom")]

use exi::dom::{Document, NodeId, raw::RawNode};
use exi::strings::{GlobalConfig, GlobalStrings};
use std::ffi::CStr;

include!("support/cases.rs");

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Comment,
    Pi,
    Attribute,
}

fn make(doc: &mut Document<'_>, kind: Kind, s: &str) -> NodeId {
    match kind {
        Kind::Text => doc.new_text(s),
        Kind::Comment => doc.new_comment(s),
        Kind::Pi => doc.new_pi("data", s),
        Kind::Attribute => doc.new_attribute("value", s).unwrap(),
    }
}

fn check(doc: &Document<'_>, kind: Kind, node: NodeId, text: &str) {
    assert_eq!(doc.content(node), text);
    let value = match kind {
        Kind::Attribute => doc.first_child(node).unwrap(),
        _ => node,
    };
    let raw = doc.raw(value).cast::<RawNode>();
    // SAFETY: the document owns this live text, comment or PI node.
    // Read exactly the known length plus the promised NUL terminator.
    let bytes =
        unsafe { std::slice::from_raw_parts((*raw).content, text.len() + 1) };
    assert_eq!(
        CStr::from_bytes_with_nul(bytes).unwrap().to_bytes(),
        text.as_bytes()
    );
}

fn storage(kind: Kind, len: usize) {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let mut doc = Document::new(&strings);
    let text = "a".repeat(len);
    let node = make(&mut doc, kind, &text);
    check(&doc, kind, node, &text);
    let copy = doc.copy(node, true).unwrap();
    for replacement in [format!("{text}é"), "b".into()] {
        doc.set_content(node, &replacement).unwrap();
        check(&doc, kind, node, &replacement);
        check(&doc, kind, copy, &text);
    }
    doc.remove(node);
    let fresh = make(&mut doc, kind, "fresh");
    assert!(!doc.contains(node));
    check(&doc, kind, fresh, "fresh");
}

macro_rules! lengths {
    ($module:ident, $kind:ident) => {
        mod $module {
            use super::*;
            use Kind::$kind;
            cases!(storage;
                empty($kind, 0), one($kind, 1), below_inline($kind, 20),
                inline_limit($kind, 21), above_inline($kind, 22),
                byte_limit($kind, 255), before_page($kind, 4095),
                page($kind, 4096), above_page($kind, 4097), large($kind, 65536),
            );
        }
    };
}
lengths!(text, Text);
lengths!(comment, Comment);
lengths!(pi, Pi);
lengths!(attribute, Attribute);

#[test]
fn adopted_nodes_from_several_chunks_outlive_both_previous_owners()
-> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let mut last = Document::new(&strings);
    let mut ids = Vec::new();
    {
        let mut middle = Document::new(&strings);
        let mut source = Document::new(&strings);
        for i in 0..5000 {
            let el = source.new_element("reading")?;
            if i % 199 == 0 {
                source.set_attribute(el, "number", &i.to_string())?;
                let raw = source.raw(el);
                let moved = middle.adopt(&mut source, el)?;
                let id = last.adopt(&mut middle, moved)?;
                assert_eq!(last.raw(id), raw);
                ids.push((id, i));
            }
        }
    }
    for (id, number) in ids {
        assert_eq!(last.attribute(id, "number"), Some(number.to_string()));
        last.remove(id);
        assert!(!last.contains(id));
    }
    Ok(())
}
