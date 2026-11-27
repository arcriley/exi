// tests/html.rs
//
// HTML round-trip tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! HTML pages through EXI and back. Each page goes HTML → events → EXI → HTML, and the
//! result must equal html5ever's own serialization of the parsed page: what the tree
//! holds survives, coerced names and all.
//!
//! The pages are `fixtures/input/html/*.html`, and every `.html` file in the directory
//! `EXI_HTML_CORPUS` names, when set (try a saved copy of any website).
#![cfg(feature = "html5ever")]

use std::path::{Path, PathBuf};

use exi::grammar::{GrammarSet, Grammars};
use exi::html::{HtmlOptions, decode_to_html, encode_html};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{Coding, Options, Preserve, Writer, WriterConfig};
use html5ever::serialize::{SerializeOpts, serialize};
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{RcDom, SerializableHandle};

fn pages() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("fixtures/input/html")];
    if let Ok(d) = std::env::var("EXI_HTML_CORPUS") {
        dirs.push(d.into());
    }
    let mut out = Vec::new();
    for d in dirs {
        for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "html" || x == "htm") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// What html5ever writes for the parsed page, with `<template>` contents moved into
/// the element (as `encode_html` gives them) and a DOCTYPE first.
fn expected(html: &str) -> String {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    move_templates(&dom.document);
    let mut out = Vec::new();
    let h: SerializableHandle = dom.document.clone().into();
    serialize(&mut out, &h, SerializeOpts::default()).unwrap();
    let s = String::from_utf8(out).unwrap();
    let s = s.strip_prefix("<!DOCTYPE html>").unwrap_or(&s).to_string();
    // Form feed as a space; characters XML does not allow as U+FFFD.
    let xml_chars = |c: char| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..);
    let s: String = s
        .chars()
        .map(|c| {
            if c == '\u{c}' {
                ' '
            } else if xml_chars(c) {
                c
            } else {
                '\u{fffd}'
            }
        })
        .collect();
    format!("<!DOCTYPE html>{}", xml_comments(&s))
}

/// Comments as XML allows them: `--` inside as `- -`, a final `-` followed by a space.
fn xml_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i + 4]);
        rest = &rest[i + 4..];
        let end = rest.find("-->").unwrap_or(rest.len());
        let mut c = String::new();
        for ch in rest[..end].chars() {
            if ch == '-' && c.ends_with('-') {
                c.push(' ');
            }
            c.push(ch);
        }
        if c.ends_with('-') {
            c.push(' ');
        }
        out.push_str(&c);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn move_templates(h: &markup5ever_rcdom::Handle) {
    if let markup5ever_rcdom::NodeData::Element { attrs, .. } = &h.data {
        // xmlns attributes are dropped: names carry their namespaces.
        attrs.borrow_mut().retain(|a| {
            !(&*a.name.ns == "http://www.w3.org/2000/xmlns/"
                || (a.name.ns.is_empty()
                    && (&*a.name.local == "xmlns" || a.name.local.starts_with("xmlns:"))))
        });
    }
    if let markup5ever_rcdom::NodeData::Element {
        template_contents, ..
    } = &h.data
    {
        if let Some(t) = template_contents.borrow_mut().take() {
            let kids: Vec<_> = t.children.borrow_mut().drain(..).collect();
            h.children.borrow_mut().extend(kids);
        }
    }
    for c in h.children.borrow().iter() {
        move_templates(c);
    }
}

fn encode(html: &str, g: &Grammars<'_>, preserve: Preserve) -> Vec<u8> {
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .preserve(preserve)
        .build()
        .unwrap();
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, g)).unwrap();
    encode_html(html, &mut w, &HtmlOptions::new()).unwrap();
    w.into_sink()
}

fn text(p: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned()
}

#[test]
fn pages_round_trip() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let mut total = (0, 0);
    for p in pages() {
        let html = text(&p);
        let exi = encode(&html, &g, Preserve::COMMENTS | Preserve::PIS);
        let back = decode_to_html(&exi, &g).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let want = expected(&html);
        if back != want {
            let at = back
                .bytes()
                .zip(want.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| back.len().min(want.len()));
            let from = at.saturating_sub(80);
            panic!(
                "{}: differs at {at}\n ours: {:?}\n html5ever: {:?}",
                p.display(),
                &back[from..(at + 80).min(back.len())],
                &want[from..(at + 80).min(want.len())]
            );
        }
        eprintln!(
            "{}: {} octets of HTML, {} of EXI",
            p.display(),
            html.len(),
            exi.len()
        );
        total.0 += html.len();
        total.1 += exi.len();
    }
    eprintln!("total: {} octets of HTML, {} of EXI", total.0, total.1);
}

#[test]
fn what_changes_on_the_way_in() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let html = text(Path::new("fixtures/input/html/page.html"));
    // As XHTML: coerced names, comments with spaces, form feed as space.
    let x = exi::html::html_to_xhtml(&html).unwrap();
    assert!(x.contains(r#"U000040click="go()""#), "{x}");
    assert!(x.contains(r#"U00003Aclass="{x: y}""#), "{x}");
    assert!(x.contains("<!-- a comment - - with dashes -->"), "{x}");
    assert!(x.contains("Form feed: here"), "{x}");
    assert!(
        x.contains(r#"<svg xmlns="http://www.w3.org/2000/svg""#),
        "{x}"
    );
    assert!(x.contains(r##"xlink:href="#top""##), "{x}");
    // Without Preserve.comments the comment is gone.
    let back = decode_to_html(&encode(&html, &g, Preserve::empty()), &g).unwrap();
    assert!(!back.contains("<!--"));
    // The XHTML parses as XML.
    #[cfg(feature = "dom")]
    exi::dom::Document::parse_xml(&strings, x.as_bytes()).unwrap();
}

/// `xml` and `xmlns` name only their own namespaces (Namespaces in XML
/// §3). A tree that gives one of them to another URI keeps the names
/// and loses the prefix.
#[test]
fn reserved_prefixes_on_other_namespaces() {
    use exi::html::HtmlSerializer;
    use exi::{OwnedEvent, OwnedName, OwnedValue};
    use markup5ever::serialize::Serializer;
    use markup5ever::{LocalName, Namespace, Prefix, QualName};
    let q = |prefix: &str, ns: &str, local: &str| {
        QualName::new(
            Some(Prefix::from(prefix)),
            Namespace::from(ns),
            LocalName::from(local),
        )
    };
    let el = q("xml", "urn:e", "el");
    let attrs = [
        (q("xmlns", "urn:a", "a"), "1"),
        (q("xml", "http://www.w3.org/XML/1998/namespace", "lang"), "en"),
    ];
    let mut events = Vec::<OwnedEvent>::new();
    let mut s = HtmlSerializer::new(&mut events, HtmlOptions::new());
    s.start_elem(el.clone(), attrs.iter().map(|(q, v)| (q, *v))).unwrap();
    s.end_elem(el).unwrap();
    s.finish().unwrap();
    let str = |v: &str| OwnedValue::Str(v.into());
    assert_eq!(
        events[1..4],
        [
            OwnedEvent::StartElement(OwnedName::new("urn:e", "el")),
            OwnedEvent::Attribute(OwnedName::new("urn:a", "a"), str("1")),
            OwnedEvent::Attribute(
                OwnedName::new("http://www.w3.org/XML/1998/namespace", "lang")
                    .with_prefix("xml"),
                str("en"),
            ),
        ]
    );
}

#[test]
fn parser_coercion_and_doctype_options() {
    use exi::ErrorKind;
    use exi::html::{Coercion, html_to_xhtml_with};
    let reject = HtmlOptions::new().coercion(Coercion::Reject);
    for html in ["<p @click=go>", "<bad@name>"] {
        assert_eq!(
            html_to_xhtml_with(html, &reject).unwrap_err().kind(),
            ErrorKind::Xml
        );
    }
    let html = "<!doctype html><p @click=go id=x>";
    let drop = HtmlOptions::new().coercion(Coercion::Drop);
    let without = html_to_xhtml_with(html, &drop).unwrap();
    assert!(without.contains(r#"<p id="x"/>"#));
    assert!(!without.contains("<!DOCTYPE"));
    let with = html_to_xhtml_with(html, &drop.keep_doctype(true)).unwrap();
    assert!(with.starts_with("<!DOCTYPE html>"));
}

#[test]
fn implied_elements_count_toward_the_writer_depth_limit() {
    let pool = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&pool).unwrap();
    let options =
        Options::builder().coding(Coding::ByteAligned).build().unwrap();
    let write = |depth| {
        let config = WriterConfig::new(options.clone(), &g)
            .limits(exi::Limits::new().max_depth(depth));
        let mut writer = Writer::new(Vec::new(), config)?;
        encode_html("<p><b>hi", &mut writer, &HtmlOptions::new())
    };
    write(4).unwrap(); // html > body > p > b
    assert_eq!(write(3).unwrap_err().kind(), exi::ErrorKind::Limit);
}
