// tests/infer.rs
//
// Schema inference tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Schemas learned from the documents in `fixtures/input/infer` accept those documents in
//! strict streams, and the streams read back to the same elements, attributes and text.
#![cfg(feature = "infer")]

use std::path::Path;

use exi::grammar::{GrammarSet, Grammars};
use exi::infer::{Fit, SchemaInference};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{
    Coding, EventSink, Options, OwnedEvent, OwnedValue, Reader, ReaderConfig, Step, Writer,
    WriterConfig,
};

fn corpus(names: &[&str]) -> Vec<Vec<u8>> {
    names
        .iter()
        .map(|n| std::fs::read(Path::new("fixtures/input/infer").join(n)).unwrap())
        .collect()
}

/// XML text to strict EXI under `g`.
fn encode(g: &Grammars<'_>, xml: &[u8]) -> exi::Result<Vec<u8>> {
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .strict(true)
        .build()?;
    let mut x: XmlReader =
        XmlReader::new(XmlConfig::new().report_comments(false).report_pis(false), g);
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, g))?;
    let mut rest = xml;
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        if exi::transcode(&mut x, &mut w)? == exi::Status::End {
            return Ok(w.into_sink());
        }
    }
}

/// What a strict stream keeps of a document, to compare: element names, attribute names
/// and values as text, and text that is not whitespace.
fn essence(events: impl IntoIterator<Item = OwnedEvent>) -> Vec<String> {
    let mut out = Vec::new();
    let mut attrs: Vec<String> = Vec::new();
    let flush = |out: &mut Vec<String>, attrs: &mut Vec<String>| {
        attrs.sort();
        out.append(attrs);
    };
    for e in events {
        match e {
            OwnedEvent::StartElement(n) => {
                flush(&mut out, &mut attrs);
                out.push(format!(
                    "<{{{}}}{}",
                    n.as_name().uri(),
                    n.as_name().local_name()
                ));
            }
            OwnedEvent::Attribute(n, v) => attrs.push(format!(
                "@{{{}}}{}={}",
                n.as_name().uri(),
                n.as_name().local_name(),
                text(&v)
            )),
            OwnedEvent::Characters(v) => {
                let t = text(&v);
                if !t.trim().is_empty() {
                    flush(&mut out, &mut attrs);
                    out.push(t.trim().to_string());
                }
            }
            OwnedEvent::EndElement(_) => {
                flush(&mut out, &mut attrs);
                out.push(">".into());
            }
            _ => {}
        }
    }
    out
}

/// Values as text, numbers in a canonical form so "9.99" and Decimal(9.99) compare equal.
fn text(v: &OwnedValue) -> String {
    let mut buf = vec![0u8; 4096];
    let s = v.as_value().write_lexical(&mut buf).unwrap().to_string();
    match s.parse::<f64>() {
        Ok(f) if !s.contains(['-', ':']) || s.starts_with('-') && !s.contains(':') => {
            format!("{f}")
        }
        _ => s,
    }
}

fn xml_events(g: &Grammars<'_>, xml: &[u8]) -> Vec<OwnedEvent> {
    let mut x: XmlReader =
        XmlReader::new(XmlConfig::new().report_comments(false).report_pis(false), g);
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

fn exi_events(g: &Grammars<'_>, exi: &[u8]) -> Vec<OwnedEvent> {
    let mut r: Reader = Reader::new(ReaderConfig::new(g)).unwrap();
    r.feed(exi);
    r.end_input();
    let mut out = Vec::new();
    loop {
        match r.next().unwrap() {
            Step::Event(e, _) => out.push(e.to_owned()),
            Step::End => return out,
            Step::NeedInput => panic!("all input given"),
        }
    }
}

fn learn(fit: Fit, docs: &[Vec<u8>]) -> GrammarSet {
    let mut l = SchemaInference::new().fit(fit);
    for d in docs {
        l.document(d).unwrap();
    }
    match l.grammars() {
        Ok(g) => g,
        Err(e) => {
            for s in l.schemas() {
                eprintln!("--- {}\n{}", s.location, s.text);
            }
            panic!("{e}");
        }
    }
}

fn round_trip(names: &[&str]) {
    let docs = corpus(names);
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    for fit in [Fit::General, Fit::Exact] {
        let set = learn(fit, &docs);
        let g = set.attach(&strings).unwrap();
        for (n, d) in names.iter().zip(&docs) {
            let exi = encode(&g, d).unwrap_or_else(|e| panic!("{n} {fit:?}: {e}"));
            assert_eq!(
                essence(exi_events(&g, &exi)),
                essence(xml_events(&g, d)),
                "{n} {fit:?}"
            );
            eprintln!(
                "{n} {fit:?}: {} octets of XML, {} of EXI",
                d.len(),
                exi.len()
            );
        }
    }
}

#[test]
fn svg() {
    round_trip(&["logo.svg"]);
}

#[test]
fn atom() {
    round_trip(&["feed.xml"]);
}

#[test]
fn orders_in_two_orders() {
    round_trip(&["orders-1.xml", "orders-2.xml"]);
}

#[test]
fn every_document_at_once() {
    round_trip(&["logo.svg", "feed.xml", "orders-1.xml", "orders-2.xml"]);
}

/// The learned model: a choice where the examples disagree on order, a sequence where they
/// agree, and Exact's enumeration.
#[test]
fn orders_model() {
    let mut l = SchemaInference::new().fit(Fit::Exact);
    for d in corpus(&["orders-1.xml", "orders-2.xml"]) {
        l.document(&d).unwrap();
    }
    let t = &l.schemas()[0].text;
    assert!(t.contains("<xs:element name=\"order\">\n    <xs:complexType>\n      <xs:choice minOccurs=\"0\" maxOccurs=\"unbounded\">"), "{t}");
    assert!(t.contains("<xs:element name=\"orders\">\n    <xs:complexType>\n      <xs:sequence>\n        <xs:element ref=\"order\" maxOccurs=\"unbounded\"/>"), "{t}");
    assert!(
        t.contains("<xs:enumeration value=\"open\"/>\n      <xs:enumeration value=\"shipped\"/>"),
        "{t}"
    );
    assert!(
        t.contains("<xs:attribute name=\"paid\" type=\"xs:boolean\" use=\"required\"/>"),
        "{t}"
    );
    assert!(
        t.contains("<xs:element name=\"date\" type=\"xs:date\"/>"),
        "{t}"
    );
}

/// Writes the learned schemas to `fixtures/input/inferred/<kind>/` for EXIficient
/// (`fixtures/manifest.tsv`). Run with `--ignored` after changing inference.
#[test]
#[ignore]
fn write_inferred_fixtures() {
    for (kind, names) in [
        ("svg", &["logo.svg"][..]),
        ("atom", &["feed.xml"][..]),
        ("orders", &["orders-1.xml", "orders-2.xml"][..]),
    ] {
        for fit in [Fit::General, Fit::Exact] {
            let mut l = SchemaInference::new().fit(fit);
            for d in corpus(names) {
                l.document(&d).unwrap();
            }
            let dir = Path::new("fixtures/input/inferred").join(format!(
                "{kind}-{}",
                if fit == Fit::Exact {
                    "exact"
                } else {
                    "general"
                }
            ));
            std::fs::create_dir_all(&dir).unwrap();
            for s in l.schemas() {
                std::fs::write(dir.join(&s.location), &s.text).unwrap();
            }
        }
    }
}

#[test]
fn as_an_event_sink() {
    // Any event source feeds it: here the events of one document, given one by one.
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let mut l = SchemaInference::new();
    for e in xml_events(&g, b"<a><b>1</b><b>2</b></a>") {
        l.event(&e.as_event()).unwrap();
    }
    assert!(
        l.schemas()[0]
            .text
            .contains("<xs:element name=\"b\" type=\"xs:unsignedInt\"/>")
    );
}

/// Each value seen widens a slot's type just enough: from one document to the next, the
/// type is the narrowest that fits every value so far.
#[test]
fn types_widen_across_documents() {
    let declared = |values: &[&str]| -> String {
        let mut l = SchemaInference::new();
        for v in values {
            l.document(format!("<r><a>{v}</a></r>").as_bytes()).unwrap();
        }
        let text = l.schemas()[0].text.clone();
        let line = text.lines().find(|l| l.contains(r#"name="a""#)).unwrap();
        let ty = line.split(r#"type="xs:"#).nth(1).unwrap();
        ty[..ty.find('"').unwrap()].to_string()
    };
    for (values, ty) in [
        (&["true", "false"][..], "boolean"),
        (&["1", "0"], "unsignedInt"), // 0 and 1 are integers before they are booleans
        (&["7", "-3"], "int"),
        (&["7", "5000000000"], "unsignedLong"),
        (&["7", "-5000000000"], "long"),
        (&["7", "2.5"], "decimal"),
        (&["7", "2.5", "1e3"], "double"),
        (&["2026-11-27", "2026-11-28"], "date"),
        (&["2026-11-27", "soon"], "string"),
        (&["7", ""], "string"), // an empty element is the empty string
    ] {
        assert_eq!(declared(values), ty, "{values:?}");
    }
}

/// A schema learned with `Fit::Exact` declares only what the examples showed, so a
/// strict stream refuses values outside it; `Fit::General` takes them.
#[test]
fn exact_fits_refuse_what_was_not_seen() {
    let examples = [br#"<log><e level="info" code="200"/><e level="warn" code="404"/><e level="info" code="200"/><e level="warn" code="301"/></log>"#.to_vec()];
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let exact = learn(Fit::Exact, &examples);
    let general = learn(Fit::General, &examples);
    let (ge, gg) = (exact.attach(&strings).unwrap(), general.attach(&strings).unwrap());
    let doc = |level: &str, code: &str| {
        format!(r#"<log><e level="{level}" code="{code}"/></log>"#).into_bytes()
    };
    encode(&ge, &examples[0]).unwrap();
    encode(&ge, &doc("warn", "250")).unwrap(); // inside the range
    for (level, code) in [("info", "500"), ("info", "199"), ("debug", "200")] {
        let e = encode(&ge, &doc(level, code)).unwrap_err();
        assert_eq!(e.kind(), exi::ErrorKind::Unencodable, "{level} {code}: {e}");
        encode(&gg, &doc(level, code)).unwrap();
    }
}
