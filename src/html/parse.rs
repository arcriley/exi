// src/html/parse.rs
//
// HTML parsing with html5ever
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! HTML text through html5ever (feature `html5ever`).

use std::string::String;
use std::vec::Vec;

use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, parse_document};
use markup5ever::serialize::Serializer;
use markup5ever::{Attribute, QualName};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use super::{HtmlOptions, HtmlSerializer};
use crate::{Error, ErrorKind, EventSink, Result};

/// Parses `html` as a browser does and writes its events to `sink`:
/// StartDocument, the page, EndDocument. `<template>` contents are
/// written as the element's children.
///
/// ```
/// use exi::html::{encode_html, HtmlOptions};
/// use exi::OwnedEvent;
/// let mut events: Vec<OwnedEvent> = Vec::new();
/// encode_html("<p @click=go>x", &mut events, &HtmlOptions::new())?;
/// // html, head, body, p: the parser adds what the page leaves out.
/// let names: Vec<String> = events.iter().filter_map(|e| match e {
///     OwnedEvent::StartElement(n) => Some(n.as_name().local_name().to_string()),
///     OwnedEvent::Attribute(n, _) => Some(format!("@{}", n.as_name().local_name())),
///     _ => None,
/// }).collect();
/// assert_eq!(names, ["html", "head", "body", "p", "@U000040click"]);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn encode_html<S: EventSink>(
    html: &str,
    sink: &mut S,
    opts: &HtmlOptions,
) -> Result<()> {
    parse_into(html, sink, opts, start_tag, |ser, n| ser.end_elem(n.clone()))
}

fn start_tag<S: EventSink>(
    ser: &mut HtmlSerializer<'_, S>,
    name: &QualName,
    attrs: &mut [Attribute],
) -> std::io::Result<()> {
    ser.start_elem(name.clone(), attrs.iter().map(|a| (&a.name, &a.value[..])))
}

// The profile writer can consume a parsed start tag's attributes while
// the DOM still owns them. Its writer options are fixed by the version.
#[cfg(feature = "xhtml5")]
pub(super) fn encode_profile<S: crate::coding::Sink>(
    html: &str,
    writer: &mut crate::Writer<'_, S>,
) -> Result<()> {
    parse_into(
        html,
        writer,
        &HtmlOptions::new().keep_comments(false),
        |ser, name, attrs| {
            // With unqualified NCNames the parser has already removed
            // duplicates. Rewriting and namespace declarations use the
            // ordinary serializer, which handles collisions after
            // coercion.
            if attrs.iter().any(|a| {
                !a.name.ns.is_empty()
                    || a.name.prefix.is_some()
                    || &*a.name.local == "xmlns"
                    || !super::is_ncname(&a.name.local)
            }) {
                return start_tag(ser, name, attrs);
            }
            ser.emit(|ser| {
                let local = ser.local(&name.local)?;
                attrs.sort_unstable_by(|a, b| {
                    a.name.local[..].cmp(&b.name.local)
                });
                ser.sink.parsed_tag(
                    super::event_name(name, &local),
                    attrs
                        .iter()
                        .map(|a| (&*a.name.local, super::xml_text(&a.value))),
                )
            })
        },
        |ser, _| ser.emit(|ser| ser.sink.end_element()),
    )
}

fn parse_into<S: EventSink>(
    html: &str,
    sink: &mut S,
    opts: &HtmlOptions,
    start: impl Fn(
        &mut HtmlSerializer<'_, S>,
        &QualName,
        &mut [Attribute],
    ) -> std::io::Result<()>,
    end: impl Fn(&mut HtmlSerializer<'_, S>, &QualName) -> std::io::Result<()>,
) -> Result<()> {
    let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);
    let mut ser = HtmlSerializer::new(sink, opts.clone());
    for child in dom.document.children.borrow().iter() {
        walk(child, &mut ser, &start, &end)?;
    }
    ser.finish()
}

/// One node and what is inside it, through the serializer's events.
fn walk<S: EventSink>(
    node: &Handle,
    ser: &mut HtmlSerializer<'_, S>,
    start: &impl Fn(
        &mut HtmlSerializer<'_, S>,
        &QualName,
        &mut [Attribute],
    ) -> std::io::Result<()>,
    end: &impl Fn(&mut HtmlSerializer<'_, S>, &QualName) -> std::io::Result<()>,
) -> Result<()> {
    let r = match &node.data {
        NodeData::Element { name, attrs, template_contents, .. } => {
            start(ser, name, &mut attrs.borrow_mut()).map_err(|_| err(ser))?;
            for c in node.children.borrow().iter() {
                walk(c, ser, start, end)?;
            }
            if let Some(t) = template_contents.borrow().as_ref() {
                for c in t.children.borrow().iter() {
                    walk(c, ser, start, end)?;
                }
            }
            end(ser, name)
        }
        NodeData::Text { contents } => ser.write_text(&contents.borrow()),
        NodeData::Comment { contents } => ser.write_comment(contents),
        NodeData::Doctype { name, .. } => ser.write_doctype(name),
        NodeData::ProcessingInstruction { target, contents } => {
            ser.write_processing_instruction(target, contents)
        }
        NodeData::Document => Ok(()),
    };
    r.map_err(|_| err(ser))
}

fn err<S: EventSink>(ser: &mut HtmlSerializer<'_, S>) -> Error {
    ser.error.take().unwrap_or_else(|| {
        Error::new(
            ErrorKind::Internal,
            "the serializer failed without an error",
        )
    })
}

/// The page as XHTML: XML text in the XHTML namespace, SVG and MathML
/// in theirs.
///
/// ```
/// use exi::html::html_to_xhtml;
/// assert_eq!(html_to_xhtml("<p>a<br>b")?,
///     r#"<html xmlns="http://www.w3.org/1999/xhtml"><head/><body><p>a<br/>b</p></body></html>"#);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn html_to_xhtml(html: &str) -> Result<String> {
    html_to_xhtml_with(html, &HtmlOptions::new())
}

/// As [`html_to_xhtml`], with options.
///
/// ```
/// use exi::html::{HtmlOptions, html_to_xhtml_with};
/// let xhtml = html_to_xhtml_with("<!--note--><p>text", &HtmlOptions::new().keep_comments(false))?;
/// assert!(xhtml.contains("<p>text</p>"));
/// assert!(!xhtml.contains("<!--note-->"));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn html_to_xhtml_with(html: &str, opts: &HtmlOptions) -> Result<String> {
    use crate::xml::{XmlWriter, XmlWriterConfig};
    let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
    encode_html(html, &mut w, opts)?;
    String::from_utf8(w.into_sink()).map_err(|_| {
        Error::new(
            ErrorKind::Internal,
            "the XML writer wrote bytes that are not UTF-8",
        )
    })
}
