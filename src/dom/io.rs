// src/dom/io.rs
//
// Conversion between document trees and events
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Trees from events and events from trees: layer 2 reads and writes
//! through layer 1.

use alloc::string::String;
use alloc::vec::Vec;
use core::ptr;

use super::raw::*;
use super::{Document, NodeId, append_raw};
use crate::stream::{EventSink, EventSource, Step};
use crate::strings::GlobalStrings;
use crate::{Error, ErrorKind, Event, EventMeta, Name, NsDecl, Result, Value};

/// Builds a [`Document`] from events: an [`EventSink`], so any reader
/// feeds it, a piece at a time.
///
/// ```
/// use exi::dom::DocumentBuilder;
/// use exi::strings::{GlobalConfig, GlobalStrings};
/// use exi::EventSink;
///
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let mut b = DocumentBuilder::new(&strings);
/// b.start_element("stream")?;
/// b.start_element("message")?;
/// b.attribute("to", "juliet@example.com")?;
/// b.end_element()?;
/// // The stream is still open, and the tree is usable: an XMPP server works this way.
/// let doc = b.document();
/// let stream = doc.root().unwrap();
/// assert_eq!(doc.child_element_count(stream), 1);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct DocumentBuilder<'g> {
    doc: Document<'g>,
    stack: Vec<NodeId>,
    /// The open element's namespace is chosen after its NS events.
    pending: bool,
    text: String,
    line: u32,
    ignore_blanks: bool,
    ended: bool,
}

impl<'g> DocumentBuilder<'g> {
    ///
    /// ```
    /// use exi::{Event, EventSink, dom::DocumentBuilder};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// assert!(builder.document().root().is_none());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(strings: &'g GlobalStrings) -> DocumentBuilder<'g> {
        DocumentBuilder {
            doc: Document::new(strings),
            stack: Vec::new(),
            pending: false,
            text: String::new(),
            line: 0,
            ignore_blanks: false,
            ended: false,
        }
    }

    /// Leave out text that is whitespace only, such as indentation
    /// (libxml2's `XML_PARSE_NOBLANKS`). Off by default.
    ///
    /// ```
    /// # use exi::{dom::DocumentBuilder, strings::{GlobalConfig, GlobalStrings}, EventSink};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings).ignore_blanks(true);
    /// builder.start_element("reading")?;
    /// builder.text("\n  ")?;
    /// builder.start_element("value")?;
    /// builder.text("2")?;
    /// builder.text("1")?;
    /// builder.end_element()?;
    /// builder.end_element()?;
    /// let doc = builder.into_document();
    /// let root = doc.root().unwrap();
    /// let value = doc.first_element_child(root).unwrap();
    /// assert_eq!(doc.children(root).count(), 1);
    /// assert_eq!(doc.children(value).count(), 1);
    /// assert_eq!(doc.content(value), "21");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn ignore_blanks(mut self, on: bool) -> Self {
        self.ignore_blanks = on;
        self
    }

    /// The tree so far. Open elements are in it, without their end.
    ///
    /// ```
    /// use exi::{Event, EventSink, dom::DocumentBuilder};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// builder.start_element("root")?;
    /// assert_eq!(builder.document().root(), builder.current());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn document(&self) -> &Document<'g> {
        &self.doc
    }

    /// Edits the tree built so far. If an edit removes an element
    /// the builder still has open, that element can no longer receive
    /// events. The builder reports `ErrorKind::State` when it reaches
    /// it.
    ///
    /// ```
    /// # use exi::{dom::DocumentBuilder, strings::{GlobalConfig, GlobalStrings}, ErrorKind, EventSink};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// builder.start_element("reading")?;
    /// let open = builder.current().unwrap();
    /// builder.document_mut().remove(open);
    /// assert_eq!(builder.current(), None);
    /// assert_eq!(builder.text("21").unwrap_err().kind(), ErrorKind::State);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn document_mut(&mut self) -> &mut Document<'g> {
        &mut self.doc
    }

    /// The open element, if any.
    ///
    /// ```
    /// use exi::{Event, EventSink, dom::DocumentBuilder};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// assert_eq!(builder.current(), None);
    /// builder.start_element("root")?;
    /// assert!(builder.current().is_some());
    /// builder.end_element()?;
    /// assert_eq!(builder.current(), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn current(&self) -> Option<NodeId> {
        self.stack.last().copied().filter(|&id| self.doc.contains(id))
    }

    /// The document, after its end.
    ///
    /// # Errors
    ///
    /// `ErrorKind::State` before `EndDocument`.
    ///
    /// ```
    /// use exi::{Event, EventSink, dom::DocumentBuilder};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// builder.start_element("root")?;
    /// builder.end_element()?;
    /// builder.event(&Event::EndDocument)?;
    /// assert!(builder.finish()?.root().is_some());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn finish(self) -> Result<Document<'g>> {
        if !self.ended {
            return Err(Error::new(
                ErrorKind::State,
                "the document has not ended",
            ));
        }
        Ok(self.doc)
    }

    /// The tree, ended or not: open elements stay as they are.
    ///
    /// ```
    /// use exi::{Event, EventSink, dom::DocumentBuilder};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// builder.start_element("root")?;
    /// builder.text("partial")?;
    /// let doc = builder.into_document();
    /// assert_eq!(doc.content(doc.root().unwrap()), "partial");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn into_document(mut self) -> Document<'g> {
        let _ = self.flush();
        let _ = self.settle();
        self.doc
    }

    /// An event with its position: the start tag's line goes to `line`.
    ///
    /// ```
    /// use exi::{Event, EventSink, dom::DocumentBuilder};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut builder = DocumentBuilder::new(&strings);
    /// let mut meta = exi::EventMeta::default();
    /// meta.position.line = 7;
    /// builder.event_at(&Event::StartElement(exi::Name::new("", "root")), Some(&meta))?;
    /// let doc = builder.into_document();
    /// let node = doc.raw(doc.root().unwrap()).cast::<exi::dom::raw::RawNode>();
    /// // The root is a live element owned by doc.
    /// assert_eq!(unsafe { (*node).line }, 7);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn event_at(
        &mut self,
        ev: &Event<'_>,
        meta: Option<&EventMeta>,
    ) -> Result<()> {
        if let Some(m) = meta {
            self.line = m.position.line;
        }
        self.event(ev)
    }

    fn open_element(&self) -> Result<Option<*mut RawNode>> {
        self.stack
            .last()
            .map(|&id| {
                self.doc.live_ptr(id).map(|p| p.cast()).ok_or_else(|| {
                    Error::new(ErrorKind::State, "an open element was removed")
                })
            })
            .transpose()
    }

    fn parent(&self) -> Result<*mut RawCommon> {
        Ok(self
            .open_element()?
            .unwrap_or_else(|| self.doc.doc_ptr().cast())
            as *mut RawCommon)
    }

    /// Chooses the open element's namespace, once its declarations are
    /// in.
    fn settle(&mut self) -> Result<()> {
        if self.pending {
            self.pending = false;
            if let Some(el) = self.open_element()? {
                self.doc.fix_ns(el, super::Declare::Here)?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.text.is_empty() {
            return Ok(());
        }
        let text = core::mem::take(&mut self.text);
        if self.stack.is_empty()
            || (self.ignore_blanks
                && text
                    .bytes()
                    .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r')))
        {
            return Ok(());
        }
        let mut text = text.into_bytes();
        text.push(0);
        let n = self.doc.new_text_buffer(text.into_boxed_slice());
        self.place(n)
    }

    fn place(&mut self, n: NodeId) -> Result<()> {
        let p = self.doc.p(n);
        // SAFETY: a new node appended to the open element or the
        // document.
        unsafe { append_raw(self.parent()?, p) };
        Ok(())
    }

    fn start_ev(&mut self, name: Name<'_>) -> Result<()> {
        self.flush()?;
        self.settle()?;
        let p = self.doc.alloc_node(EXI_ELEMENT_NODE);
        self.doc.set_name(p, name)?;
        self.doc.want_prefix(p, name.prefix());
        // SAFETY: a new element appended to the open element or the
        // document.
        unsafe {
            (*(p as *mut RawNode)).line =
                self.line.min(u16::MAX as u32) as u16;
            append_raw(self.parent()?, p);
        }
        self.stack.push(self.doc.id(p));
        self.pending = true;
        Ok(())
    }

    fn ns_ev(&mut self, d: &NsDecl<'_>) -> Result<()> {
        let el = self.open_element()?.ok_or_else(|| {
            Error::new(ErrorKind::State, "a namespace outside an element")
        })?;
        let prefix = (!d.prefix.is_empty()).then_some(d.prefix);
        if prefix == Some("xml") {
            return Ok(());
        }
        let ns = self.doc.declare_raw(el, prefix, d.uri)?;
        if d.local_element_ns {
            // SAFETY: the open element.
            unsafe { (*el).ns = ns };
        }
        Ok(())
    }

    fn attribute_ev(&mut self, name: Name<'_>, v: &Value<'_>) -> Result<()> {
        super::check_attribute_name(name)?;
        self.settle()?;
        let el = self.open_element()?.ok_or_else(|| {
            Error::new(ErrorKind::State, "an attribute outside an element")
        })?;
        let a = self.doc.alloc_attr();
        self.doc.set_name(a, name)?;
        self.doc.want_prefix(a, name.prefix());
        // SAFETY: a new attribute on the open element.
        unsafe { super::append_attr_raw(el, a as *mut RawAttr) };
        self.doc.fix_attr_ns(a as *mut RawAttr, super::Declare::Here)?;
        let t = match v {
            Value::Str(text) => self.doc.new_text(text),
            _ => self.doc.new_value(v.to_owned()),
        };
        let tp = self.doc.p(t);
        // SAFETY: a new text node under the new attribute.
        unsafe { append_raw(a, tp) };
        Ok(())
    }

    fn characters(&mut self, v: &Value<'_>) -> Result<()> {
        if self.stack.is_empty() {
            return Ok(()); // whitespace around the root element
        }
        self.settle()?;
        match v {
            Value::Str(s) => self.text.push_str(s),
            v => {
                self.flush()?;
                let n = self.doc.new_value(v.to_owned());
                self.place(n)?;
            }
        }
        Ok(())
    }
}

impl EventSink for DocumentBuilder<'_> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        if self.ended {
            return Err(Error::new(
                ErrorKind::State,
                "an event after the end of the document",
            ));
        }
        self.open_element()?;
        match ev {
            Event::StartDocument | Event::SelfContained => Ok(()),
            Event::StartElement(n) => self.start_ev(*n),
            Event::Namespace(d) => self.ns_ev(d),
            Event::Attribute(n, v) => self.attribute_ev(*n, v),
            Event::Characters(v) => self.characters(v),
            Event::EndElement(name) => {
                let open = self.current().ok_or_else(|| {
                    Error::new(ErrorKind::State, "an end with no element open")
                })?;
                if !name.is("", "")
                    && !self
                        .doc
                        .name(open)
                        .is_some_and(|n| n.is(name.uri(), name.local_name()))
                {
                    return Err(Error::new(
                        ErrorKind::State,
                        "end element name does not match",
                    ));
                }
                self.flush()?;
                self.settle()?;
                self.stack.pop();
                Ok(())
            }
            Event::Comment(t) => {
                self.flush()?;
                self.settle()?;
                let n = self.doc.new_comment(t);
                self.place(n)?;
                Ok(())
            }
            Event::ProcessingInstruction { target, data } => {
                self.flush()?;
                self.settle()?;
                let n = self.doc.new_pi(target, data);
                self.place(n)?;
                Ok(())
            }
            Event::EntityReference(name) => {
                self.flush()?;
                self.settle()?;
                let n = self.doc.new_entity_ref(name);
                self.place(n)?;
                Ok(())
            }
            Event::Doctype(d) => {
                self.doc.set_doctype(*d);
                Ok(())
            }
            Event::EndDocument => {
                self.flush()?;
                if !self.stack.is_empty() {
                    return Err(Error::new(
                        ErrorKind::State,
                        "the document ended inside an element",
                    ));
                }
                self.ended = true;
                Ok(())
            }
        }
    }
}

impl<'g> Document<'g> {
    /// A document from every event of `source` (`xmlReadMemory` for any
    /// layer 1 reader: XML text, EXI, or your own). The source must
    /// have all its input; for input that arrives in pieces, feed a
    /// [`DocumentBuilder`].
    ///
    /// # Errors
    ///
    /// The source's errors; `ErrorKind::Truncated` if it needs more
    /// input.
    ///
    /// ```
    /// # #[cfg(feature = "xml")] {
    /// use exi::dom::Document;
    /// use exi::grammar::GrammarSet;
    /// use exi::strings::{GlobalConfig, GlobalStrings};
    /// use exi::{Coding, Options, Reader, ReaderConfig, Writer, WriterConfig};
    ///
    /// let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let set = GrammarSet::schemaless();
    /// let g = set.attach(&strings)?;
    /// let doc = Document::parse_xml(&strings, b"<note to='Tove'>Hello</note>")?;
    ///
    /// // To EXI, and back into a tree.
    /// let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &g))?;
    /// doc.write(&mut w)?;
    /// let exi = w.into_sink();
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&g))?;
    /// r.feed(&exi);
    /// r.end_input();
    /// let back = Document::read(&strings, &mut r)?;
    /// assert_eq!(back.to_xml()?, "<note to=\"Tove\">Hello</note>");
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn read(
        strings: &'g GlobalStrings,
        source: &mut impl EventSource,
    ) -> Result<Document<'g>> {
        let mut b = DocumentBuilder::new(strings);
        loop {
            match source.next()? {
                Step::Event(e, meta) => b.event_at(&e, Some(&meta))?,
                Step::NeedInput => {
                    return Err(Error::new(
                        ErrorKind::Truncated,
                        "the source needs more input",
                    ));
                }
                Step::End => break,
            }
        }
        b.finish()
    }

    /// A document from XML text (`xmlReadMemory`). Comments, PIs and
    /// the DOCTYPE are kept.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::parse_xml(&strings, b"<root/>")?;
    /// assert_eq!(doc.node_name(doc.root().unwrap()), "root");
    /// # Ok::<(), exi::Error>(())
    /// ```
    #[cfg(feature = "xml")]
    pub fn parse_xml(
        strings: &'g GlobalStrings,
        xml: &[u8],
    ) -> Result<Document<'g>> {
        Self::parse_xml_with(strings, xml, DocumentBuilder::new(strings))
    }

    /// As [`parse_xml`](Self::parse_xml), with a configured builder.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// use exi::dom::DocumentBuilder;
    /// let builder = DocumentBuilder::new(&strings).ignore_blanks(true);
    /// let doc = Document::parse_xml_with(&strings, b"<root>  <child/>  </root>", builder)?;
    /// assert_eq!(doc.children(doc.root().unwrap()).count(), 1);
    /// # Ok::<(), exi::Error>(())
    /// ```
    #[cfg(feature = "xml")]
    pub fn parse_xml_with(
        strings: &'g GlobalStrings,
        xml: &[u8],
        mut b: DocumentBuilder<'g>,
    ) -> Result<Document<'g>> {
        use crate::grammar::GrammarSet;
        use crate::xml::{XmlConfig, XmlReader};
        let set = GrammarSet::schemaless();
        let g = set.attach(strings)?;
        let mut x: XmlReader = XmlReader::new(
            XmlConfig::new()
                .report_comments(true)
                .report_pis(true)
                .report_doctype(true),
            &g,
        );
        let mut rest = xml;
        loop {
            rest = &rest[x.feed(rest)..];
            if rest.is_empty() {
                x.end_input();
            }
            loop {
                match x.next()? {
                    Step::Event(e, meta) => b.event_at(&e, Some(&meta))?,
                    Step::NeedInput => break,
                    Step::End => return b.finish(),
                }
            }
        }
    }

    /// Writes the document's events to `sink` (`xmlDocDump` for any
    /// layer 1 writer).
    ///
    /// ```
    /// use exi::{dom::Document, Event, EventSource, OwnedEvent, Step};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let mut events: Vec<OwnedEvent> = Vec::new();
    /// doc.write(&mut events)?;
    /// assert_eq!(events.first(), Some(&OwnedEvent::StartDocument));
    /// assert_eq!(events.last(), Some(&OwnedEvent::EndDocument));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn write(&self, sink: &mut impl EventSink) -> Result<()> {
        let mut e = self.events();
        while let Step::Event(ev, _) = e.next()? {
            sink.event(&ev)?;
        }
        Ok(())
    }

    /// Writes one element and everything inside it, with the namespace
    /// declarations it needs from its ancestors: a stanza from a
    /// stream.
    ///
    /// ```
    /// use exi::{dom::Document, Event, EventSource, OwnedEvent, Step};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let mut events: Vec<OwnedEvent> = Vec::new();
    /// doc.write_subtree(root, &mut events)?;
    /// assert!(matches!(events.as_slice(), [OwnedEvent::StartElement(_), OwnedEvent::EndElement(_)]));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn write_subtree(
        &self,
        element: NodeId,
        sink: &mut impl EventSink,
    ) -> Result<()> {
        let mut e = self.subtree_events(element);
        while let Step::Event(ev, _) = e.next()? {
            sink.event(&ev)?;
        }
        Ok(())
    }

    /// The document as events: an [`EventSource`].
    ///
    /// ```
    /// use exi::{dom::Document, Event, EventSource, OwnedEvent, Step};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let mut events = doc.events();
    /// assert!(matches!(events.next()?, Step::Event(Event::StartDocument, _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn events(&self) -> DocEvents<'_, 'g> {
        DocEvents {
            doc: self,
            top: ptr::null_mut(),
            top_ns: Vec::new(),
            state: St::Start,
            scratch: String::new(),
        }
    }

    /// One element as events (no StartDocument or EndDocument).
    /// Its first event is followed by all in-scope namespace bindings,
    /// including inherited bindings that do not name a node.
    ///
    /// ```
    /// use exi::{dom::Document, Event, EventSource, OwnedEvent, Step};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let mut events = doc.subtree_events(root);
    /// assert!(matches!(events.next()?, Step::Event(Event::StartElement(_), _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn subtree_events(&self, element: NodeId) -> DocEvents<'_, 'g> {
        let p = self.p(element);
        // Declarations in scope at the element, nearest first, one per
        // prefix.
        let mut top_ns: Vec<*mut RawNs> = Vec::new();
        // SAFETY: walking live ancestors.
        unsafe {
            if (*p).r#type == EXI_ELEMENT_NODE {
                let mut e = p as *mut RawNode;
                while !e.is_null() && (*e).r#type == EXI_ELEMENT_NODE {
                    let mut ns = (*e).nsDef;
                    while !ns.is_null() {
                        if !top_ns.iter().any(|&o| {
                            (*o).prefix.is_null() == (*ns).prefix.is_null()
                                && str_at((*o).prefix) == str_at((*ns).prefix)
                        }) {
                            top_ns.push(ns);
                        }
                        ns = (*ns).next;
                    }
                    e = (*e).parent;
                }
            }
        }
        DocEvents {
            doc: self,
            top: p,
            top_ns,
            state: St::Enter(p),
            scratch: String::new(),
        }
    }

    /// The document as XML text, without an XML declaration.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::parse_xml(&strings, b"<root/>")?;
    /// assert_eq!(doc.to_xml()?, "<root/>");
    /// # Ok::<(), exi::Error>(())
    /// ```
    #[cfg(feature = "xml")]
    pub fn to_xml(&self) -> Result<String> {
        let b = self.write_xml(crate::xml::XmlWriterConfig::new())?;
        String::from_utf8(b).map_err(|_| {
            Error::new(
                ErrorKind::Internal,
                "the XML writer wrote bytes that are not UTF-8",
            )
        })
    }

    /// The document as XML text with this writer configuration
    /// (`xmlDocDumpMemory`). The tree's own prefixes and declarations
    /// are written.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::parse_xml(&strings, b"<root/>")?;
    /// let cfg = exi::xml::XmlWriterConfig::new().declaration(true);
    /// assert!(doc.write_xml(cfg)?.starts_with(b"<?xml "));
    /// # Ok::<(), exi::Error>(())
    /// ```
    #[cfg(feature = "xml")]
    pub fn write_xml(
        &self,
        cfg: crate::xml::XmlWriterConfig,
    ) -> Result<Vec<u8>> {
        use crate::xml::{PrefixPolicy, XmlWriter};
        let mut w =
            XmlWriter::new(Vec::new(), cfg.prefixes(PrefixPolicy::FromEvents));
        self.write(&mut w)?;
        Ok(w.into_sink())
    }

    /// Builds a document from owned events, mainly for tests.
    ///
    /// ```
    /// use exi::{dom::Document, Event, EventSource, OwnedEvent, Step};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let mut events: Vec<OwnedEvent> = Vec::new();
    /// doc.write(&mut events)?;
    /// let rebuilt = Document::from_events(&strings, &events)?;
    /// assert_eq!(rebuilt.node_name(rebuilt.root().unwrap()), "root");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn from_events(
        strings: &'g GlobalStrings,
        events: &[crate::OwnedEvent],
    ) -> Result<Document<'g>> {
        let mut b = DocumentBuilder::new(strings);
        for e in events {
            b.event(&e.as_event())?;
        }
        b.finish()
    }
}

#[derive(Clone, Copy)]
enum St {
    Start,
    Enter(*mut RawCommon),
    TopNs(*mut RawNode, usize),
    Ns(*mut RawNode, *mut RawNs),
    Attr(*mut RawNode, *mut RawAttr),
    Leave(*mut RawNode),
    EndDoc,
    Done,
}

/// A document's events, in document order: an [`EventSource`] from
/// [`Document::events`] or [`Document::subtree_events`].
pub struct DocEvents<'d, 'g> {
    doc: &'d Document<'g>,
    /// The element of a subtree; NULL for the whole document.
    top: *mut RawCommon,
    top_ns: Vec<*mut RawNs>,
    state: St,
    scratch: String,
}

impl DocEvents<'_, '_> {
    /// The state after node `p` is done.
    fn after(&self, p: *mut RawCommon) -> St {
        if p == self.top {
            return St::Done;
        }
        // SAFETY: live nodes of the borrowed document.
        unsafe {
            let next = (*p).next;
            if !next.is_null() {
                return St::Enter(next as *mut RawCommon);
            }
            let parent = (*p).parent;
            if parent.is_null() || (*parent).r#type == EXI_DOCUMENT_NODE {
                if self.top.is_null() { St::EndDoc } else { St::Done }
            } else {
                St::Leave(parent)
            }
        }
    }
}

impl EventSource for DocEvents<'_, '_> {
    fn next(&mut self) -> Result<Step<'_>> {
        let meta = EventMeta::default();
        let doc = self.doc;
        loop {
            // SAFETY: every pointer in the state is a live node of
            // `doc`, which is borrowed for the life of `self`, so the
            // strings returned live long enough.
            unsafe {
                match self.state {
                    St::Start => {
                        let first = (*doc.doc_ptr()).children;
                        self.state = if first.is_null() {
                            St::EndDoc
                        } else {
                            St::Enter(first as *mut RawCommon)
                        };
                        return Ok(Step::Event(Event::StartDocument, meta));
                    }
                    St::Enter(p) => match (*p).r#type {
                        EXI_ELEMENT_NODE => {
                            let el = p as *mut RawNode;
                            self.state =
                                if p == self.top && !self.top_ns.is_empty() {
                                    St::TopNs(el, 0)
                                } else {
                                    St::Ns(el, (*el).nsDef)
                                };
                            let name =
                                doc.name(doc.id(p)).unwrap_or(Name::local(""));
                            return Ok(Step::Event(
                                Event::StartElement(name),
                                meta,
                            ));
                        }
                        EXI_TEXT_NODE | EXI_CDATA_SECTION_NODE => {
                            self.state = self.after(p);
                            let v =
                                doc.value(doc.id(p)).unwrap_or(Value::Str(""));
                            return Ok(Step::Event(
                                Event::Characters(v),
                                meta,
                            ));
                        }
                        EXI_COMMENT_NODE => {
                            self.state = self.after(p);
                            let x = extra(p);
                            return Ok(Step::Event(
                                Event::Comment(x.content.text()),
                                meta,
                            ));
                        }
                        EXI_PI_NODE => {
                            self.state = self.after(p);
                            let x = extra(p);
                            let data = x.content.text();
                            return Ok(Step::Event(
                                Event::ProcessingInstruction {
                                    target: str_at((*p).name),
                                    data,
                                },
                                meta,
                            ));
                        }
                        EXI_ENTITY_REF_NODE => {
                            self.state = self.after(p);
                            return Ok(Step::Event(
                                Event::EntityReference(str_at((*p).name)),
                                meta,
                            ));
                        }
                        EXI_DTD_NODE => {
                            self.state = self.after(p);
                            if let Some(d) = doc.doctype() {
                                return Ok(Step::Event(
                                    Event::Doctype(d),
                                    meta,
                                ));
                            }
                        }
                        _ => self.state = self.after(p),
                    },
                    St::TopNs(el, i) => match self.top_ns.get(i) {
                        Some(&ns) => {
                            self.state = St::TopNs(el, i + 1);
                            return Ok(Step::Event(
                                Event::Namespace(ns_event(ns, el)),
                                meta,
                            ));
                        }
                        None => self.state = St::Attr(el, (*el).properties),
                    },
                    St::Ns(el, ns) => {
                        if ns.is_null() {
                            self.state = St::Attr(el, (*el).properties);
                        } else {
                            self.state = St::Ns(el, (*ns).next);
                            return Ok(Step::Event(
                                Event::Namespace(ns_event(ns, el)),
                                meta,
                            ));
                        }
                    }
                    St::Attr(el, a) => {
                        if a.is_null() {
                            self.state = if (*el).children.is_null() {
                                St::Leave(el)
                            } else {
                                St::Enter((*el).children as *mut RawCommon)
                            };
                        } else {
                            self.state = St::Attr(el, (*a).next);
                            let id = doc.id(a as *mut RawCommon);
                            let name = doc.name(id).unwrap_or(Name::local(""));
                            let v = match doc.value(id) {
                                Some(v) => v,
                                None => {
                                    self.scratch = doc.content(id);
                                    // The event borrows the scratch
                                    // buffer until the next call to
                                    // next().
                                    Value::Str(&self.scratch)
                                }
                            };
                            return Ok(Step::Event(
                                Event::Attribute(name, v),
                                meta,
                            ));
                        }
                    }
                    St::Leave(el) => {
                        self.state = self.after(el as *mut RawCommon);
                        let name = doc
                            .name(doc.id(el as *mut RawCommon))
                            .unwrap_or(Name::local(""));
                        return Ok(Step::Event(Event::EndElement(name), meta));
                    }
                    St::EndDoc => {
                        self.state = St::Done;
                        return Ok(Step::Event(Event::EndDocument, meta));
                    }
                    St::Done => return Ok(Step::End),
                }
            }
        }
    }
}

/// An NS event for a declaration on (or in scope at) `el`.
///
/// # Safety
///
/// `ns` and `el` are live.
unsafe fn ns_event<'a>(ns: *mut RawNs, el: *mut RawNode) -> NsDecl<'a> {
    // SAFETY: the caller's contract; the strings live in held table
    // entries.
    unsafe {
        NsDecl {
            prefix: str_at((*ns).prefix),
            uri: str_at((*ns).href),
            local_element_ns: (*el).ns == ns,
        }
    }
}
