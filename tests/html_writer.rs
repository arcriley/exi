// tests/html_writer.rs
//
// HTML output tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! HTML output with only the html feature enabled.
#![cfg(feature = "html")]

/// The default DOCTYPE comes before whatever is written first, an
/// entity reference too.
#[test]
fn doctype_before_an_entity_reference() {
    use exi::html::HtmlWriter;
    use exi::{Event, EventSink};
    let mut w = HtmlWriter::new(Vec::new());
    w.event(&Event::EntityReference("amp")).unwrap();
    w.start_element(("http://www.w3.org/1999/xhtml", "p")).unwrap();
    w.end_element().unwrap();
    w.event(&Event::EndDocument).unwrap();
    assert_eq!(w.into_sink(), b"<!DOCTYPE html>&amp;<p></p>");
}

#[test]
fn writer_options_and_escaping() {
    use exi::EventSink;
    use exi::html::{HtmlWriter, HtmlWriterOptions};
    let options =
        HtmlWriterOptions::new().reverse_coercion(false).doctype(false);
    let mut w = HtmlWriter::with_options(Vec::new(), options);
    let value = "\u{a0}'\"&<>";
    w.start_element(("http://www.w3.org/1999/xhtml", "p")).unwrap();
    w.attribute("U000040click", value).unwrap();
    w.text(value).unwrap();
    w.end_element().unwrap();
    w.end_document().unwrap();
    assert_eq!(w.into_sink(), br#"<p U000040click="&nbsp;'&quot;&amp;&lt;&gt;">&nbsp;'"&amp;&lt;&gt;</p>"#);
}

// A byte-aligned, schema-less stream with options in its header.
// The bare-header-only build explicitly excludes such headers.
#[cfg(any(not(feature = "bare-header-only"), feature = "header-options"))]
#[test]
fn decode_xhtml_void_element_to_html() {
    use exi::grammar::GrammarSet;
    use exi::strings::{GlobalConfig, GlobalStrings};

    // exi encode -c byte over <br xmlns="http://www.w3.org/1999/xhtml"/>
    let stream = b"\xa0\x00\x4a\x00\x1chttp://www.w3.org/1999/xhtml\x03br\x00";
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    assert_eq!(
        exi::html::decode_to_html(stream, &g).unwrap(),
        "<!DOCTYPE html><br>"
    );
}
