// tests/dom.rs
//
// Document tree round-trip and editing tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Layer 2: trees from XML text and EXI, written back out unchanged, and edited the way
//! libxml2 programs edit them.
#![cfg(all(feature = "dom", feature = "xsd"))]

use std::path::Path;

use exi::dom::{Document, NodeKind};
use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{PrefixPolicy, XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    Coding, EventSink, Name, Options, OwnedEvent, OwnedValue, Reader, ReaderConfig, Status, Step,
    Value, Writer, WriterConfig,
};

fn strings() -> GlobalStrings {
    GlobalStrings::new(GlobalConfig::new()).unwrap()
}

fn xml_events(g: &Grammars<'_>, xml: &[u8]) -> Vec<OwnedEvent> {
    let mut x: XmlReader = XmlReader::new(
        XmlConfig::new()
            .report_comments(true)
            .report_pis(true)
            .report_doctype(true),
        g,
    );
    let mut out = Vec::new();
    let mut rest = xml;
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        loop {
            match x.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

/// Adjacent text joined, as a tree holds it.
fn joined(evs: Vec<OwnedEvent>) -> Vec<OwnedEvent> {
    let mut out: Vec<OwnedEvent> = Vec::new();
    for e in evs {
        if let (
            OwnedEvent::Characters(OwnedValue::Str(b)),
            Some(OwnedEvent::Characters(OwnedValue::Str(a))),
        ) = (&e, out.last_mut())
        {
            *a = format!("{a}{b}").into();
            continue;
        }
        out.push(e);
    }
    out
}

fn doc_events(doc: &Document<'_>) -> Vec<OwnedEvent> {
    let mut e = doc.events();
    let mut out = Vec::new();
    while let Step::Event(ev, _) = exi::EventSource::next(&mut e).unwrap() {
        out.push(ev.to_owned());
    }
    out
}

/// Every XML input: the tree gives back the reader's events, and the same text.
#[test]
fn xml_round_trips() {
    let s = strings();
    let set = GrammarSet::schemaless();
    let g = set.attach(&s).unwrap();
    let mut n = 0;
    for dir in ["fixtures/input", "fixtures/input/infer"] {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if !p.extension().is_some_and(|x| x == "xml" || x == "svg") {
                continue;
            }
            let xml = std::fs::read(&p).unwrap();
            let doc =
                Document::parse_xml(&s, &xml).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert_eq!(
                doc_events(&doc),
                joined(xml_events(&g, &xml)),
                "{}",
                p.display()
            );
            // As text: the same as the reader's events written directly.
            let mut w = XmlWriter::new(
                Vec::new(),
                XmlWriterConfig::new().prefixes(PrefixPolicy::FromEvents),
            );
            for ev in xml_events(&g, &xml) {
                w.event(&ev.as_event()).unwrap();
            }
            assert_eq!(
                doc.to_xml().unwrap(),
                String::from_utf8(w.into_sink()).unwrap(),
                "{}",
                p.display()
            );
            n += 1;
        }
    }
    assert!(n >= 20, "{n} documents");
}

fn compile(main: &str) -> GrammarSet {
    let mut c = exi::xsd::XsdCompiler::new();
    let docs: Vec<(String, Vec<u8>)> = std::fs::read_dir("fixtures/input")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "xsd"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    for (n, b) in &docs {
        c = c.document(n, b);
    }
    c.compile(main).unwrap()
}

fn encode(g: &Grammars<'_>, xml: &[u8], strict: bool) -> Vec<u8> {
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .strict(strict)
        .build()
        .unwrap();
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, g)).unwrap();
    let mut x: XmlReader =
        XmlReader::new(XmlConfig::new().report_comments(false).report_pis(false), g);
    let mut rest = xml;
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        if exi::transcode(&mut x, &mut w).unwrap() == Status::End {
            return w.into_sink();
        }
    }
}

/// A strict stream with typed values: the tree keeps them typed, and writes the same
/// stream back.
#[test]
fn typed_values_survive() {
    let s = strings();
    for (xsd, xml) in [
        ("complex.xsd", "complex.xml"),
        ("types.xsd", "types.xml"),
        ("pretty.xsd", "pretty.xml"),
    ] {
        let set = compile(xsd);
        let g = set.attach(&s).unwrap();
        let exi = encode(
            &g,
            &std::fs::read(Path::new("fixtures/input").join(xml)).unwrap(),
            true,
        );
        let mut r: Reader = Reader::new(ReaderConfig::new(&g)).unwrap();
        r.feed(&exi);
        r.end_input();
        let doc = Document::read(&s, &mut r).unwrap();
        let opts = Options::builder()
            .coding(Coding::ByteAligned)
            .strict(true)
            .build()
            .unwrap();
        let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &g)).unwrap();
        doc.write(&mut w).unwrap();
        assert_eq!(w.into_sink(), exi, "{xml}");
    }
    // A typed value is typed in the tree, and text on request.
    let set = compile("types.xsd");
    let g = set.attach(&s).unwrap();
    let exi = encode(
        &g,
        &std::fs::read("fixtures/input/types.xml").unwrap(),
        true,
    );
    let mut r: Reader = Reader::new(ReaderConfig::new(&g)).unwrap();
    r.feed(&exi);
    r.end_input();
    let doc = Document::read(&s, &mut r).unwrap();
    let typed = descendants(&doc, doc.root().unwrap())
        .into_iter()
        .filter(|&n| doc.kind(n) == NodeKind::Text && !matches!(doc.value(n), Some(Value::Str(_))))
        .count();
    assert!(typed > 3, "{typed} typed values");
}

fn descendants(doc: &Document<'_>, n: exi::dom::NodeId) -> Vec<exi::dom::NodeId> {
    let mut out = vec![n];
    for c in doc.children(n) {
        out.extend(descendants(doc, c));
    }
    out
}

#[test]
fn edits_like_libxml2() {
    let s = strings();
    let mut doc = Document::parse_xml(
        &s,
        b"<list><item>a</item><item>b</item><item>c</item></list>",
    )
    .unwrap();
    let list = doc.root().unwrap();
    let items: Vec<_> = doc.children(list).collect();
    // Move the last item first.
    doc.insert_before(items[0], items[2]).unwrap();
    assert_eq!(
        doc.to_xml().unwrap(),
        "<list><item>c</item><item>a</item><item>b</item></list>"
    );
    // Remove one: its id is no longer valid.
    doc.remove(items[1]);
    assert!(!doc.contains(items[1]));
    assert_eq!(doc.child_element_count(list), 2);
    // Replace, then the old node is unlinked but still ours.
    let new = doc.new_element("new").unwrap();
    doc.replace(items[2], new).unwrap();
    assert_eq!(doc.parent(items[2]), None);
    doc.append_child(list, items[2]).unwrap();
    assert_eq!(
        doc.to_xml().unwrap(),
        "<list><new/><item>a</item><item>c</item></list>"
    );
    // Content.
    doc.set_content(new, "x < y").unwrap();
    doc.add_content(new, " & more").unwrap();
    assert_eq!(doc.content(new), "x < y & more");
    assert_eq!(doc.content(list), "x < y & moreac");
    assert_eq!(
        doc.to_xml().unwrap(),
        "<list><new>x &lt; y &amp; more</new><item>a</item><item>c</item></list>"
    );
    // Copies: shallow keeps attributes, deep keeps children.
    doc.set_attribute(items[2], "n", "2").unwrap();
    let shallow = doc.copy(items[2], false).unwrap();
    let deep = doc.copy(items[2], true).unwrap();
    doc.append_child(list, shallow).unwrap();
    doc.append_child(list, deep).unwrap();
    assert_eq!(
        doc.to_xml().unwrap(),
        r#"<list><new>x &lt; y &amp; more</new><item>a</item><item n="2">c</item><item n="2"/><item n="2">c</item></list>"#
    );
    // No cycles, and only one root.
    assert!(doc.append_child(new, list).is_err());
    let other = doc.new_element("other").unwrap();
    assert!(doc.append_child(doc.document_node(), other).is_err());
    assert_eq!(doc.set_root(other).unwrap(), Some(list));
    assert_eq!(doc.to_xml().unwrap(), "<other/>");
}

#[test]
fn replace_with_a_neighbour() {
    let s = strings();
    let mut doc = Document::parse_xml(&s, b"<a><b/><c/><d/></a>").unwrap();
    let kids: Vec<_> = doc.children(doc.root().unwrap()).collect();
    doc.replace(kids[0], kids[1]).unwrap();
    assert_eq!(doc.to_xml().unwrap(), "<a><c/><d/></a>");
    doc.replace(kids[2], kids[1]).unwrap();
    assert_eq!(doc.to_xml().unwrap(), "<a><c/></a>");
}

#[test]
#[should_panic(expected = "is not a live node")]
fn freed_ids_panic() {
    let s = strings();
    let mut doc = Document::parse_xml(&s, b"<a><b/></a>").unwrap();
    let b = doc.first_child(doc.root().unwrap()).unwrap();
    doc.remove(b);
    let c = doc.new_element("c").unwrap(); // reuses b's slot
    assert_ne!(b, c);
    doc.kind(b);
}

#[test]
fn namespaces() {
    let s = strings();
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="#a"/><g/></svg>"##;
    let mut doc = Document::parse_xml(&s, svg).unwrap();
    let root = doc.root().unwrap();
    let use_ = doc.first_element_child(root).unwrap();
    assert_eq!(doc.name(use_).unwrap().uri(), "http://www.w3.org/2000/svg");
    assert_eq!(
        doc.search_ns(use_, Some("xlink")),
        Some("http://www.w3.org/1999/xlink")
    );
    assert_eq!(
        doc.search_ns_by_href(use_, "http://www.w3.org/2000/svg"),
        Some(None)
    );
    let href = doc
        .attribute_node(use_, ("http://www.w3.org/1999/xlink", "href"))
        .unwrap();
    assert_eq!(doc.name(href).unwrap().prefix(), Some("xlink"));

    // An element in no namespace under the SVG default needs xmlns="".
    let plain = doc.new_element("plain").unwrap();
    doc.append_child(root, plain).unwrap();
    // An element in a new namespace gets a declaration where it is placed.
    let m = doc
        .new_element(Name::new("http://www.w3.org/1998/Math/MathML", "math").with_prefix("m"))
        .unwrap();
    doc.append_child(root, m).unwrap();
    doc.set_attribute(m, ("http://www.w3.org/XML/1998/namespace", "lang"), "en")
        .unwrap();
    assert_eq!(
        doc.to_xml().unwrap(),
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="#a"/><g/><plain xmlns=""/><m:math xmlns:m="http://www.w3.org/1998/Math/MathML" xml:lang="en"/></svg>"##
    );

    // Moving an element into another document: names and declarations come along.
    let s2 = strings();
    let mut other = Document::parse_xml(
        &s2,
        b"<html xmlns='http://www.w3.org/1999/xhtml'><body/></html>",
    )
    .unwrap();
    let body = other.first_element_child(other.root().unwrap()).unwrap();
    let copied = other.import(&doc, use_, true).unwrap();
    other.append_child(body, copied).unwrap();
    assert_eq!(
        other.to_xml().unwrap(),
        r##"<html xmlns="http://www.w3.org/1999/xhtml"><body><use xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xlink:href="#a"/></body></html>"##
    );
}

/// One stanza of an open stream, written with the declarations it needs.
#[test]
fn subtrees_carry_their_scope() {
    let s = strings();
    let mut b = exi::dom::DocumentBuilder::new(&s);
    let set = GrammarSet::schemaless();
    let g = set.attach(&s).unwrap();
    let mut x: XmlReader = XmlReader::new(XmlConfig::new(), &g);
    x.feed(br#"<stream:stream xmlns="jabber:client" xmlns:stream="http://etherx.jabber.org/streams"><message to="juliet@example.com"><body>Hi</body></message>"#);
    while let Step::Event(e, m) = x.next().unwrap() {
        b.event_at(&e, Some(&m)).unwrap();
    }
    let doc = b.document();
    let msg = doc.first_element_child(doc.root().unwrap()).unwrap();
    let mut w = XmlWriter::new(
        Vec::new(),
        XmlWriterConfig::new().prefixes(PrefixPolicy::FromEvents),
    );
    doc.write_subtree(msg, &mut w).unwrap();
    assert_eq!(
        String::from_utf8(w.into_sink()).unwrap(),
        r#"<message xmlns="jabber:client" xmlns:stream="http://etherx.jabber.org/streams" to="juliet@example.com"><body>Hi</body></message>"#
    );
}

#[test]
fn doctype_comments_pis_and_lines() {
    let s = strings();
    let xml = b"<?xml version='1.0'?>\n<!DOCTYPE note SYSTEM \"note.dtd\">\n<!-- first -->\n<note>\n  <?app keep?>\n  <to>Tove</to>\n</note>";
    let doc = Document::parse_xml(&s, xml).unwrap();
    let dt = doc.doctype().unwrap();
    assert_eq!((dt.name, dt.system_id), ("note", "note.dtd"));
    let kinds: Vec<_> = doc
        .children(doc.document_node())
        .map(|n| doc.kind(n))
        .collect();
    assert_eq!(
        kinds,
        [NodeKind::Doctype, NodeKind::Comment, NodeKind::Element]
    );
    let note = doc.root().unwrap();
    let to = doc.first_element_child(note).unwrap();
    // SAFETY: reading the C struct of a live element of a borrowed document.
    let line = unsafe { (*(doc.raw(to) as *const exi::dom::raw::RawNode)).line };
    assert_eq!(line, 6);
    assert!(doc.to_xml().unwrap().contains("<?app keep?>"));
}

/// The C view: libxml2-style walks over the structs see the same tree.
#[test]
fn raw_walk() {
    use exi::dom::raw::*;
    let s = strings();
    let doc = Document::parse_xml(&s, b"<a x='1'><b/>text<c/></a>").unwrap();
    let root = doc.raw(doc.root().unwrap()) as *const RawNode;
    let mut names = Vec::new();
    // SAFETY: a walk over live nodes of a borrowed document, as C code would do it.
    unsafe {
        let mut cur = (*root).children;
        while !cur.is_null() {
            names.push((
                std::ffi::CStr::from_ptr((*cur).name.cast())
                    .to_str()
                    .unwrap(),
                (*cur).r#type,
            ));
            cur = (*cur).next;
        }
        let attr = (*root).properties;
        assert_eq!(
            std::ffi::CStr::from_ptr((*attr).name.cast())
                .to_str()
                .unwrap(),
            "x"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr((*(*attr).children).content.cast())
                .to_str()
                .unwrap(),
            "1"
        );
        assert_eq!((*attr).parent as *const RawNode, root);
        assert_eq!((*root).parent as *const RawDoc, doc.as_raw());
        // Name ids are the global ids.
        let g = doc.global_name(doc.root().unwrap()).unwrap();
        assert_eq!((*root).localId, g.local.index());
    }
    assert_eq!(
        names,
        [
            ("b", EXI_ELEMENT_NODE),
            ("text", EXI_TEXT_NODE),
            ("c", EXI_ELEMENT_NODE)
        ]
    );
}

/// An unlinked element whose namespace was declared on a parent that is then freed.
#[test]
fn unlinked_nodes_outlive_their_old_scope() {
    let s = strings();
    let mut doc = Document::parse_xml(
        &s,
        br#"<r><p:outer xmlns:p="urn:p"><p:inner p:a="1"/></p:outer><home/></r>"#,
    )
    .unwrap();
    let r = doc.root().unwrap();
    let outer = doc.first_element_child(r).unwrap();
    let inner = doc.first_element_child(outer).unwrap();
    doc.unlink(inner);
    doc.remove(outer); // frees the declaration of p
    let home = doc.last_element_child(r).unwrap();
    doc.append_child(home, inner).unwrap();
    assert_eq!(
        doc.to_xml().unwrap(),
        r#"<r><home><p:inner xmlns:p="urn:p" p:a="1"/></home></r>"#
    );
}

#[test]
fn adopt_between_string_tables() {
    let (s1, s2) = (strings(), strings());
    let mut a = Document::parse_xml(
        &s1,
        br#"<a xmlns="urn:a" xmlns:x="urn:x"><b x:k="v" xml:lang="en"><c/></b></a>"#,
    )
    .unwrap();
    let mut b = Document::parse_xml(&s2, b"<z/>").unwrap();
    let node = a.first_element_child(a.root().unwrap()).unwrap();
    let moved = b.adopt(&mut a, node).unwrap();
    assert!(!a.contains(node));
    drop(a); // nothing in b refers to a's tables any more
    b.append_child(b.root().unwrap(), moved).unwrap();
    assert_eq!(
        b.to_xml().unwrap(),
        r#"<z><b xmlns="urn:a" xmlns:x="urn:x" x:k="v" xml:lang="en"><c/></b></z>"#
    );
    assert_eq!(b.global_name(moved), b.lookup_name("urn:a", "b"));
}
