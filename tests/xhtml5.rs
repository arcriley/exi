// tests/xhtml5.rs
//
// XHTML5 profile tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The XHTML5 profile: its grammars are what the schema compiles to, their digest is
//! pinned, and pages round-trip through it as through schema-less EXI, smaller.
#![cfg(all(feature = "xhtml5", feature = "html5ever"))]

use exi::html::profile;

#[test]
fn digest_is_pinned() {
    assert_eq!(
        profile::grammars(1).unwrap().digest().unwrap(),
        profile::DIGEST_V1
    );
    assert_eq!(
        profile::resolve(profile::SCHEMA_V1).map(|g| g as *const _),
        profile::grammars(1).map(|g| g as *const _)
    );
    assert!(profile::grammars(2).is_none());
}

/// The committed `.exig` is the XSD compiled. Regenerate with
/// `XHTML5_REGEN=1 cargo test --features xhtml5,html5ever,xsd --test xhtml5`
/// (a frozen version must never need it).
#[cfg(feature = "xsd")]
#[test]
fn exig_is_the_schema_compiled() {
    let set = exi::xsd::XsdCompiler::new()
        .document("xhtml5-1.xsd", profile::XSD_V1.as_bytes())
        .compile("xhtml5-1.xsd")
        .unwrap();
    let bytes = set.to_exig().unwrap();
    if std::env::var("XHTML5_REGEN").is_ok() {
        std::fs::write("schemas/xhtml5-1.exig", &bytes).unwrap();
    }
    assert_eq!(
        bytes,
        profile::EXIG_V1,
        "schemas/xhtml5-1.exig is not xhtml5-1.xsd compiled"
    );
}

fn pages() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![std::path::PathBuf::from("fixtures/input/html")];
    if let Ok(d) = std::env::var("EXI_HTML_CORPUS") {
        dirs.push(d.into());
    }
    let mut out: Vec<_> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    out.sort();
    out
}

#[test]
fn pages_round_trip() {
    use exi::grammar::GrammarSet;
    use exi::strings::{GlobalConfig, GlobalStrings};
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let (mut html_total, mut less_total, mut profile_total) = (0, 0, 0);
    for p in pages() {
        let html = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
        let exi = profile::encode(&html, 1).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let back = profile::decode(&exi, 1).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        // The same page as schema-less EXI gives back.
        let opts = exi::Options::builder()
            .coding(exi::Coding::ByteAligned)
            .build()
            .unwrap();
        let mut w = exi::Writer::new(Vec::new(), exi::WriterConfig::new(opts, &g)).unwrap();
        exi::html::encode_html(&html, &mut w, &exi::html::HtmlOptions::new()).unwrap();
        let less = w.into_sink();
        let want = exi::html::decode_to_html(&less, &g).unwrap();
        // Schema-informed streams carry attributes sorted (declared ones must be in
        // schema order), so compare with attributes sorted.
        assert_eq!(sorted_attrs(&back), sorted_attrs(&want), "{}", p.display());
        eprintln!(
            "{}: HTML {}, schema-less EXI {}, XHTML5 profile {}",
            p.display(),
            html.len(),
            less.len(),
            exi.len()
        );
        html_total += html.len();
        less_total += less.len();
        profile_total += exi.len();
    }
    eprintln!(
        "total: HTML {html_total}, schema-less EXI {less_total}, XHTML5 profile {profile_total}"
    );
}

#[test]
fn headers_with_options_name_the_version() {
    let exi = profile::encode_with("<p>x", 1, exi::Header::WithOptions { cookie: false }).unwrap();
    let h = exi::header::parse_header(&exi, &exi::Limits::new())
        .unwrap()
        .unwrap();
    assert!(
        matches!(h.options.unwrap().schema_id(), exi::SchemaId::Id(id) if &**id == profile::SCHEMA_V1)
    );
    assert_eq!(
        profile::decode(&exi, 1).unwrap(),
        "<!DOCTYPE html><html><head></head><body><p>x</p></body></html>"
    );
}

/// The page with every element's attributes sorted by name.
fn sorted_attrs(html: &str) -> String {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
    fn walk(h: &Handle) {
        if let NodeData::Element { attrs, .. } = &h.data {
            attrs
                .borrow_mut()
                .sort_by(|a, b| (&*a.name.local, &*a.name.ns).cmp(&(&*b.name.local, &*b.name.ns)));
        }
        for c in h.children.borrow().iter() {
            walk(c);
        }
    }
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    walk(&dom.document);
    let mut out = Vec::new();
    let h: SerializableHandle = dom.document.clone().into();
    html5ever::serialize::serialize(&mut out, &h, Default::default()).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn parsed_tags_match_the_event_writer_byte_for_byte() {
    use exi::html::{HtmlOptions, encode_html};
    use exi::strings::{GlobalConfig, GlobalStrings};
    use exi::{Header, Writer, WriterConfig};

    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let g = profile::grammars(1).unwrap().attach(&strings).unwrap();
    for html in [
        include_str!("../fixtures/input/html/page.html"),
        include_str!("../fixtures/input/html/fragmentary.html"),
        include_str!("../fixtures/input/html/media-and-forms.html"),
        "<p z='hi' id=a class=b id=c @click=go U000040click=ignored>",
        "<svg><use xlink:href='#p'/><foreignObject><p lang=en>X</p></foreignObject></svg>",
        "<template><table>outside<tr><td a='a\u{c}b'>中\u{ffff}</table></template>",
        "<my-widget z=1 a=2>text<!--gone-->then<my-widget z=3 a=4></my-widget>",
    ] {
        let config = WriterConfig::new(profile::options(1).unwrap(), &g)
            .header(Header::Bare { cookie: false })
            .count_deviations(true);
        let mut writer = Writer::new(Vec::new(), config).unwrap();
        encode_html(html, &mut writer, &HtmlOptions::new()).unwrap();
        let report = profile::report(html, 1).unwrap();
        assert!(report.iter().eq(writer.deviations().unwrap().iter()));
        assert_eq!(profile::encode(html, 1).unwrap(), writer.into_sink());
    }
}

#[test]
fn unsupported_profile_version() {
    use exi::{ErrorKind, Header};
    let valid = profile::encode("<p>ok", 1).unwrap();
    let header = Header::WithOptions { cookie: false };
    for error in [
        profile::options(2).unwrap_err(),
        profile::encode("<p>ok", 2).unwrap_err(),
        profile::encode_with("<p>ok", 2, header).unwrap_err(),
        profile::decode(&valid, 2).unwrap_err(),
        profile::report("<p>ok", 2).unwrap_err(),
    ] {
        assert_eq!(error.kind(), ErrorKind::Arg);
    }
}

#[test]
fn profile_rejects_schemaless_headers() {
    use exi::grammar::GrammarSet;
    use exi::strings::{GlobalConfig, GlobalStrings};
    use exi::{
        Coding, ErrorKind, EventSink, Header, Options, SchemaId, Writer,
        WriterConfig,
    };

    let pool = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&pool).unwrap();
    for id in [SchemaId::Nil, SchemaId::Absent] {
        let options = Options::builder()
            .coding(Coding::ByteAligned)
            .schema_id(id)
            .build()
            .unwrap();
        let config = WriterConfig::new(options, &g)
            .header(Header::WithOptions { cookie: false });
        let mut writer = Writer::new(Vec::new(), config).unwrap();
        writer.start_element("p").unwrap();
        writer.end_element().unwrap();
        writer.end_document().unwrap();
        assert_eq!(
            profile::decode(&writer.into_sink(), 1).unwrap_err().kind(),
            ErrorKind::Schema
        );
    }
}

#[test]
fn profile_rejects_incomplete_pages_and_invalid_root_codes() {
    use exi::{ErrorKind, Header};
    let page = profile::encode_with(
        "<p>hello",
        1,
        Header::WithOptions { cookie: true },
    )
    .unwrap();
    for end in 0..page.len() {
        assert_eq!(
            profile::decode(&page[..end], 1).unwrap_err().kind(),
            ErrorKind::Truncated,
            "prefix of {end} bytes"
        );
    }
    // Bare header followed by a root event outside the v1 document
    // grammar.
    assert_eq!(
        profile::decode(&[0x80, 0xff], 1).unwrap_err().kind(),
        ErrorKind::Grammar
    );
}

#[test]
fn v1_string_attributes_preserve_their_spelling() {
    let html = "<table><tr><td colspan='02'>a<td colspan='not-a-number'>b";
    let back = profile::decode(&profile::encode(html, 1).unwrap(), 1).unwrap();
    assert!(back.contains(
        r#"<td colspan="02">a</td><td colspan="not-a-number">b</td>"#
    ));
    assert!(
        !profile::report(html, 1)
            .unwrap()
            .iter()
            .any(|(_, _, name, _)| name == "{}colspan")
    );
}
