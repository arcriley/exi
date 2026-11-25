// tests/roundtrip.rs
//
// XML and EXI round-trip tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Round trips through the whole pipeline: XML text → EXI → XML text, and EXI written and
//! read back with each option that changes the grammars.
#![cfg(feature = "xml")]

use exi::grammar::GrammarSet;
use exi::grammar::informed::{IntegerRep, Prod, Representation};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{PrefixPolicy, XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    Coding, EventSink, Header, Options, OwnedEvent, OwnedName, OwnedValue, Preserve, Reader,
    ReaderConfig, Step, Writer, WriterConfig, transcode,
};

/// XML text to EXI with `opts`, then back to XML text, reading the EXI `chunk` octets at a
/// time.
fn xml_exi_xml(xml: &str, opts: Options, policy: PrefixPolicy, chunk: usize) -> (Vec<u8>, String) {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings).unwrap();
    let cfg = XmlConfig::new().report_doctype(true);
    let mut x: XmlReader = XmlReader::new(cfg, &grammars);
    x.feed(xml.as_bytes());
    x.end_input();
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &grammars)).unwrap();
    transcode(&mut x, &mut w).unwrap();
    let exi = w.into_sink();

    let mut r: Reader = Reader::new(ReaderConfig::new(&grammars)).unwrap();
    let mut out = XmlWriter::new(Vec::new(), XmlWriterConfig::new().prefixes(policy));
    for piece in exi.chunks(chunk) {
        assert_eq!(r.feed(piece), piece.len());
        while let Step::Event(e, _) = r.next().unwrap() {
            out.event(&e).unwrap();
        }
    }
    r.end_input();
    loop {
        match r.next().unwrap() {
            Step::Event(e, _) => out.event(&e).unwrap(),
            Step::End => break,
            Step::NeedInput => panic!("all input was given"),
        }
    }
    (exi.clone(), String::from_utf8(out.into_sink()).unwrap())
}

fn byte() -> exi::OptionsBuilder {
    Options::builder().coding(Coding::ByteAligned)
}

#[test]
fn plain_document() {
    let xml = r#"<doc><p class="x">one</p><p class="x">two &amp; three</p><empty/></doc>"#;
    for chunk in [1, 2, 7, 1000] {
        let (_, back) = xml_exi_xml(xml, byte().build().unwrap(), PrefixPolicy::Generate, chunk);
        assert_eq!(back, xml, "chunk {chunk}");
    }
}

#[test]
fn comments_pis_and_doctype() {
    let xml = r#"<!DOCTYPE doc [<!ENTITY e "x">]><!--c--><?pi data?><doc><!--in--><?p?>t</doc>"#;
    let opts = byte()
        .preserve(Preserve::COMMENTS | Preserve::PIS | Preserve::DTD)
        .build()
        .unwrap();
    let (_, back) = xml_exi_xml(xml, opts, PrefixPolicy::Generate, 3);
    assert_eq!(
        back,
        r#"<!DOCTYPE doc [<!ENTITY e "x">]><!--c--><?pi data?><doc><!--in--><?p?>t</doc>"#
    );
    // Without the flags they are dropped.
    let (_, back) = xml_exi_xml(xml, byte().build().unwrap(), PrefixPolicy::Generate, 3);
    assert_eq!(back, "<doc>t</doc>");
}

#[test]
fn namespaces_and_prefixes() {
    let xml = r#"<a:r xmlns:a="urn:a" xmlns:b="urn:b"><b:s b:t="1"/><a:s/></a:r>"#;
    let opts = byte().preserve(Preserve::PREFIXES).build().unwrap();
    let (_, back) = xml_exi_xml(xml, opts, PrefixPolicy::FromEvents, 5);
    assert_eq!(back, xml);
    let (_, back) = xml_exi_xml(xml, byte().build().unwrap(), PrefixPolicy::Generate, 5);
    assert_eq!(
        back,
        r#"<r xmlns="urn:a"><s xmlns="urn:b" xmlns:ns1="urn:b" ns1:t="1"/><s/></r>"#
    );
}

#[test]
fn fragments_and_header_options() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings).unwrap();
    let opts = byte().fragment(true).build().unwrap();
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts, &grammars).header(Header::WithOptions { cookie: true }),
    )
    .unwrap();
    for i in 0..3 {
        w.start_element("m").unwrap();
        w.text(format!("{i}").as_str()).unwrap();
        w.end_element().unwrap();
    }
    w.end_document().unwrap();
    let exi = w.into_sink();
    assert!(exi.starts_with(b"$EXI"));
    let mut r: Reader = Reader::new(ReaderConfig::new(&grammars)).unwrap();
    r.feed(&exi);
    r.end_input();
    let mut texts = Vec::new();
    loop {
        match r.next().unwrap() {
            Step::Event(e, _) => {
                if let OwnedEvent::Characters(OwnedValue::Str(s)) = e.to_owned() {
                    texts.push(s.to_string());
                }
            }
            Step::End => break,
            Step::NeedInput => unreachable!(),
        }
    }
    assert_eq!(texts, ["0", "1", "2"]);
}

#[test]
fn schema_informed_typed_values() {
    const NS: &str = "urn:t";
    let mut b = GrammarSet::builder();
    let int = b.simple_type(None, Representation::Integer(IntegerRep::Signed));
    let small = b.simple_type(
        None,
        Representation::Integer(IntegerRep::Bounded { min: 10, max: 20 }),
    );
    let dec = b.simple_type(None, Representation::Decimal);
    let boolean = b.simple_type(None, Representation::Boolean { pattern: false });
    let date = b.simple_type(None, Representation::DateTime(exi::DateTimeKind::Date));
    let list = b.simple_type(None, Representation::List(int));
    let color = b.simple_type(
        None,
        Representation::Enumeration {
            values: vec!["red".into(), "green".into()].into(),
            base: int,
        },
    );
    let t_int = b.simple_content_type(None, int);
    let t_dec = b.simple_content_type(None, dec);
    let t_list = b.simple_content_type(None, list);
    let t_date = b.simple_content_type(None, date);
    let e_i = b.element(OwnedName::new(NS, "i"), t_int, false);
    let e_d = b.element(OwnedName::new(NS, "d"), t_dec, true);
    let e_l = b.element(OwnedName::new(NS, "l"), t_list, false);
    let e_t = b.element(OwnedName::new(NS, "t"), t_date, false);
    let root_t = b.complex_type(
        None,
        vec![
            vec![Prod::At {
                name: OwnedName::new("", "color"),
                ty: color,
                next: 1,
            }],
            vec![Prod::At {
                name: OwnedName::new("", "n"),
                ty: small,
                next: 2,
            }],
            vec![Prod::At {
                name: OwnedName::new("", "ok"),
                ty: boolean,
                next: 3,
            }],
            vec![Prod::Se {
                element: e_i,
                next: 4,
            }],
            vec![Prod::Se {
                element: e_d,
                next: 5,
            }],
            vec![Prod::Se {
                element: e_l,
                next: 6,
            }],
            vec![Prod::Se {
                element: e_t,
                next: 7,
            }],
            vec![Prod::EE],
        ],
    );
    let root = b.element(OwnedName::new(NS, "root"), root_t, false);
    b.global(root);
    let set = b.build().unwrap();
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let grammars = set.attach(&strings).unwrap();
    let strict = byte().strict(true).build().unwrap();

    let xml = r#"<root xmlns="urn:t" ok="true" n="15" color="green"><i>-42</i><d>3.50</d><l> 1 2  3 </l><t>2026-11-22</t></root>"#;
    let mut x: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
    x.feed(xml.as_bytes());
    x.end_input();
    let mut w = Writer::new(Vec::new(), WriterConfig::new(strict, &grammars)).unwrap();
    transcode(&mut x, &mut w).unwrap();
    let exi = w.into_sink();

    let mut r: Reader = Reader::new(ReaderConfig::new(&grammars)).unwrap();
    r.feed(&exi);
    r.end_input();
    let mut out = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
    let mut values = Vec::new();
    loop {
        match r.next().unwrap() {
            Step::Event(e, _) => {
                match e.to_owned() {
                    OwnedEvent::Attribute(_, v) | OwnedEvent::Characters(v) => values.push(v),
                    _ => {}
                }
                out.event(&e).unwrap();
            }
            Step::End => break,
            Step::NeedInput => unreachable!(),
        }
    }
    assert_eq!(
        values[0],
        OwnedValue::Enum {
            ordinal: 1,
            text: "green".into()
        }
    );
    assert_eq!(values[1], OwnedValue::Int(15));
    assert_eq!(values[2], OwnedValue::Bool(true));
    assert_eq!(values[3], OwnedValue::Int(-42));
    assert_eq!(
        values[4],
        OwnedValue::Decimal(exi::Decimal::new(false, 3, 5, 1))
    );
    assert_eq!(
        values[5],
        OwnedValue::List(vec![
            OwnedValue::Int(1),
            OwnedValue::Int(2),
            OwnedValue::Int(3)
        ])
    );
    assert_eq!(
        String::from_utf8(out.into_sink()).unwrap(),
        r#"<root xmlns="urn:t" color="green" n="15" ok="true"><i>-42</i><d>3.5</d><l>1 2 3</l><t>2026-11-22</t></root>"#
    );
}

#[test]
fn string_table_hits_across_elements() {
    let xml = "<r><a>same</a><b>same</b><a>same</a><a>other</a></r>";
    let (exi, back) = xml_exi_xml(xml, byte().build().unwrap(), PrefixPolicy::Generate, 1);
    assert_eq!(back, xml);
    // "same" travels once as a literal.
    assert_eq!(exi.windows(4).filter(|w| *w == b"same").count(), 1);
}

#[test]
fn self_contained_elements() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings).unwrap();
    let opts = byte().self_contained(true).build().unwrap();
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), &grammars).header(Header::Bare { cookie: false }),
    )
    .unwrap();
    // <r><a k="v">x</a><s k="v"><a>x</a></s><a>x</a></r> with <s> self-contained.
    w.start_element("r").unwrap();
    w.start_element("a").unwrap();
    w.attribute("k", "v").unwrap();
    w.text("x").unwrap();
    w.end_element().unwrap();
    w.start_element("s").unwrap();
    w.event(&exi::Event::SelfContained).unwrap();
    w.attribute("k", "v").unwrap();
    w.start_element("a").unwrap();
    w.text("x").unwrap();
    w.end_element().unwrap();
    w.end_element().unwrap();
    w.start_element("a").unwrap();
    w.text("x").unwrap();
    w.end_element().unwrap();
    w.end_element().unwrap();
    w.end_document().unwrap();
    let exi = w.into_sink();
    // Inside <s> the tables start again, so "x" and "v" travel as literals a second time.
    assert_eq!(exi.windows(2).filter(|p| *p == [0x03, b'x']).count(), 2);
    assert_eq!(exi.windows(2).filter(|p| *p == [0x03, b'v']).count(), 2);

    let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(opts)).unwrap();
    r.feed(&exi);
    r.end_input();
    let mut events = Vec::new();
    loop {
        match r.next().unwrap() {
            Step::Event(e, meta) => events.push((e.to_owned(), meta.depth)),
            Step::End => break,
            Step::NeedInput => unreachable!(),
        }
    }
    let n = |l: &str| OwnedName::new("", l);
    let s = |t: &str| OwnedValue::Str(t.into());
    use OwnedEvent::*;
    assert_eq!(
        events,
        [
            (StartDocument, 0),
            (StartElement(n("r")), 1),
            (StartElement(n("a")), 2),
            (Attribute(n("k"), s("v")), 2),
            (Characters(s("x")), 2),
            (EndElement(n("a")), 1),
            (StartElement(n("s")), 2),
            (SelfContained, 2),
            (Attribute(n("k"), s("v")), 2),
            (StartElement(n("a")), 3),
            (Characters(s("x")), 3),
            (EndElement(n("a")), 2),
            (EndElement(n("s")), 1),
            (StartElement(n("a")), 2),
            (Characters(s("x")), 2),
            (EndElement(n("a")), 1),
            (EndElement(n("r")), 0),
            (EndDocument, 0),
        ]
    );
}
