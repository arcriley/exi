// src/html/mod.rs
//
// HTML pages as EXI (features `html`, `html5ever`, `xhtml5`)
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! An HTML page becomes events without XHTML text in between: html5ever
//! parses it into a tree, as browsers do, and the tree's nodes become
//! events. Those events go to any layer 1 sink: an EXI writer, the XML
//! writer (XHTML), or an application. [`HtmlWriter`] turns events back
//! into HTML text.
//!
//! ```
//! # #[cfg(feature = "html5ever")] {
//! use exi::html::{decode_to_html, encode_html, HtmlOptions};
//! use exi::grammar::GrammarSet;
//! use exi::strings::{GlobalConfig, GlobalStrings};
//! use exi::{Coding, Options, Writer, WriterConfig};
//!
//! let page = r#"<!doctype html><title>Hi</title><p class=intro>Hello <b>world</b><img src=a.png alt="">"#;
//! let strings = GlobalStrings::new(GlobalConfig::new())?;
//! let set = GrammarSet::schemaless();
//! let g = set.attach(&strings)?;
//! let opts = Options::builder().coding(Coding::ByteAligned).build()?;
//! let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &g))?;
//! encode_html(page, &mut w, &HtmlOptions::new())?;
//! let exi = w.into_sink();
//!
//! assert_eq!(decode_to_html(&exi, &g)?,
//!     r#"<!DOCTYPE html><html><head><title>Hi</title></head><body><p class="intro">Hello <b>world</b><img src="a.png" alt=""></p></body></html>"#);
//! # }
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # What changes on the way in
//!
//! An HTML tree can hold what the XML infoset cannot. [`HtmlOptions`]
//! decides:
//!
//! | HTML | Becomes |
//! |---|---|
//! | A name XML does not allow: `@click`, `:href`, `x"y` | Each bad character as `U` and six hex digits, the HTML standard's coercion: `U000040click` ([`Coercion`]). [`HtmlWriter`] turns it back |
//! | Form feed in text or values | A space |
//! | Other characters XML does not allow | U+FFFD |
//! | `--` in a comment, or a final `-` | `- -`, and `- ` |
//! | `xmlns` attributes | Dropped: names carry their namespaces |
//! | `<!DOCTYPE html>` | Kept with [`HtmlOptions::keep_doctype`] (and `Preserve.dtd` in the stream); the HTML writer writes one anyway |
//! | `<template>` contents | The element's children (`encode_html`; trees given to [`encode_dom`] decide themselves) |
//!
//! SVG and MathML inside a page are in their namespaces already,
//! `xlink:href` and the other foreign attributes too.

#[cfg(feature = "html5ever")]
mod parse;
#[cfg(feature = "xhtml5")]
pub mod profile;
mod writer;

#[cfg(feature = "html5ever")]
pub use parse::{encode_html, html_to_xhtml, html_to_xhtml_with};
pub use writer::{HtmlWriter, HtmlWriterOptions, decode_to_html};

use std::borrow::Cow;
use std::io;
use std::string::{String, ToString};

use markup5ever::QualName;
use markup5ever::serialize::{AttrRef, Serialize, Serializer, TraversalScope};

use crate::xml::chars::{is_char, is_name_char, is_name_start};
use crate::{
    Doctype, Error, ErrorKind, Event, EventSink, Name, Result, Value,
};

pub(crate) const XHTML: &str = "http://www.w3.org/1999/xhtml";
pub(crate) const SVG: &str = "http://www.w3.org/2000/svg";
pub(crate) const MATHML: &str = "http://www.w3.org/1998/Math/MathML";
pub(crate) const XLINK: &str = "http://www.w3.org/1999/xlink";
pub(crate) const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
pub(crate) const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

/// What happens to a name XML does not allow.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Coercion {
    /// Each bad character becomes `U` and six uppercase hex digits (the
    /// HTML standard's coercion to the infoset): `@click` →
    /// `U000040click`.
    #[default]
    Rewrite,
    /// The attribute is left out (an element keeps the rewritten name).
    Drop,
    /// `ErrorKind::Xml`.
    Reject,
}

/// How an HTML tree becomes events. See the [module documentation](self).
#[derive(Clone, Debug, Default)]
pub struct HtmlOptions {
    coercion: Coercion,
    keep_doctype: bool,
    keep_comments: bool,
}

impl HtmlOptions {
    /// Rewrite bad names, drop the DOCTYPE, keep comments (the EXI
    /// writer drops them unless `Preserve.comments`).
    pub fn new() -> Self {
        HtmlOptions {
            coercion: Coercion::Rewrite,
            keep_doctype: false,
            keep_comments: true,
        }
    }
    pub fn coercion(mut self, c: Coercion) -> Self {
        self.coercion = c;
        self
    }
    /// Pass `<!DOCTYPE …>` on as a Doctype event. Default off: the HTML
    /// writer writes `<!DOCTYPE html>` anyway.
    pub fn keep_doctype(mut self, on: bool) -> Self {
        self.keep_doctype = on;
        self
    }
    /// Pass comments on. Default on.
    pub fn keep_comments(mut self, on: bool) -> Self {
        self.keep_comments = on;
        self
    }
}

/// `name` as an XML NCName: each character that cannot be there becomes
/// `U` and six uppercase hex digits. `None` when nothing changes.
///
/// ```
/// use exi::html::coerce_name;
/// assert_eq!(coerce_name("@click").as_deref(), Some("U000040click"));
/// assert_eq!(coerce_name(":href").as_deref(), Some("U00003Ahref"));
/// assert_eq!(coerce_name("data-x"), None);
/// assert_eq!(coerce_name("1st").as_deref(), Some("U000031st"));
/// ```
pub fn coerce_name(name: &str) -> Option<String> {
    let ok = |i: usize, c: char| {
        if i == 0 {
            is_name_start(c) && c != ':'
        } else {
            is_name_char(c) && c != ':'
        }
    };
    if !name.is_empty() && name.chars().enumerate().all(|(i, c)| ok(i, c)) {
        return None;
    }
    let mut out = String::with_capacity(name.len() + 8);
    for (i, c) in name.chars().enumerate() {
        if ok(i, c) {
            out.push(c);
        } else {
            use core::fmt::Write;
            let _ = write!(out, "U{:06X}", c as u32);
        }
    }
    if out.is_empty() {
        out.push_str("U000000");
    }
    Some(out)
}

/// The name a coerced name came from: `U` and six hex digits back to
/// their character.
///
/// ```
/// use exi::html::uncoerce_name;
/// assert_eq!(uncoerce_name("U000040click"), "@click");
/// assert_eq!(uncoerce_name("data-x"), "data-x");
/// ```
pub fn uncoerce_name(name: &str) -> Cow<'_, str> {
    if !name.contains('U') {
        return Cow::Borrowed(name);
    }
    let b = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    let mut changed = false;
    while i < b.len() {
        if b[i] == b'U'
            && i + 7 <= b.len()
            && b[i + 1..i + 7]
                .iter()
                .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
        {
            let hex = &name[i + 1..i + 7];
            if let Some(c) =
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            {
                out.push(c);
                i += 7;
                changed = true;
                continue;
            }
        }
        let c = name[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    if changed { Cow::Owned(out) } else { Cow::Borrowed(name) }
}

/// Text with form feeds as spaces and characters XML does not allow as
/// U+FFFD.
pub(crate) fn xml_text(s: &str) -> Cow<'_, str> {
    if s.chars().all(|c| c != '\u{c}' && is_char(c)) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| {
                if c == '\u{c}' {
                    ' '
                } else if is_char(c) {
                    c
                } else {
                    '\u{fffd}'
                }
            })
            .collect(),
    )
}

/// Comment text XML allows: no `--`, no final `-`.
pub(crate) fn xml_comment(s: &str) -> Cow<'_, str> {
    let t = xml_text(s);
    if !t.contains("--") && !t.ends_with('-') {
        return t;
    }
    let mut out = String::with_capacity(t.len() + 4);
    for c in t.chars() {
        if c == '-' && out.ends_with('-') {
            out.push(' ');
        }
        out.push(c);
    }
    if out.ends_with('-') {
        out.push(' ');
    }
    Cow::Owned(out)
}

/// Turns an HTML tree into events: markup5ever's [`Serializer`], over
/// any [`EventSink`]. For trees that implement markup5ever's
/// [`Serialize`] (RcDom, scraper, kuchikiki); [`encode_dom`] drives it.
pub struct HtmlSerializer<'s, S: EventSink> {
    sink: &'s mut S,
    opts: HtmlOptions,
    started: bool,
    /// The first error, which `io::Error` cannot carry.
    error: Option<Error>,
}

impl<'s, S: EventSink> HtmlSerializer<'s, S> {
    pub fn new(sink: &'s mut S, opts: HtmlOptions) -> Self {
        HtmlSerializer { sink, opts, started: false, error: None }
    }

    /// Ends the document: EndDocument, and the first error, if any.
    pub fn finish(mut self) -> Result<()> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        self.start()?;
        self.sink.event(&Event::EndDocument)
    }

    fn start(&mut self) -> Result<()> {
        if !self.started {
            self.started = true;
            self.sink.event(&Event::StartDocument)?;
        }
        Ok(())
    }

    fn emit(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<()>,
    ) -> io::Result<()> {
        if self.error.is_some() {
            return Err(io::Error::other("an earlier event failed"));
        }
        let r = self.start().and_then(|()| f(self));
        r.map_err(|e| {
            let msg = io::Error::other(std::format!("{e}"));
            self.error = Some(e);
            msg
        })
    }

    fn local<'n>(&self, local: &'n str) -> Result<Cow<'n, str>> {
        match coerce_name(local) {
            None => Ok(Cow::Borrowed(local)),
            Some(_) if self.opts.coercion == Coercion::Reject => {
                Err(Error::new(ErrorKind::Xml, "a name XML does not allow"))
            }
            Some(n) => Ok(Cow::Owned(n)),
        }
    }
}

/// The event name of an element or attribute name from markup5ever.
fn event_name<'n>(q: &'n QualName, local: &'n str) -> Name<'n> {
    let n = Name::new(&q.ns, local);
    match &q.prefix {
        Some(p) if !q.ns.is_empty() => n.with_prefix(p),
        _ => n,
    }
}

impl<S: EventSink> Serializer for HtmlSerializer<'_, S> {
    fn start_elem<'a, AttrIter>(
        &mut self,
        name: QualName,
        attrs: AttrIter,
    ) -> io::Result<()>
    where
        AttrIter: Iterator<Item = AttrRef<'a>>,
    {
        self.emit(|s| {
            let local = s.local(&name.local)?;
            s.sink.event(&Event::StartElement(event_name(&name, &local)))?;
            let mut seen: std::vec::Vec<(String, String)> =
                std::vec::Vec::new();
            for (q, v) in attrs {
                if &*q.ns == XMLNS
                    || (q.ns.is_empty()
                        && (&*q.local == "xmlns"
                            || q.local.starts_with("xmlns:")))
                {
                    continue;
                }
                let bad = coerce_name(&q.local);
                let local: Cow<'_, str> = match (bad, s.opts.coercion) {
                    (None, _) => Cow::Borrowed(&*q.local),
                    (Some(_), Coercion::Drop) => continue,
                    (Some(_), Coercion::Reject) => {
                        return Err(Error::new(
                            ErrorKind::Xml,
                            "an attribute name XML does not allow",
                        ));
                    }
                    (Some(n), Coercion::Rewrite) => Cow::Owned(n),
                };
                // After rewriting two names can meet; the first stays,
                // as html5ever keeps the first of duplicate attributes.
                let key = (q.ns.to_string(), local.to_string());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                let value = xml_text(v);
                s.sink.event(&Event::Attribute(
                    event_name(q, &local),
                    Value::Str(&value),
                ))?;
            }
            Ok(())
        })
    }

    fn end_elem(&mut self, name: QualName) -> io::Result<()> {
        self.emit(|s| {
            let local = s.local(&name.local)?;
            s.sink.event(&Event::EndElement(event_name(&name, &local)))
        })
    }

    fn write_text(&mut self, text: &str) -> io::Result<()> {
        self.emit(|s| {
            s.sink.event(&Event::Characters(Value::Str(&xml_text(text))))
        })
    }

    fn write_comment(&mut self, text: &str) -> io::Result<()> {
        if !self.opts.keep_comments {
            return Ok(());
        }
        self.emit(|s| s.sink.event(&Event::Comment(&xml_comment(text))))
    }

    fn write_doctype(&mut self, name: &str) -> io::Result<()> {
        if !self.opts.keep_doctype {
            return Ok(());
        }
        self.emit(|s| {
            s.sink.event(&Event::Doctype(Doctype {
                name,
                public_id: "",
                system_id: "",
                text: "",
            }))
        })
    }

    fn write_processing_instruction(
        &mut self,
        target: &str,
        data: &str,
    ) -> io::Result<()> {
        self.emit(|s| {
            let target = s.local(target)?;
            s.sink.event(&Event::ProcessingInstruction {
                target: &target,
                data: &xml_text(data),
            })
        })
    }
}

/// Writes a markup5ever tree's events to `sink`: StartDocument, the
/// nodes `scope` selects (`ChildrenOnly(None)` for a document),
/// EndDocument.
pub fn encode_dom<T: Serialize, S: EventSink>(
    tree: &T,
    scope: TraversalScope,
    sink: &mut S,
    opts: &HtmlOptions,
) -> Result<()> {
    let mut ser = HtmlSerializer::new(sink, opts.clone());
    let r = tree.serialize(&mut ser, scope);
    match (r, ser.error.take()) {
        (_, Some(e)) => Err(e),
        (Err(_), None) => {
            Err(Error::new(ErrorKind::Io, "the tree's serializer failed"))
        }
        (Ok(()), None) => ser.finish(),
    }
}
