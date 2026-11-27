// tests/writer_rules.rs
//
// Writer rules under schema-informed grammars
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The numbered rules of `src/writer.rs` that need a schema, against
//! `fixtures/input/complex.xsd`.

#![cfg(feature = "xsd")]

use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xsd::XsdCompiler;
use exi::{
    Coding, DeviationKind, ErrorKind, EventSink, Options, Writer, WriterConfig,
};

const C: &str = "urn:c";

fn complex() -> GrammarSet {
    XsdCompiler::new()
        .document(
            "complex.xsd",
            include_bytes!("../fixtures/input/complex.xsd"),
        )
        .document("other.xsd", include_bytes!("../fixtures/input/other.xsd"))
        .compile("complex.xsd")
        .unwrap()
}

fn options(strict: bool) -> Options {
    Options::builder()
        .coding(Coding::ByteAligned)
        .strict(strict)
        .build()
        .unwrap()
}

/// Rule 7: `shape` has type `Base`, which has a sub-type, so its
/// attributes wait for the start tag to be written. An undeclared one
/// is refused there, after SE: the writer is poisoned.
#[test]
fn a_late_attribute_refusal_poisons() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = complex();
    let g = set.attach(&strings)?;
    let mut w = Writer::new(Vec::new(), WriterConfig::new(options(true), &g))?;
    w.start_element((C, "shape"))?;
    w.attribute("id", "a")?;
    w.attribute("kind", "mid")?; // declared in Derived only
    let e = w.start_element((C, "name")).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unencodable);
    let e = w.start_element((C, "name")).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::State);
    Ok(())
}

/// Rule 7: an attribute checked as it comes is refused without harm.
#[test]
fn an_early_attribute_refusal_leaves_the_writer_usable() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = complex();
    let g = set.attach(&strings)?;
    let mut w = Writer::new(Vec::new(), WriterConfig::new(options(true), &g))?;
    w.start_element((C, "doc"))?;
    let e = w.attribute("undeclared", "x").unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unencodable);
    w.start_element((C, "note"))?;
    w.end_element()?;
    Ok(())
}

/// Rule 9: `<note/>` (`xs:string`) gets an empty value; `<p/>` (an
/// integer) cannot, and is refused without harm.
#[test]
fn empty_values() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = complex();
    let g = set.attach(&strings)?;
    let mut w = Writer::new(Vec::new(), WriterConfig::new(options(true), &g))?;
    w.start_element((C, "doc"))?;
    w.start_element((C, "note"))?;
    w.end_element()?; // <note/>: the empty string
    for _ in 0..2 {
        w.start_element((C, "m"))?;
        w.text("1.5")?;
        w.end_element()?;
    }
    w.start_element((C, "any"))?;
    w.end_element()?;
    w.start_element((C, "all"))?;
    w.start_element((C, "x"))?;
    w.text("0A")?;
    w.end_element()?;
    w.start_element((C, "z"))?;
    w.text("7")?;
    w.end_element()?;
    w.end_element()?;
    w.start_element((C, "nums"))?;
    w.start_element((C, "p"))?;
    let e = w.end_element().unwrap_err(); // <p/>
    assert_eq!(e.kind(), ErrorKind::Unencodable);
    w.text("50")?;
    w.end_element()?;
    Ok(())
}

/// Not strict: a value its type cannot hold, and an element that ends
/// early, are counted; `reset` clears the counts.
#[test]
fn untyped_values_and_early_ends_are_counted() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = complex();
    let g = set.attach(&strings)?;
    let cfg = WriterConfig::new(options(false), &g).count_deviations(true);
    let mut w = Writer::new(Vec::new(), cfg)?;
    w.start_element((C, "doc"))?;
    w.attribute("version", "one")?; // xs:decimal
    w.start_element((C, "note"))?;
    w.text("x")?;
    w.end_element()?;
    w.end_element()?; // before m, any, all, ...
    w.end_document()?;
    let d = w.deviations().unwrap();
    let kinds: Vec<_> = d.iter().map(|(k, e, n, _)| (k, e, n)).collect();
    let doc = "{urn:c}doc";
    assert!(kinds.contains(&(DeviationKind::UntypedValue, doc, "{}version")));
    assert!(kinds.contains(&(DeviationKind::EarlyEnd, doc, "")));
    w.reset(Vec::new());
    assert_eq!(w.deviations().unwrap().total(), 0);
    Ok(())
}
