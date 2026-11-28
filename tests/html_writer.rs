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

include!("support/cases.rs");

fn void_content(name: &str, expected: &str) {
    use exi::html::HtmlWriter;
    use exi::{Event, EventSink, Value};
    let mut writer = HtmlWriter::new(Vec::new());
    let html = "http://www.w3.org/1999/xhtml";
    writer.start_element((html, name)).unwrap();
    writer.attribute("data-v", Value::Int(7)).unwrap();
    writer.text("ignored").unwrap();
    writer.event(&Event::Comment("ignored")).unwrap();
    writer
        .event(&Event::ProcessingInstruction { target: "p", data: "ignored" })
        .unwrap();
    writer.event(&Event::EntityReference("amp")).unwrap();
    writer.start_element((html, "span")).unwrap();
    writer.attribute("id", "ignored").unwrap();
    writer.end_element().unwrap();
    writer.end_element().unwrap();
    writer.start_element((html, "p")).unwrap();
    writer.text("after").unwrap();
    writer.end_element().unwrap();
    writer.end_document().unwrap();
    assert_eq!(String::from_utf8(writer.into_sink()).unwrap(), expected);
}
cases!(void_content;
    line_break("br", "<!DOCTYPE html><br data-v=\"7\"><p>after</p>"),
    image("img", "<!DOCTYPE html><img data-v=\"7\"><p>after</p>"),
    metadata("meta", "<!DOCTYPE html><meta data-v=\"7\"><p>after</p>"),
    input("input", "<!DOCTYPE html><input data-v=\"7\"><p>after</p>"),
    rule("hr", "<!DOCTYPE html><hr data-v=\"7\"><p>after</p>"),
    link("link", "<!DOCTYPE html><link data-v=\"7\"><p>after</p>"),
);

fn raw_text(name: &str, source: &str, expected: &str) {
    use exi::EventSink;
    use exi::html::HtmlWriter;
    let mut writer = HtmlWriter::new(Vec::new());
    writer.start_element(("http://www.w3.org/1999/xhtml", name)).unwrap();
    writer.text(source).unwrap();
    writer.end_element().unwrap();
    writer.end_document().unwrap();
    assert_eq!(String::from_utf8(writer.into_sink()).unwrap(), expected);
}
cases!(raw_text;
    script("script", "x < 2 && y", "<!DOCTYPE html><script>x < 2 && y</script>"),
    style("style", "x::after { content: '<&'; }", "<!DOCTYPE html><style>x::after { content: '<&'; }</style>"),
    title("title", "x < 2 && y", "<!DOCTYPE html><title>x &lt; 2 &amp;&amp; y</title>"),
);

#[test]
fn foreign_element_and_attributes_keep_their_prefixes() -> exi::Result<()> {
    use exi::html::HtmlWriter;
    use exi::{EventSink, Name};
    let mut w = HtmlWriter::new(Vec::new());
    w.start_element(Name::new("urn:foreign", "Thing").with_prefix("p"))?;
    w.attribute(("http://www.w3.org/2000/xmlns/", "p"), "urn:foreign")?;
    w.attribute(("http://www.w3.org/XML/1998/namespace", "lang"), "en")?;
    w.attribute(Name::new("urn:foreign", "Value").with_prefix("p"), "x")?;
    w.end_element()?;
    w.end_document()?;
    assert_eq!(
        w.into_sink(),
        b"<!DOCTYPE html><p:Thing xmlns:p=\"urn:foreign\" xml:lang=\"en\" p:Value=\"x\"></p:Thing>"
    );
    Ok(())
}

#[test]
fn comment_before_doctype_and_pi_after_root() -> exi::Result<()> {
    use exi::html::HtmlWriter;
    use exi::{Doctype, Event, EventSink};
    let mut w = HtmlWriter::new(Vec::new());
    w.event(&Event::Comment("before"))?;
    w.event(&Event::Doctype(Doctype {
        name: "ignored",
        public_id: "",
        system_id: "",
        text: "",
    }))?;
    w.start_element(("http://www.w3.org/1999/xhtml", "main"))?;
    w.end_element()?;
    w.event(&Event::ProcessingInstruction { target: "done", data: "" })?;
    w.end_document()?;
    assert_eq!(
        w.into_sink(),
        b"<!DOCTYPE html><!--before--><main></main><?done>"
    );
    Ok(())
}

#[test]
fn writer_reports_state_and_sink_errors() -> exi::Result<()> {
    use exi::coding::SliceSink;
    use exi::html::HtmlWriter;
    use exi::{ErrorKind, EventSink};
    let mut w = HtmlWriter::new(Vec::new());
    assert_eq!(w.end_element().unwrap_err().kind(), ErrorKind::State);
    w.start_element(("http://www.w3.org/1999/xhtml", "main"))?;
    w.text("content")?;
    assert_eq!(w.attribute("late", "x").unwrap_err().kind(), ErrorKind::State);
    let mut tiny = [0; 1];
    let mut w = HtmlWriter::new(SliceSink::new(&mut tiny));
    w.start_element("x")?;
    w.end_element()?;
    assert_eq!(
        w.end_document().unwrap_err().kind(),
        ErrorKind::BufferFull
    );
    Ok(())
}
