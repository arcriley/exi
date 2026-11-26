// src/xml/writer.rs
//
// XML 1.0 text writer
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # What it writes
//!
//! UTF-8, without a byte order mark, with LF line ends, and no
//! whitespace of its own.
//!
//! | Item | Written as |
//! |---|---|
//! | Declaration | `<?xml version="1.0" encoding="UTF-8"?>` when configured |
//! | Start tag | `<name`, namespace declarations, attributes, then `>`; an element with no content is `<name/>` |
//! | Text | `&` as `&amp;`, `<` as `&lt;`, `>` as `&gt;`, CR as `&#13;`; typed values in their canonical lexical form ([`Value::write_lexical`](crate::Value::write_lexical)) |
//! | Attribute value | in `"`; `&`, `<`, `"` escaped as `&amp;`, `&lt;`, `&quot;`; tab, LF and CR as `&#9;`, `&#10;`, `&#13;`, so the value survives attribute normalization |
//! | Comment | `<!--text-->` |
//! | PI | `<?target data?>`, or `<?target?>` when the data is empty |
//! | DOCTYPE | `<!DOCTYPE name PUBLIC "p" "s" [subset]>`, with each part present only when not empty (`SYSTEM "s"` when there is no public id) |
//! | Entity reference | `&name;` |
//!
//! Errors (`ErrorKind::Xml`): text, attribute values, comments or PI
//! data holding a character outside XML `Char`; a comment containing
//! `--` or ending with `-`; PI data containing `?>`; a PI target `xml`
//! in any case; names that are not QNames.
//!
//! # Prefixes
//!
//! | [`PrefixPolicy`] | Behaviour |
//! |---|---|
//! | `Generate` (default) | NS events are ignored. An element in a namespace is written with no prefix, declaring `xmlns="uri"` where the default namespace changes; an element in no namespace inside a default namespace gets `xmlns=""`. An attribute in a namespace uses a prefix already bound to its URI, else a new one declared on its element: the name's own prefix when it is free there, else `ns1`, `ns2`, … (the lowest number not in scope). The XML namespace always uses `xml` and is never declared |
//! | `FromEvents` | Prefixes and declarations as the events give them (streams with `Preserve.prefixes`). A name whose prefix is not declared in scope is written as `Generate` would |

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::coding::Sink;
use crate::engine::lexical_string;
use crate::event::Event;
use crate::name::{Name, OwnedName};
use crate::stream::EventSink;
use crate::xml::chars::{is_char, is_ncname};
use crate::{Error, ErrorKind, Result};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

fn xml_err(detail: &'static str) -> Error {
    Error::new(ErrorKind::Xml, detail)
}

fn state(detail: &'static str) -> Error {
    Error::new(ErrorKind::State, detail)
}

fn check_chars(s: &str) -> Result<()> {
    if s.chars().all(is_char) {
        Ok(())
    } else {
        Err(xml_err("a character outside XML Char"))
    }
}

fn check_qname(n: Name<'_>) -> Result<()> {
    let prefix_ok = n.prefix().is_none_or(|p| p.is_empty() || is_ncname(p));
    if is_ncname(n.local_name()) && prefix_ok {
        Ok(())
    } else {
        Err(xml_err("a name that is not a QName"))
    }
}

/// How prefixes are chosen (module documentation).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PrefixPolicy {
    #[default]
    Generate,
    FromEvents,
}

/// Configuration for an [`XmlWriter`].
#[derive(Clone, Debug, Default)]
pub struct XmlWriterConfig {
    declaration: bool,
    prefixes: PrefixPolicy,
}

impl XmlWriterConfig {
    ///
    /// ```
    /// use exi::{EventSink, xml::{XmlWriter, XmlWriterConfig}};
    /// let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
    /// w.start_element("a")?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// assert_eq!(w.into_sink(), b"<a/>");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new() -> Self {
        XmlWriterConfig {
            declaration: false,
            prefixes: PrefixPolicy::Generate,
        }
    }
    /// Write the XML declaration first (default off).
    ///
    /// ```
    /// use exi::{EventSink, xml::{XmlWriter, XmlWriterConfig}};
    /// let cfg = XmlWriterConfig::new().declaration(true);
    /// let mut w = XmlWriter::new(Vec::new(), cfg);
    /// w.start_element("a")?;
    /// w.end_element()?;
    /// assert!(w.into_sink().starts_with(b"<?xml "));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn declaration(self, on: bool) -> Self {
        XmlWriterConfig { declaration: on, ..self }
    }
    ///
    /// ```
    /// use exi::{EventSink, xml::{XmlWriter, XmlWriterConfig}};
    /// use exi::xml::PrefixPolicy;
    /// let cfg = XmlWriterConfig::new().prefixes(PrefixPolicy::FromEvents);
    /// let mut w = XmlWriter::new(Vec::new(), cfg);
    /// w.start_element("a")?;
    /// w.namespace("p", "urn:p")?;
    /// w.end_element()?;
    /// assert_eq!(w.into_sink(), br#"<a xmlns:p="urn:p"/>"#);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn prefixes(self, p: PrefixPolicy) -> Self {
        XmlWriterConfig { prefixes: p, ..self }
    }
}

/// Writes events as XML text.
///
/// ```
/// use exi::xml::{XmlWriter, XmlWriterConfig};
/// use exi::{EventSink, Value};
/// let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
/// w.start_element(("http://www.w3.org/1999/xhtml", "p"))?;
/// w.attribute("title", "a\"b\n<c>&")?;
/// w.text("x < y & z > w\r")?;
/// w.start_element(("http://www.w3.org/2000/svg", "svg"))?;
/// w.attribute(("http://www.w3.org/1999/xlink", "href"), "#a")?;
/// w.attribute(("http://www.w3.org/XML/1998/namespace", "lang"), "en")?;
/// w.end_element()?;
/// w.start_element("plain")?;
/// w.end_element()?;
/// w.text(Value::Int(-3))?;
/// w.end_element()?;
/// w.end_document()?;
/// assert_eq!(String::from_utf8(w.into_sink()).unwrap(), concat!(
///     r#"<p xmlns="http://www.w3.org/1999/xhtml" title="a&quot;b&#10;&lt;c>&amp;">"#,
///     "x &lt; y &amp; z &gt; w&#13;",
///     r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:ns1="http://www.w3.org/1999/xlink" ns1:href="#a" xml:lang="en"/>"##,
///     r#"<plain xmlns=""/>-3</p>"#));
/// # Ok::<(), exi::Error>(())
/// ```
///
/// A shadowed prefix returns to its outer binding when the element
/// closes.
///
/// ```
/// use exi::xml::{PrefixPolicy, XmlWriter, XmlWriterConfig};
/// use exi::EventSink;
/// let config = XmlWriterConfig::new().prefixes(PrefixPolicy::FromEvents);
/// let mut w = XmlWriter::new(Vec::new(), config);
/// w.start_element("root")?;
/// w.namespace("p", "urn:outer")?;
/// w.start_element("child")?;
/// w.namespace("p", "urn:inner")?;
/// w.attribute(("urn:outer", "a"), "x")?;
/// w.attribute(("urn:inner", "b"), "y")?;
/// w.end_element()?;
/// w.start_element("sibling")?;
/// w.attribute(("urn:outer", "a"), "z")?;
/// w.end_element()?;
/// w.end_element()?;
/// w.end_document()?;
/// assert_eq!(w.into_sink(), br#"<root xmlns:p="urn:outer"><child xmlns:p="urn:inner" xmlns:ns1="urn:outer" ns1:a="x" p:b="y"/><sibling p:a="z"/></root>"#);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// What cannot be written.
///
/// ```
/// use exi::xml::{XmlWriter, XmlWriterConfig};
/// use exi::{EventSink, ErrorKind};
/// let fails = |f: &dyn Fn(&mut XmlWriter<Vec<u8>>) -> exi::Result<()>| {
///     let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
///     w.start_element("r").unwrap();
///     f(&mut w).unwrap_err().kind()
/// };
/// assert_eq!(fails(&|w| w.comment("a -- b")), ErrorKind::Xml);
/// assert_eq!(fails(&|w| w.comment("ends -")), ErrorKind::Xml);
/// assert_eq!(fails(&|w| w.processing_instruction("p", "a ?> b")), ErrorKind::Xml);
/// assert_eq!(fails(&|w| w.processing_instruction("XML", "")), ErrorKind::Xml);
/// assert_eq!(fails(&|w| w.text("\u{0}")), ErrorKind::Xml);
/// assert_eq!(fails(&|w| w.start_element("not a name")), ErrorKind::Xml);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct XmlWriter<S: Sink> {
    sink: S,
    cfg: XmlWriterConfig,
    begun: bool,
    done: bool,
    /// Bindings in scope: prefix ("" for the default namespace) and
    /// URI.
    bindings: Vec<(String, String)>,
    /// Open elements: the written name and the bindings length before
    /// the element.
    stack: Vec<(String, usize)>,
    open: Option<OpenTag>,
}

struct OpenTag {
    name: OwnedName,
    /// Declarations from NS events (`FromEvents`).
    ns: Vec<(String, String)>,
    attrs: Vec<(OwnedName, String)>,
}

impl<S: Sink> XmlWriter<S> {
    ///
    /// ```
    /// use exi::{EventSink, xml::{XmlWriter, XmlWriterConfig}};
    /// assert!(XmlWriter::new(Vec::new(), XmlWriterConfig::new()).into_sink().is_empty());
    /// ```
    pub fn new(sink: S, cfg: XmlWriterConfig) -> Self {
        XmlWriter {
            sink,
            cfg,
            begun: false,
            done: false,
            bindings: Vec::new(),
            stack: Vec::new(),
            open: None,
        }
    }
    /// Ends the document: an error while elements are open. The
    /// `EndDocument` event does the same.
    ///
    /// ```
    /// use exi::{EventSink, xml::{XmlWriter, XmlWriterConfig}};
    /// let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
    /// w.start_element("a")?;
    /// assert_eq!(w.end_document().unwrap_err().kind(), exi::ErrorKind::State);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_document(&mut self) -> Result<()> {
        self.end_doc()
    }
    ///
    /// ```
    /// use exi::{EventSink, xml::{XmlWriter, XmlWriterConfig}};
    /// let w = XmlWriter::new(b"prefix".to_vec(), XmlWriterConfig::new());
    /// assert_eq!(w.into_sink(), b"prefix");
    /// ```
    pub fn into_sink(self) -> S {
        self.sink
    }
}

impl<S: Sink> EventSink for XmlWriter<S> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        self.begin()?;
        match *ev {
            Event::StartDocument | Event::SelfContained => Ok(()),
            Event::EndDocument => self.end_doc(),
            Event::StartTag(t) => {
                self.event(&Event::StartElement(t.name))?;
                for ns in t.ns {
                    self.event(&Event::Namespace(*ns))?;
                }
                for (n, v) in t.attrs {
                    self.event(&Event::Attribute(*n, *v))?;
                }
                Ok(())
            }
            Event::StartElement(n) => {
                check_qname(n)?;
                self.close_tag(false)?;
                self.open = Some(OpenTag {
                    name: n.to_owned(),
                    ns: Vec::new(),
                    attrs: Vec::new(),
                });
                Ok(())
            }
            Event::Namespace(ns) => {
                if self.cfg.prefixes == PrefixPolicy::FromEvents {
                    if let Some(t) = &mut self.open {
                        t.ns.push((ns.prefix.into(), ns.uri.into()));
                    }
                }
                Ok(())
            }
            Event::Attribute(n, v) => {
                check_qname(n)?;
                let text = lexical_string(&v)?;
                check_chars(&text)?;
                match &mut self.open {
                    Some(t) => {
                        t.attrs.push((n.to_owned(), text));
                        Ok(())
                    }
                    None => Err(state("attribute after content")),
                }
            }
            Event::EndElement(_) => {
                if self.open.is_some() {
                    return self.close_tag(true);
                }
                let (name, mark) = self.stack.pop().ok_or_else(|| {
                    state("end_element with no element open")
                })?;
                self.bindings.truncate(mark);
                self.put(&format!("</{name}>"))
            }
            Event::Characters(v) => {
                let owned;
                let text = match v.as_str() {
                    Some(text) => text,
                    None => {
                        owned = lexical_string(&v)?;
                        &owned
                    }
                };
                check_chars(text)?;
                self.close_tag(false)?;
                if self.stack.is_empty() {
                    return Err(state(
                        "character data outside the document element",
                    ));
                }
                if !text.contains(['&', '<', '>', '\r']) {
                    return self.put(text);
                }
                let mut out = String::with_capacity(text.len());
                escape_text(&mut out, text);
                self.put(&out)
            }
            Event::Comment(c) => {
                check_chars(c)?;
                if c.contains("--") || c.ends_with('-') {
                    return Err(xml_err(
                        "a comment holding -- or ending with -",
                    ));
                }
                self.close_tag(false)?;
                self.put(&format!("<!--{c}-->"))
            }
            Event::ProcessingInstruction { target, data } => {
                check_chars(data)?;
                if !is_ncname(target)
                    || target.eq_ignore_ascii_case("xml")
                    || data.contains("?>")
                {
                    return Err(xml_err(
                        "a PI target that is not a name or is xml, or data holding ?>",
                    ));
                }
                self.close_tag(false)?;
                if data.is_empty() {
                    self.put(&format!("<?{target}?>"))
                } else {
                    self.put(&format!("<?{target} {data}?>"))
                }
            }
            Event::Doctype(d) => {
                self.close_tag(false)?;
                let mut out = format!("<!DOCTYPE {}", d.name);
                if !d.public_id.is_empty() {
                    out.push_str(&format!(
                        " PUBLIC \"{}\" \"{}\"",
                        d.public_id, d.system_id
                    ));
                } else if !d.system_id.is_empty() {
                    out.push_str(&format!(" SYSTEM \"{}\"", d.system_id));
                }
                if !d.text.is_empty() {
                    out.push_str(&format!(" [{}]", d.text));
                }
                out.push('>');
                self.put(&out)
            }
            Event::EntityReference(name) => {
                self.close_tag(false)?;
                self.put(&format!("&{name};"))
            }
        }
    }
}

fn escape_text(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}

fn escape_attr(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}

impl<S: Sink> XmlWriter<S> {
    /// The sink, for the C API's accumulating writer.
    #[cfg(feature = "capi")]
    pub(crate) fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    fn lookup(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NS);
        }
        match self.bindings.iter().rev().find(|(p, _)| p == prefix) {
            Some((_, u)) => Some(u),
            None if prefix.is_empty() => Some(""),
            None => None,
        }
    }

    fn put(&mut self, s: &str) -> Result<()> {
        self.sink.write(s.as_bytes())
    }

    fn begin(&mut self) -> Result<()> {
        if !self.begun {
            self.begun = true;
            if self.cfg.declaration {
                self.put("<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
            }
        }
        if self.done {
            return Err(state("event after the end of the document"));
        }
        Ok(())
    }

    /// Writes the buffered start tag, ending it with `>` or, when
    /// `empty`, `/>`.
    fn close_tag(&mut self, empty: bool) -> Result<()> {
        let Some(tag) = self.open.take() else { return Ok(()) };
        let mark = self.bindings.len();
        let from_events = self.cfg.prefixes == PrefixPolicy::FromEvents;
        self.bindings.extend(tag.ns);
        // The element's name.
        let uri = &*tag.name.uri;
        let mut ename = None;
        if from_events {
            let p = tag.name.prefix.as_deref().unwrap_or("");
            if self.lookup(p) == Some(uri) {
                ename = Some(if p.is_empty() {
                    tag.name.local.to_string()
                } else {
                    format!("{p}:{}", tag.name.local)
                });
            }
        }
        let ename = match ename {
            Some(n) => n,
            None => {
                if uri == XML_NS {
                    format!("xml:{}", tag.name.local)
                } else {
                    if self.lookup("") != Some(uri) {
                        self.bindings.push((String::new(), uri.into()));
                    }
                    tag.name.local.to_string()
                }
            }
        };
        let prefixes: Vec<_> = tag
            .attrs
            .iter()
            .map(|(name, _)| self.attribute_prefix(name, from_events))
            .collect();
        let mut out = String::new();
        out.push('<');
        out.push_str(&ename);
        for (p, u) in &self.bindings[mark..] {
            if p.is_empty() {
                out.push_str(" xmlns=\"");
            } else {
                out.push_str(" xmlns:");
                out.push_str(p);
                out.push_str("=\"");
            }
            escape_attr(&mut out, u);
            out.push('"');
        }
        for ((name, value), prefix) in tag.attrs.iter().zip(prefixes) {
            out.push(' ');
            if let Some(i) = prefix {
                out.push_str(&self.bindings[i].0);
                out.push(':');
            } else if &*name.uri == XML_NS {
                out.push_str("xml:");
            }
            out.push_str(&name.local);
            out.push_str("=\"");
            escape_attr(&mut out, value);
            out.push('"');
        }
        if empty {
            out.push_str("/>");
            self.bindings.truncate(mark);
        } else {
            out.push('>');
            self.stack.push((ename, mark));
        }
        self.put(&out)
    }

    fn attribute_prefix(
        &mut self,
        name: &OwnedName,
        from_events: bool,
    ) -> Option<usize> {
        let uri = &*name.uri;
        if uri.is_empty() || uri == XML_NS {
            return None;
        }
        let own = name.prefix.as_deref().filter(|p| !p.is_empty());
        if from_events {
            if let Some(p) = own.filter(|p| *p != "xml") {
                if let Some(i) =
                    self.bindings.iter().rposition(|(prefix, _)| prefix == p)
                {
                    if self.bindings[i].1 == uri {
                        return Some(i);
                    }
                }
            }
        }
        if let Some(i) = self
            .bindings
            .iter()
            .rposition(|(p, _)| !p.is_empty() && self.lookup(p) == Some(uri))
        {
            return Some(i);
        }
        let prefix = match own {
            Some(p) if p != "xmlns" && self.lookup(p).is_none() => p.into(),
            _ => {
                let mut k = 1;
                loop {
                    let p = format!("ns{k}");
                    if self.lookup(&p).is_none() {
                        break p;
                    }
                    k += 1;
                }
            }
        };
        self.bindings.push((prefix, uri.into()));
        Some(self.bindings.len() - 1)
    }

    fn end_doc(&mut self) -> Result<()> {
        self.begin()?;
        if self.open.is_some() || !self.stack.is_empty() {
            return Err(state("end_document while elements are open"));
        }
        self.done = true;
        Ok(())
    }
}
