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
//!
//! The tests that compile schemas also need the `xsd` feature; the others run with `dom`
//! and `xml` alone.
#![cfg(all(feature = "dom", feature = "xml"))]

use exi::dom::{Document, NodeId, NodeKind};
use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{PrefixPolicy, XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    Coding, ErrorKind, EventSink, Name, Options, OwnedEvent, OwnedValue, Reader, ReaderConfig,
    Step, Writer, WriterConfig,
};
#[cfg(feature = "xsd")]
use exi::Value;

fn strings() -> GlobalStrings {
    GlobalStrings::new(GlobalConfig::new()).unwrap()
}

/// Live strings in the pool's four tables: a tree that is dropped, or never finished,
/// leaves none behind.
fn live(s: &GlobalStrings) -> u32 {
    s.uris().stats().strings
        + s.prefixes().stats().strings
        + s.locals().stats().strings
        + s.values().stats().strings
}

/// A tree read from a complete EXI stream.
fn read_exi<'g>(
    s: &'g GlobalStrings,
    g: &Grammars<'_>,
    exi: &[u8],
) -> exi::Result<Document<'g>> {
    let mut r: Reader = Reader::new(ReaderConfig::new(g)).unwrap();
    r.feed(exi);
    r.end_input();
    Document::read(s, &mut r)
}

/// A tree written as EXI with these options.
fn write_exi(doc: &Document<'_>, g: &Grammars<'_>, strict: bool) -> exi::Result<Vec<u8>> {
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .strict(strict)
        .build()
        .unwrap();
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, g)).unwrap();
    doc.write(&mut w)?;
    Ok(w.into_sink())
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

#[cfg(feature = "xsd")]
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

#[cfg(feature = "xsd")]
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
        if exi::transcode(&mut x, &mut w).unwrap() == exi::Status::End {
            return w.into_sink();
        }
    }
}

/// A strict stream with typed values: the tree keeps them typed, and writes the same
/// stream back.
#[cfg(feature = "xsd")]
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
            &std::fs::read(std::path::Path::new("fixtures/input").join(xml)).unwrap(),
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

#[cfg(feature = "xsd")]
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

/// Text longer than a node holds inline lives in the document's text
/// arena; an adopted node takes its own copy, so it outlives the source.
#[test]
fn adopted_long_text_outlives_its_document() {
    let s = strings();
    let long = "x".repeat(100);
    let huge = "y".repeat(5000); // past the arena: a heap buffer
    let xml = format!(r#"<a><b k="{long}">{long}<!--{long}--><c>{huge}</c></b></a>"#);
    let mut a = Document::parse_xml(&s, xml.as_bytes()).unwrap();
    let mut b = Document::parse_xml(&s, b"<z/>").unwrap();
    let node = a.first_element_child(a.root().unwrap()).unwrap();
    let moved = b.adopt(&mut a, node).unwrap();
    drop(a); // frees a's text arena
    b.append_child(b.root().unwrap(), moved).unwrap();
    assert_eq!(
        b.to_xml().unwrap(),
        format!(r#"<z><b k="{long}">{long}<!--{long}--><c>{huge}</c></b></z>"#)
    );
    // The C view reads the same text through `content`.
    let text = b.first_child(moved).unwrap();
    // SAFETY: reading a live text node of a borrowed document.
    let content = unsafe {
        std::ffi::CStr::from_ptr((*b.raw(text).cast::<exi::dom::raw::RawNode>()).content.cast())
    };
    assert_eq!(content.to_str().unwrap(), long);
}

/// A typed tree from a strict stream, edited and written to a strict writer again. Text
/// set on a typed node is a string until the writer parses it for the node's type, so a
/// good edit encodes and a bad one is `Unencodable`, never stale octets.
#[cfg(feature = "xsd")]
#[test]
fn edited_typed_trees_write_strict_exi() {
    let s = strings();
    let set = compile("types.xsd");
    let g = set.attach(&s).unwrap();
    let exi = encode(&g, &std::fs::read("fixtures/input/types.xml").unwrap(), true);
    let fresh = || read_exi(&s, &g, &exi).unwrap();
    // <i> holds Int(-42); the attribute n is a small integer, 10 to 20.
    let doc = fresh();
    let root = doc.root().unwrap();
    let i = doc.first_element_child(root).unwrap();
    assert_eq!(doc.value(doc.first_child(i).unwrap()), Some(Value::Int(-42)));
    assert_eq!(doc.attribute_value(root, "n"), Some(Value::Int(15)));
    let i_back = |doc: &Document<'_>| {
        let i = doc.first_element_child(doc.root().unwrap()).unwrap();
        doc.value(doc.first_child(i).unwrap()).map(|v| v.to_owned())
    };

    // Content replaced or extended as text is typed again on the way out.
    let mut doc = fresh();
    doc.set_content(i, "7").unwrap();
    let back = read_exi(&s, &g, &write_exi(&doc, &g, true).unwrap()).unwrap();
    assert_eq!(i_back(&back), Some(OwnedValue::Int(7)));
    let mut doc = fresh();
    let t = doc.first_child(i).unwrap();
    doc.add_content(t, "0").unwrap();
    assert_eq!(doc.value(t), Some(Value::Str("-420")));
    let back = read_exi(&s, &g, &write_exi(&doc, &g, true).unwrap()).unwrap();
    assert_eq!(i_back(&back), Some(OwnedValue::Int(-420)));
    let mut doc = fresh();
    doc.set_content(i, "seven").unwrap();
    let e = write_exi(&doc, &g, true).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unencodable, "{e}");

    // An attribute set again keeps its node and place; out of range is refused.
    let mut doc = fresh();
    let n = doc.attribute_node(root, "n").unwrap();
    let order: Vec<_> = doc.attributes(root).collect();
    assert_eq!(doc.set_attribute(root, "n", "12").unwrap(), n);
    assert_eq!(doc.attributes(root).collect::<Vec<_>>(), order);
    let back = read_exi(&s, &g, &write_exi(&doc, &g, true).unwrap()).unwrap();
    assert_eq!(back.attribute_value(back.root().unwrap(), "n"), Some(Value::Int(12)));
    doc.set_attribute(root, "n", "99").unwrap();
    let e = write_exi(&doc, &g, true).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unencodable, "{e}");
    doc.set_attribute_value(root, "n", OwnedValue::Int(11)).unwrap();
    write_exi(&doc, &g, true).unwrap();
    // The attribute is optional: removed, the stream has no n.
    assert!(doc.remove_attribute(root, "n"));
    let back = read_exi(&s, &g, &write_exi(&doc, &g, true).unwrap()).unwrap();
    assert_eq!(back.attribute(back.root().unwrap(), "n"), None);

    // Copies keep typed values; another <s> fits the schema's repeat.
    let mut doc = fresh();
    let c = doc.copy(i, true).unwrap();
    assert_eq!(doc.value(doc.first_child(c).unwrap()), Some(Value::Int(-42)));
    let last = doc.last_element_child(root).unwrap();
    let again = doc.copy(last, true).unwrap();
    doc.append_child(root, again).unwrap();
    let back = read_exi(&s, &g, &write_exi(&doc, &g, true).unwrap()).unwrap();
    assert!(back.to_xml().unwrap().ends_with("<s>y</s><s>y</s></root>"));

    // An element the schema does not declare cannot go into a strict stream.
    let other = Document::parse_xml(&s, br#"<root xmlns="urn:t"><zzz/></root>"#).unwrap();
    let e = write_exi(&other, &g, true).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unencodable, "{e}");
}

/// Both directions of the sibling links agree, and every child names `parent`.
fn assert_linked(doc: &Document<'_>, parent: NodeId) {
    let forward: Vec<_> = doc.children(parent).collect();
    let mut backward = Vec::new();
    let mut at = doc.last_child(parent);
    while let Some(n) = at {
        backward.push(n);
        at = doc.prev_sibling(n);
    }
    backward.reverse();
    assert_eq!(forward, backward);
    assert_eq!(doc.first_child(parent), forward.first().copied());
    for &n in &forward {
        assert_eq!(doc.parent(n), Some(parent));
    }
    let elements: Vec<_> = forward
        .iter()
        .copied()
        .filter(|&n| doc.kind(n) == NodeKind::Element)
        .collect();
    assert_eq!(doc.child_element_count(parent), elements.len());
    assert_eq!(doc.first_element_child(parent), elements.first().copied());
    assert_eq!(doc.last_element_child(parent), elements.last().copied());
    for w in elements.windows(2) {
        assert_eq!(doc.next_element_sibling(w[0]), Some(w[1]));
        assert_eq!(doc.prev_element_sibling(w[1]), Some(w[0]));
    }
}

/// A tree built from nothing, then nodes moved without unlinking them first: placing a
/// node takes it from where it was, and both lists stay linked.
#[test]
fn moves_keep_both_lists_linked() {
    let s = strings();
    let mut doc = Document::new(&s);
    let root = doc.new_element("root").unwrap();
    doc.set_root(root).unwrap();
    let [a, b, c, box_] = ["a", "b", "c", "box"].map(|n| doc.new_element(n).unwrap());
    let text = doc.new_text("t");
    doc.append_child(root, a).unwrap();
    doc.append_child(root, c).unwrap();
    doc.insert_after(a, b).unwrap(); // in the middle
    doc.insert_after(c, text).unwrap(); // at the end: the last child changes
    doc.append_child(root, box_).unwrap();
    assert_eq!(doc.to_xml().unwrap(), "<root><a/><b/><c/>t<box/></root>");
    assert_linked(&doc, root);
    assert_eq!(doc.prev_element_sibling(box_), Some(c));

    // Into another parent, and back between two siblings.
    doc.append_child(box_, b).unwrap();
    doc.insert_before(b, text).unwrap();
    assert_eq!(doc.to_xml().unwrap(), "<root><a/><c/><box>t<b/></box></root>");
    assert_linked(&doc, root);
    assert_linked(&doc, box_);
    doc.insert_after(a, b).unwrap();
    doc.insert_before(a, c).unwrap();
    assert_eq!(doc.to_xml().unwrap(), "<root><c/><a/><b/><box>t</box></root>");
    assert_linked(&doc, root);
    assert_linked(&doc, box_);
    // Next to itself is no move; into its own subtree is refused.
    doc.insert_after(b, b).unwrap();
    assert!(doc.append_child(box_, root).is_err());
    assert!(doc.insert_before(text, box_).is_err());
    assert_eq!(doc.to_xml().unwrap(), "<root><c/><a/><b/><box>t</box></root>");
}

/// Attributes set again keep their node and place, in or out of a namespace, and the C
/// list of attributes stays linked both ways after removals.
#[test]
fn attributes_edit_in_place() {
    use exi::dom::raw::RawAttr;
    let s = strings();
    let mut doc =
        Document::parse_xml(&s, br#"<e xmlns:p="urn:p" a="1" p:b="2" c="3" d="4"/>"#).unwrap();
    let e = doc.root().unwrap();
    let before: Vec<_> = doc.attributes(e).collect();
    assert_eq!(doc.set_attribute(e, "a", "one").unwrap(), before[0]);
    assert_eq!(doc.set_attribute(e, ("urn:p", "b"), "two").unwrap(), before[1]);
    assert_eq!(doc.attributes(e).collect::<Vec<_>>(), before);
    assert_eq!(doc.attribute(e, ("urn:p", "b")).as_deref(), Some("two"));
    // The same local name in no namespace is another attribute.
    assert_eq!(doc.attribute(e, "b"), None);
    assert!(!doc.remove_attribute(e, "b"));

    assert!(doc.remove_attribute(e, ("urn:p", "b"))); // from the middle
    doc.remove(before[3]); // the last, by its node
    assert!(!doc.contains(before[1]) && !doc.contains(before[3]));
    assert_eq!(doc.attributes(e).collect::<Vec<_>>(), [before[0], before[2]]);
    assert_eq!(doc.to_xml().unwrap(), r#"<e xmlns:p="urn:p" a="one" c="3"/>"#);
    // SAFETY: walking the attribute list of a live element of a borrowed document.
    unsafe {
        let first = (*(doc.raw(e) as *const exi::dom::raw::RawNode)).properties;
        let second = (*first).next;
        assert_eq!(first as *const RawAttr, doc.raw(before[0]) as *const RawAttr);
        assert_eq!(second as *const RawAttr, doc.raw(before[2]) as *const RawAttr);
        assert!((*first).prev.is_null() && (*second).next.is_null());
        assert_eq!((*second).prev, first);
    }
    let late = doc.set_attribute(e, "z", "26").unwrap();
    assert_eq!(doc.attributes(e).last(), Some(late));
}

/// Bad input is an error, and the strings of the part-built tree go back to the pool.
#[test]
fn bad_input_is_an_error() {
    let s = strings();
    let set = GrammarSet::schemaless();
    let g = set.attach(&s).unwrap();
    let start = live(&s);
    for (xml, detail) in [
        (&b"<a><b></a>"[..], "does not match"),
        (b"<a><b>text", "ends inside"),
        (b"<a x='1' x='2'/>", ""),
    ] {
        let e = Document::parse_xml(&s, xml).err().expect("an error");
        assert_eq!(e.kind(), ErrorKind::Xml, "{e}");
        assert!(e.to_string().contains(detail), "{e}");
        assert_eq!(live(&s), start, "{e}");
    }

    let doc = Document::parse_xml(&s, br#"<r><p k="v">some text</p><p k="w">more</p></r>"#)
        .unwrap();
    let exi = write_exi(&doc, &g, false).unwrap();
    drop(doc);
    assert_eq!(live(&s), start);
    for cut in [exi.len() - 1, exi.len() / 2, 2] {
        let e = read_exi(&s, &g, &exi[..cut]).err().expect("an error");
        assert_eq!(e.kind(), ErrorKind::Truncated, "cut at {cut}: {e}");
        assert_eq!(live(&s), start, "cut at {cut}");
    }
    // A distinguishing bit that is wrong: not a document, whatever the error is.
    let mut bad = exi.clone();
    bad[0] ^= 0x80;
    assert!(read_exi(&s, &g, &bad).is_err());
    assert_eq!(live(&s), start);
}

#[test]
fn namespace_declarations_are_not_attributes() -> exi::Result<()> {
    let strings = strings();
    let mut doc = Document::new(&strings);
    let root = doc.new_element("root")?;
    doc.set_root(root)?;
    let attr = doc.set_attribute(root, "ordinary", "kept")?;
    let mut builder = exi::dom::DocumentBuilder::new(&strings);
    builder.start_element("root")?;

    for name in [
        Name::local("xmlns"),
        Name::new("urn:example", "bad").with_prefix("xmlns"),
        Name::new("http://www.w3.org/2000/xmlns/", "bad"),
    ] {
        assert_eq!(
            doc.new_attribute(name, "urn:bad").unwrap_err().kind(),
            ErrorKind::Arg
        );
        assert_eq!(
            doc.set_attribute(root, name, "urn:bad").unwrap_err().kind(),
            ErrorKind::Arg
        );
        assert_eq!(doc.rename(attr, name).unwrap_err().kind(), ErrorKind::Arg);
        assert_eq!(
            builder.attribute(name, "urn:bad").unwrap_err().kind(),
            ErrorKind::Arg
        );
    }

    assert_eq!(doc.attribute(root, "ordinary").as_deref(), Some("kept"));
    assert_eq!(doc.attributes(root).count(), 1);
    assert_eq!(
        builder.document().attributes(builder.current().unwrap()).count(),
        0
    );
    Ok(())
}

#[test]
fn builder_rejects_mismatched_end_before_flushing() -> exi::Result<()> {
    let strings = strings();
    let mut builder = exi::dom::DocumentBuilder::new(&strings);
    builder.start_element(("urn:open", "item"))?;
    builder.text("pending")?;
    let node = builder.current().unwrap();
    let end = |uri| exi::Event::EndElement(Name::new(uri, "item"));
    assert_eq!(
        builder.event(&end("urn:other")).unwrap_err().kind(),
        ErrorKind::State
    );
    assert_eq!(builder.current(), Some(node));
    assert_eq!(builder.document().first_child(node), None);
    builder.event(&end("urn:open"))?;
    assert_eq!(builder.document().content(node), "pending");
    Ok(())
}

#[test]
fn adopted_xml_bindings_outlive_the_source() -> exi::Result<()> {
    const XML: &str = "http://www.w3.org/XML/1998/namespace";
    let dest_strings = strings();
    let mut dest = Document::new(&dest_strings);
    let (element, attr) = {
        let source_strings = strings();
        let mut source = Document::parse_xml(
            &source_strings,
            b"<root xml:lang='en'><xml:item xml:space='preserve'/></root>",
        )?;
        let root = source.root().unwrap();
        let element = source.first_element_child(root).unwrap();
        let attr = source.attribute_node(root, (XML, "lang")).unwrap();
        (dest.adopt(&mut source, element)?, dest.adopt(&mut source, attr)?)
    };
    assert_eq!(dest.parent(element), None);
    assert_eq!(dest.name(element).unwrap().prefix(), Some("xml"));
    assert_eq!(dest.name(attr).unwrap().prefix(), Some("xml"));
    dest.set_root(element)?;
    dest.attach_attribute(element, attr)?;
    assert_eq!(
        dest.to_xml()?,
        r#"<xml:item xml:space="preserve" xml:lang="en"/>"#
    );
    Ok(())
}

#[test]
fn failed_adoption_keeps_the_source_tree_intact() -> exi::Result<()> {
    let source_strings = strings();
    let dest_strings = GlobalStrings::new(GlobalConfig::new().max_strings(1))?;
    let mut source =
        Document::parse_xml(&source_strings, b"<root><child/></root>")?;
    let mut dest = Document::new(&dest_strings);
    let root = source.root().unwrap();
    let child = source.first_child(root).unwrap();
    let root_ptr = source.raw(root);
    let child_ptr = source.raw(child);

    assert_eq!(
        dest.adopt(&mut source, root).unwrap_err().kind(),
        ErrorKind::Limit
    );
    assert_eq!(source.root(), Some(root));
    assert_eq!(source.parent(child), Some(root));
    assert_eq!(source.node_id(root_ptr), Some(root));
    assert_eq!(source.node_id(child_ptr), Some(child));
    assert_eq!(source.to_xml()?, "<root><child/></root>");
    assert!(dest.root().is_none());
    Ok(())
}
