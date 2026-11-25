// tests/canonical.rs
//
// Canonical EXI tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Canonical EXI: one stream for given events, options and grammars. `tests/fixtures.rs`
//! compares the writer's canonical streams with EXIficient's; these tests check the
//! properties that make a stream canonical.
#![cfg(feature = "xml")]

use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{
    Coding, ErrorKind, EventSink, Header, Options, OwnedEvent, Preserve, Reader, ReaderConfig,
    Step, Writer, WriterConfig,
};

fn events_of_xml(g: &Grammars<'_>, xml: &[u8], comments: bool) -> Vec<OwnedEvent> {
    let mut r: XmlReader = XmlReader::new(
        XmlConfig::new()
            .report_comments(comments)
            .report_pis(comments),
        g,
    );
    let mut out = Vec::new();
    let mut rest = xml;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

fn canonical(g: &Grammars<'_>, opts: &Options, evs: &[OwnedEvent]) -> Vec<u8> {
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), g).canonical(true),
    )
    .unwrap();
    for e in evs {
        w.event(&e.as_event()).unwrap();
    }
    w.into_sink()
}

fn decode(g: &Grammars<'_>, exi: &[u8]) -> Vec<OwnedEvent> {
    let mut r: Reader<'_> = Reader::new(ReaderConfig::new(g)).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        at += r.feed(&exi[at..]);
        if at == exi.len() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

/// Writing a canonical stream's events again gives the same octets, in every coding but
/// compression (which Canonical EXI does not use).
#[test]
fn canonical_streams_are_fixed_points() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let docs = [
        "canon-text.xml",
        "channels.xml",
        "prefixes.xml",
        "comments-pis.xml",
        "values.xml",
        "hits.xml",
        "infer/logo.svg",
    ];
    let codings: Vec<Coding> = [
        Coding::ByteAligned,
        Coding::BitPacked,
        Coding::PreCompression,
    ]
    .into_iter()
    .filter(|c| {
        (*c != Coding::BitPacked || cfg!(feature = "bit-packed"))
            && (*c != Coding::PreCompression || cfg!(feature = "compression"))
    })
    .collect();
    for doc in docs {
        let xml = std::fs::read(format!("fixtures/input/{doc}")).unwrap();
        for keep in [
            Preserve::empty(),
            Preserve::PREFIXES | Preserve::COMMENTS | Preserve::PIS,
        ] {
            let evs = events_of_xml(&g, &xml, keep.contains(Preserve::COMMENTS));
            for &c in &codings {
                let opts = Options::builder()
                    .coding(c)
                    .preserve(keep)
                    .block_size(7)
                    .build()
                    .unwrap();
                let once = canonical(&g, &opts, &evs);
                let twice = canonical(&g, &opts, &decode(&g, &once));
                assert_eq!(once, twice, "{doc} {c:?} {keep:?}");
            }
        }
    }
}

/// The order of attributes and namespace declarations, and how text is split, do not show
/// in the stream.
#[test]
fn order_and_splitting_do_not_matter() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .preserve(Preserve::PREFIXES)
        .build()
        .unwrap();
    let a = events_of_xml(
        &g,
        br#"<r xmlns:b="urn:b" xmlns:a="urn:a" z="1" a:y="2" b:y="3" x="4"><p>one two</p></r>"#,
        false,
    );
    let b = events_of_xml(&g, br#"<r xmlns:a="urn:a" xmlns:b="urn:b" x="4" b:y="3" z="1" a:y="2"><p>one<![CDATA[ two]]></p></r>"#, false);
    assert_eq!(canonical(&g, &opts, &a), canonical(&g, &opts, &b));
    // Whitespace between elements is not content; inside xml:space="preserve" it is.
    let tight = events_of_xml(&g, b"<r><p>x</p><q/></r>", false);
    let loose = events_of_xml(&g, b"<r>\n  <p>x</p>\n  <q/>\n</r>", false);
    assert_eq!(canonical(&g, &opts, &tight), canonical(&g, &opts, &loose));
    let kept = events_of_xml(
        &g,
        b"<r xml:space='preserve'>\n  <p>x</p>\n  <q/>\n</r>",
        false,
    );
    let tight_kept = events_of_xml(&g, b"<r xml:space='preserve'><p>x</p><q/></r>", false);
    assert_ne!(
        canonical(&g, &opts, &kept),
        canonical(&g, &opts, &tight_kept)
    );
    // Text alone in an element is content, whitespace or not.
    let blank = events_of_xml(&g, b"<r><p>  </p></r>", false);
    let empty = events_of_xml(&g, b"<r><p/></r>", false);
    assert_ne!(canonical(&g, &opts, &blank), canonical(&g, &opts, &empty));
}

#[test]
fn what_canonical_exi_refuses() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let byte = Options::builder()
        .coding(Coding::ByteAligned)
        .build()
        .unwrap();
    let cookie = WriterConfig::new(byte, &g)
        .canonical(true)
        .header(Header::WithOptions { cookie: true });
    assert_eq!(
        Writer::new(Vec::new(), cookie).err().map(|e| e.kind()),
        Some(ErrorKind::Arg)
    );
    if cfg!(feature = "compression") {
        let opts = Options::builder()
            .coding(Coding::Compression)
            .build()
            .unwrap();
        let z = WriterConfig::new(opts, &g).canonical(true);
        assert_eq!(
            Writer::new(Vec::new(), z).err().map(|e| e.kind()),
            Some(ErrorKind::Arg)
        );
    }
}

#[cfg(feature = "xsd")]
#[test]
fn typed_values_take_one_form() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let xsd = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="f" type="xs:double"/><xs:element name="d" type="xs:decimal"/><xs:element name="t" type="xs:dateTime"/>
      </xs:sequence></xs:complexType></xs:element></xs:schema>"#;
    let set = exi::xsd::XsdCompiler::new()
        .document("s.xsd", xsd)
        .compile("s.xsd")
        .unwrap();
    let g = set.attach(&strings).unwrap();
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .strict(true)
        .build()
        .unwrap();
    let doc = |f: &str, d: &str, t: &str| {
        events_of_xml(
            &g,
            format!("<r><f>{f}</f><d>{d}</d><t>{t}</t></r>").as_bytes(),
            false,
        )
    };
    let a = canonical(
        &g,
        &opts,
        &doc("1.2300E2", "-0.0", "2015-08-11T24:00:00-07:30"),
    );
    let b = canonical(&g, &opts, &doc("123", "0", "2015-08-12T00:00:00-07:30"));
    assert_eq!(a, b);
    // utcTime: the same instant written in UTC.
    let utc = |evs: &[OwnedEvent]| {
        let mut w = Writer::new(
            Vec::new(),
            WriterConfig::new(opts.clone(), &g)
                .canonical(true)
                .utc_time(true),
        )
        .unwrap();
        for e in evs {
            w.event(&e.as_event()).unwrap();
        }
        w.into_sink()
    };
    assert_eq!(
        utc(&doc("1", "1", "2015-08-11T24:00:00-07:30")),
        utc(&doc("1", "1", "2015-08-12T07:30:00Z"))
    );
}
