// src/html/writer.rs
//
// HTML fragment serialization
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Events to HTML text, by the HTML standard's fragment serialization
//! algorithm.

use std::string::String;
use std::vec::Vec;

use super::{MATHML, SVG, XHTML, XLINK, XML_NS, XMLNS, uncoerce_name};
use crate::coding::Sink;
use crate::grammar::Grammars;
use crate::stream::{EventSink, Status};
use crate::{
    Error, ErrorKind, Event, Name, Reader, ReaderConfig, Result, Value,
};

/// Elements with no end tag, whose children are not written.
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame",
    "hr", "img", "input", "keygen", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose text is written as it is.
const RAW: &[&str] = &[
    "style",
    "script",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "plaintext",
    "noscript",
];

/// How [`HtmlWriter`] writes.
#[derive(Clone, Debug)]
pub struct HtmlWriterOptions {
    reverse_coercion: bool,
    doctype: bool,
}

impl Default for HtmlWriterOptions {
    fn default() -> Self {
        HtmlWriterOptions { reverse_coercion: true, doctype: true }
    }
}

impl HtmlWriterOptions {
    pub fn new() -> Self {
        Self::default()
    }
    /// Turn coerced names (`U000040click`) back into what they were
    /// (`@click`). Default on. A page whose own names look coerced
    /// comes back changed, so turn it off for those.
    pub fn reverse_coercion(mut self, on: bool) -> Self {
        self.reverse_coercion = on;
        self
    }
    /// Write `<!DOCTYPE html>` before the first element when the events
    /// carry no DOCTYPE. Default on.
    pub fn doctype(mut self, on: bool) -> Self {
        self.doctype = on;
        self
    }
}

struct Open {
    void: bool,
    raw: bool,
    qname: String,
}

/// Writes events as HTML text: void elements without end tags, `script`
/// and `style` text unescaped, attributes escaped as HTML requires.
///
/// ```
/// use exi::html::HtmlWriter;
/// use exi::{EventSink, Value};
/// let mut w = HtmlWriter::new(Vec::new());
/// w.start_element(("http://www.w3.org/1999/xhtml", "p"))?;
/// w.attribute("U000040click", "go()")?;
/// w.text("a < b")?;
/// w.start_element(("http://www.w3.org/1999/xhtml", "br"))?;
/// w.end_element()?;
/// w.end_element()?;
/// w.event(&exi::Event::EndDocument)?;
/// assert_eq!(w.into_sink(), br#"<!DOCTYPE html><p @click="go()">a &lt; b<br></p>"#);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct HtmlWriter<S: Sink> {
    sink: S,
    opts: HtmlWriterOptions,
    stack: Vec<Open>,
    /// A start tag is open: attributes may still come.
    in_tag: bool,
    /// Elements open inside a void element's content, which is not
    /// written.
    skip: usize,
    doctype_due: bool,
    /// Something has been written.
    wrote: bool,
    buf: String,
}

impl<S: Sink> HtmlWriter<S> {
    pub fn new(sink: S) -> Self {
        Self::with_options(sink, HtmlWriterOptions::default())
    }

    pub fn with_options(sink: S, opts: HtmlWriterOptions) -> Self {
        let doctype_due = opts.doctype;
        HtmlWriter {
            sink,
            opts,
            stack: Vec::new(),
            in_tag: false,
            skip: 0,
            doctype_due,
            wrote: false,
            buf: String::new(),
        }
    }

    pub fn into_sink(self) -> S {
        self.sink
    }

    fn name<'n>(&self, local: &'n str) -> std::borrow::Cow<'n, str> {
        if self.opts.reverse_coercion {
            uncoerce_name(local)
        } else {
            std::borrow::Cow::Borrowed(local)
        }
    }

    fn put(&mut self, s: &str) -> Result<()> {
        self.wrote = true;
        self.buf.push_str(s);
        if self.buf.len() >= 8192 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if !self.buf.is_empty() {
            self.sink.write(self.buf.as_bytes())?;
            self.buf.clear();
        }
        Ok(())
    }

    fn close_tag(&mut self) -> Result<()> {
        if self.in_tag {
            self.in_tag = false;
            self.put(">")?;
        }
        Ok(())
    }

    fn doctype(&mut self) -> Result<()> {
        if self.doctype_due {
            self.doctype_due = false;
            self.put("<!DOCTYPE html>")?;
        }
        Ok(())
    }

    fn escape(&mut self, s: &str, attribute: bool) -> Result<()> {
        self.wrote = true;
        let mut last = 0;
        for (i, c) in s.char_indices() {
            let rep = match c {
                '&' => "&amp;",
                '\u{a0}' => "&nbsp;",
                '"' if attribute => "&quot;",
                '<' => "&lt;",
                '>' => "&gt;",
                _ => continue,
            };
            self.buf.push_str(&s[last..i]);
            self.buf.push_str(rep);
            last = i + c.len_utf8();
        }
        self.buf.push_str(&s[last..]);
        if self.buf.len() >= 8192 {
            self.flush()?;
        }
        Ok(())
    }

    /// Inside a void element, or something in one: content is not
    /// written.
    fn skipping(&self) -> bool {
        self.skip > 0 || self.stack.last().is_some_and(|o| o.void)
    }

    fn start(&mut self, n: Name<'_>) -> Result<()> {
        if self.skipping() {
            self.skip += 1;
            return Ok(());
        }
        self.close_tag()?;
        self.doctype()?;
        let known = matches!(n.uri(), XHTML | SVG | MATHML);
        let local = self.name(n.local_name()).into_owned();
        let qname = match (known, n.prefix()) {
            (false, Some(p)) if !p.is_empty() => std::format!("{p}:{local}"),
            _ => local.clone(),
        };
        let html = n.uri() == XHTML;
        let void = html && VOID.contains(&local.as_str());
        let raw = html && RAW.contains(&local.as_str());
        self.put("<")?;
        self.put(&qname)?;
        self.in_tag = true;
        self.stack.push(Open { void, raw, qname });
        Ok(())
    }

    fn attribute(&mut self, n: Name<'_>, v: &Value<'_>) -> Result<()> {
        if self.skip > 0 {
            return Ok(());
        }
        if !self.in_tag {
            return Err(Error::new(
                ErrorKind::State,
                "an attribute after content",
            ));
        }
        let local = self.name(n.local_name()).into_owned();
        let name = match n.uri() {
            "" => local,
            XML_NS => std::format!("xml:{local}"),
            XMLNS if local == "xmlns" => local,
            XMLNS => std::format!("xmlns:{local}"),
            XLINK => std::format!("xlink:{local}"),
            _ => match n.prefix() {
                Some(p) if !p.is_empty() => std::format!("{p}:{local}"),
                _ => local,
            },
        };
        self.put(" ")?;
        self.put(&name)?;
        self.put("=\"")?;
        let text = text_of(v)?;
        self.escape(&text, true)?;
        self.put("\"")
    }

    fn end(&mut self) -> Result<()> {
        if self.skip > 0 {
            self.skip -= 1;
            return Ok(());
        }
        self.close_tag()?;
        let open = self.stack.pop().ok_or_else(|| {
            Error::new(ErrorKind::State, "an end with no element open")
        })?;
        if open.void {
            return Ok(());
        }
        self.put("</")?;
        self.put(&open.qname)?;
        self.put(">")
    }
}

/// A value as its text.
fn text_of<'v>(v: &'v Value<'_>) -> Result<std::borrow::Cow<'v, str>> {
    match v {
        Value::Str(s) => Ok(std::borrow::Cow::Borrowed(s)),
        v => crate::engine::lexical_string(v).map(std::borrow::Cow::Owned),
    }
}

impl<S: Sink> EventSink for HtmlWriter<S> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        match ev {
            Event::StartDocument
            | Event::Namespace(_)
            | Event::SelfContained => Ok(()),
            Event::StartElement(n) => self.start(*n),
            Event::Attribute(n, v) => self.attribute(*n, v),
            Event::Characters(v) => {
                if self.skipping() {
                    return Ok(());
                }
                self.close_tag()?;
                self.doctype()?;
                let text = text_of(v)?;
                if self.stack.last().is_some_and(|o| o.raw) {
                    self.put(&text)
                } else {
                    self.escape(&text, false)
                }
            }
            Event::EndElement(_) => self.end(),
            Event::Comment(t) => {
                if self.skipping() {
                    return Ok(());
                }
                self.close_tag()?;
                self.doctype()?;
                self.put("<!--")?;
                self.put(t)?;
                self.put("-->")
            }
            Event::ProcessingInstruction { target, data } => {
                if self.skipping() {
                    return Ok(());
                }
                self.close_tag()?;
                self.doctype()?;
                self.put("<?")?;
                self.put(target)?;
                if !data.is_empty() {
                    self.put(" ")?;
                    self.put(data)?;
                }
                self.put(">")
            }
            Event::Doctype(d) => {
                // A DOCTYPE comes first: after other output (the
                // default one, or a comment before it in the page) it
                // is left out.
                if self.wrote {
                    return Ok(());
                }
                self.doctype_due = false;
                self.put("<!DOCTYPE ")?;
                self.put(if d.name.is_empty() { "html" } else { d.name })?;
                self.put(">")
            }
            Event::EntityReference(name) => {
                if self.skipping() {
                    return Ok(());
                }
                self.close_tag()?;
                self.put("&")?;
                self.put(name)?;
                self.put(";")
            }
            Event::EndDocument => {
                self.close_tag()?;
                self.flush()
            }
            Event::StartTag(t) => {
                self.event(&Event::StartElement(t.name))?;
                for (n, v) in t.attrs {
                    self.event(&Event::Attribute(*n, *v))?;
                }
                Ok(())
            }
        }
    }
}

/// An EXI stream as HTML text (`<!DOCTYPE html>` first unless it
/// carries a DOCTYPE).
///
/// # Errors
///
/// The reader's errors; `ErrorKind::Truncated` for a stream that ends
/// too soon.
pub fn decode_to_html(exi: &[u8], grammars: &Grammars<'_>) -> Result<String> {
    decode_with(exi, ReaderConfig::new(grammars))
}

/// As [`decode_to_html`], with a reader configuration (options for a
/// bare header).
pub(crate) fn decode_with(
    exi: &[u8],
    cfg: ReaderConfig<'_>,
) -> Result<String> {
    let mut r: Reader = Reader::new(cfg)?;
    let mut w = HtmlWriter::new(Vec::new());
    let mut rest = exi;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        match crate::transcode(&mut r, &mut w)? {
            Status::End => break,
            Status::NeedInput if rest.is_empty() => {
                return Err(Error::new(
                    ErrorKind::Truncated,
                    "the stream ends too soon",
                ));
            }
            Status::NeedInput => {}
        }
    }
    String::from_utf8(w.into_sink()).map_err(|_| {
        Error::new(
            ErrorKind::Internal,
            "the HTML writer wrote bytes that are not UTF-8",
        )
    })
}
