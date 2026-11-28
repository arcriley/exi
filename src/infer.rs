// src/infer.rs
//
// Schema learning by inference
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! [`SchemaInference`] reads example XML documents and writes XML
//! Schema documents that accept them: one per namespace, as text any
//! XSD tool can read. [`SchemaInference::grammars`] compiles those with
//! this crate's XSD compiler, so a schema learned from examples is used
//! exactly like one written by hand: for strict schema-informed
//! streams, or saved as an `.exig`.
//!
//! ```
//! use exi::infer::SchemaInference;
//! let mut learn = SchemaInference::new();
//! learn.document(br#"<order id="17"><item sku="A-1" qty="2"/><item sku="B-7" qty="1"/><note>rush</note></order>"#)?;
//! learn.document(br#"<order id="18"><item sku="C-3" qty="5"/></order>"#)?;
//! let xsd = learn.schemas();
//! assert_eq!(xsd.len(), 1);                               // no namespaces: one document
//! assert_eq!(xsd[0].text, r#"<?xml version="1.0" encoding="UTF-8"?>
//! <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" elementFormDefault="qualified">
//!   <xs:element name="item">
//!     <xs:complexType>
//!       <xs:attribute name="qty" type="xs:unsignedInt" use="required"/>
//!       <xs:attribute name="sku" type="xs:string" use="required"/>
//!     </xs:complexType>
//!   </xs:element>
//!   <xs:element name="note" type="xs:string"/>
//!   <xs:element name="order">
//!     <xs:complexType>
//!       <xs:sequence>
//!         <xs:element ref="item" maxOccurs="unbounded"/>
//!         <xs:element ref="note" minOccurs="0"/>
//!       </xs:sequence>
//!       <xs:attribute name="id" type="xs:unsignedInt" use="required"/>
//!     </xs:complexType>
//!   </xs:element>
//! </xs:schema>
//! "#);
//! let grammars = learn.grammars()?;                       // ready for strict streams
//! assert!(grammars.name_id("", "sku").is_some());
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # What is learned
//!
//! Every element name becomes a global element declaration, so an
//! element means the same wherever it appears, and recursion needs
//! nothing special.
//!
//! | Seen in the examples | Declared |
//! |---|---|
//! | Children always in one order, each name in one run | `xs:sequence` of references in that order |
//! | Children in varying order, or a name in two runs | `xs:choice minOccurs="0" maxOccurs="unbounded"` |
//! | A child missing from some instances | `minOccurs="0"` |
//! | A child repeated in a run | `maxOccurs="unbounded"` |
//! | Text and children in one element | `mixed="true"` |
//! | Text only | a simple type, or simple content with the attributes |
//! | Neither | empty content |
//! | An attribute on every instance | `use="required"` |
//! | An attribute in a namespace (`xlink:href`, `xml:lang`) | a global attribute in that namespace's document |
//!
//! The order of the examples does not change the schema: a choice lists
//! its members by name, and so does a sequence where the examples leave
//! the order open. Learners given the same documents in any order make
//! grammars that read each other's streams (EXI codes a choice's
//! elements in schema order).
//!
//! Indentation between elements is not text: it is whitespace only.
//! `xsi:nil="true"` marks the element `nillable="true"` without
//! widening its type to `string`; comments, PIs and other `xsi:`
//! attributes are not learned (strict streams cannot carry comments or
//! PIs, and `xsi:` attributes are not declared in schemas).
//!
//! Values get the narrowest type all of them fit, tried in this order:
//! `boolean` (`true`/`false`), an integer type, `decimal`, `double`,
//! `dateTime`, `date`, `time`, then `string`. An element that is empty
//! in some instances has the value `""`, which only `string` fits.
//!
//! # Fit
//!
//! [`Fit::General`] (the default) declares built-in types only, so
//! other documents like the examples are likely to fit too.
//! [`Fit::Exact`] also declares what the examples show and
//! nothing more, for the smallest streams of exactly these documents:
//! integer ranges as facets (EXI codes a range of 4096 values or fewer
//! in n bits), and strings that repeat a few values as enumerations.
//!
//! ```
//! use exi::infer::{Fit, SchemaInference};
//! let doc = br#"<log><e level="info" code="200"/><e level="warn" code="404"/><e level="info" code="200"/><e level="info" code="301"/></log>"#;
//! let mut learn = SchemaInference::new().fit(Fit::Exact);
//! learn.document(doc)?;
//! let text = &learn.schemas()[0].text;
//! assert!(text.contains(r#"<xs:minInclusive value="200"/>"#));
//! assert!(text.contains(r#"<xs:enumeration value="info"/>"#));
//!
//! let mut general = SchemaInference::new();
//! general.document(doc)?;
//! assert!(general.schemas()[0].text.contains(r#"<xs:attribute name="level" type="xs:string" use="required"/>"#));
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # Namespaces
//!
//! Each namespace gets its own document; references across them use
//! `xs:import`. The first document is for the namespace of the first
//! document element. Prefixes are the ones the examples used, where
//! they are free.
//!
//! ```
//! use exi::infer::SchemaInference;
//! let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10">
//!   <use xlink:href="#a" x="1.5"/>
//! </svg>"##;
//! let mut learn = SchemaInference::new();
//! learn.document(svg)?;
//! let docs = learn.schemas();
//! assert_eq!(docs.iter().map(|d| (d.namespace.as_str(), d.location.as_str())).collect::<Vec<_>>(),
//!            [("http://www.w3.org/2000/svg", "schema.xsd"), ("http://www.w3.org/1999/xlink", "schema-xlink.xsd")]);
//! assert!(docs[0].text.contains(r#"<xs:import namespace="http://www.w3.org/1999/xlink" schemaLocation="schema-xlink.xsd"/>"#));
//! assert!(docs[0].text.contains(r#"<xs:attribute ref="xlink:href" use="required"/>"#));
//! assert!(docs[1].text.contains(r#"<xs:attribute name="href" type="xs:string"/>"#));
//! learn.grammars()?;
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # Streams of documents
//!
//! [`SchemaInference`] is an [`EventSink`], so any event source feeds
//! it: an XML reader over a large file a piece at a time, or an EXI
//! reader. It keeps counts and up to
//! [`SchemaInference::max_enumeration`] distinct values per slot, never
//! whole documents.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

use crate::grammar::GrammarSet;
use crate::lexical;
use crate::stream::EventSink;
use crate::strings::{GlobalConfig, GlobalStrings};
use crate::value::DateTimeKind;
use crate::xml::{XmlConfig, XmlReader};
use crate::xsd::XsdCompiler;
use crate::{Error, ErrorKind, Event, Result, Step};

const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// How closely the schema follows the examples. See the [module documentation](self).
///
/// ```
/// use exi::infer::Fit;
/// assert_eq!(Fit::default(), Fit::General);
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Built-in types only.
    #[default]
    General,
    /// Also integer ranges and enumerations, from the values seen.
    Exact,
}

/// One schema document: its target namespace (`""` for none), the
/// location the other documents import it by, and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferredSchema {
    pub namespace: String,
    pub location: String,
    pub text: String,
}

/// Learns a schema from example documents. See the [module documentation](self).
#[derive(Debug)]
pub struct SchemaInference {
    fit: Fit,
    max_enum: usize,
    elements: Vec<ElemStats>,
    index: BTreeMap<String, BTreeMap<String, usize>>,
    /// Global attributes (namespaced): their values over every element.
    globals: BTreeMap<(String, String), Values>,
    /// The first document element's namespace.
    first_root: Option<String>,
    /// Prefixes the examples used, by URI, first one seen.
    prefixes: BTreeMap<String, String>,
    stack: Vec<Frame>,
}

impl Default for SchemaInference {
    fn default() -> Self {
        Self {
            fit: Fit::default(),
            max_enum: 16,
            elements: Vec::new(),
            index: BTreeMap::new(),
            globals: BTreeMap::new(),
            first_root: None,
            prefixes: BTreeMap::new(),
            stack: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct ElemStats {
    uri: String,
    local: String,
    count: u64,
    attrs: Vec<AttrStats>,
    /// Values of instances with no child elements.
    text: Values,
    /// Instances with child elements.
    with_children: u64,
    /// Some instance had text that is not whitespace and child
    /// elements, or instances with text and instances with children.
    text_seen: bool,
    mixed: bool,
    nillable: bool,
    nilled: u64,
    model: Model,
}

#[derive(Debug, Default)]
struct AttrStats {
    uri: String,
    local: String,
    count: u64,
    values: Values,
}

/// What the child sequences of all instances allow.
#[derive(Debug)]
struct Model {
    /// Child element indexes in order of first appearance.
    names: Vec<usize>,
    /// a before b in some instance.
    before: BTreeSet<(usize, usize)>,
    /// Instances in which each name appears.
    present: BTreeMap<usize, u64>,
    repeats: BTreeSet<usize>,
    /// Still a sequence: no name in two runs of one instance, and no
    /// order conflicts.
    sequence: bool,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            names: Vec::new(),
            before: BTreeSet::new(),
            present: BTreeMap::new(),
            repeats: BTreeSet::new(),
            sequence: true,
        }
    }
}

#[derive(Debug)]
struct Frame {
    elem: usize,
    /// Runs of children: element index and count.
    runs: Vec<(usize, u64)>,
    text: String,
    text_nonws: bool,
    nilled: bool,
    attrs: Vec<usize>,
}

/// Which lexical types every value seen fits.
const BOOL: u8 = 1;
const INT: u8 = 2;
const DECIMAL: u8 = 4;
const DOUBLE: u8 = 8;
const DATETIME: u8 = 16;
const DATE: u8 = 32;
const TIME: u8 = 64;

#[derive(Debug)]
struct Values {
    count: u64,
    fits: u8,
    /// Integer range; `None` once a value is beyond i64.
    range: Option<(i64, i64)>,
    negative: bool,
    /// Candidate enumeration values, while there are few.
    distinct: BTreeSet<String>,
    many: bool,
}

impl Default for Values {
    fn default() -> Self {
        Values {
            count: 0,
            fits: 0x7F,
            range: None,
            negative: false,
            distinct: BTreeSet::new(),
            many: false,
        }
    }
}

impl Values {
    fn add(&mut self, v: &str, max_enum: usize) {
        let first = self.count == 0;
        self.count += 1;
        let mut fits = 0;
        if matches!(
            v.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r')),
            "true" | "false"
        ) {
            fits |= BOOL;
        }
        match lexical::parse_int(v) {
            Some(Ok(i)) => {
                fits |= INT;
                self.negative |= i < 0;
                if first {
                    self.range = Some((i, i));
                } else if let Some((lo, hi)) = &mut self.range {
                    *lo = (*lo).min(i);
                    *hi = (*hi).max(i);
                }
            }
            Some(Err((neg, _))) => {
                fits |= INT;
                self.negative |= neg;
                self.range = None;
                self.many = true;
                self.distinct.clear();
            }
            None => {}
        }
        if lexical::parse_decimal(v).is_some() {
            fits |= DECIMAL;
        }
        if lexical::parse_float(v).is_some() {
            fits |= DOUBLE;
        }
        for (bit, kind) in [
            (DATETIME, DateTimeKind::DateTime),
            (DATE, DateTimeKind::Date),
            (TIME, DateTimeKind::Time),
        ] {
            if lexical::parse_datetime(v, kind).is_some() {
                fits |= bit;
            }
        }
        self.fits &= fits;
        if self.fits & INT == 0 {
            self.range = None;
        }
        if !self.many {
            let candidate = !v.is_empty()
                && v.trim_matches(|c| {
                    matches!(c, ' ' | '\t' | '\n' | '\r')
                }) == v;
            if !candidate {
                self.many = true;
                self.distinct.clear();
            } else if !self.distinct.contains(v) {
                if self.distinct.len() >= max_enum {
                    self.many = true;
                    self.distinct.clear();
                } else {
                    self.distinct.insert(v.to_string());
                }
            }
        }
    }
}

/// A simple type: a built-in name, or a restriction of one.
enum Simple {
    Builtin(&'static str),
    Range(&'static str, i64, i64),
    Enumeration(Vec<String>),
}

impl SchemaInference {
    /// Starts with no examples and the default inference policy.
    ///
    /// ```
    /// use exi::{ErrorKind, infer::SchemaInference};
    /// let learned = SchemaInference::new();
    /// assert!(learned.schemas().is_empty());
    /// assert_eq!(learned.grammars().unwrap_err().kind(), ErrorKind::State);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// How closely to follow the examples. Default [`Fit::General`].
    /// The fit is applied when the schemas are emitted.
    ///
    /// ```
    /// use exi::infer::{Fit, SchemaInference};
    /// let mut learned = SchemaInference::new();
    /// learned.document(b"<temperature>21</temperature>")?;
    /// assert!(learned.schemas()[0].text.contains(r#"type="xs:unsignedInt""#));
    /// let exact = learned.fit(Fit::Exact);
    /// assert!(exact.schemas()[0].text.contains(r#"<xs:maxInclusive value="21"/>"#));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn fit(mut self, f: Fit) -> Self {
        self.fit = f;
        self
    }

    /// The most distinct values an enumeration may have under
    /// [`Fit::Exact`]. Default 16. A slot with more values is not an
    /// enumeration. Candidates must occur at least twice on average.
    ///
    /// ```
    /// use exi::infer::{Fit, SchemaInference};
    /// let xml = br#"<log><e level="info"/><e level="warn"/><e level="info"/><e level="warn"/></log>"#;
    /// let mut default = SchemaInference::default().fit(Fit::Exact);
    /// let mut capped = SchemaInference::new().fit(Fit::Exact).max_enumeration(1);
    /// default.document(xml)?;
    /// capped.document(xml)?;
    /// assert!(default.schemas()[0].text.contains(r#"<xs:enumeration value="info"/>"#));
    /// assert!(capped.schemas()[0].text.contains(r#"name="level" type="xs:string""#));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn max_enumeration(mut self, n: usize) -> Self {
        self.max_enum = n;
        self
    }

    /// Learns from one whole document.
    ///
    /// # Errors
    ///
    /// The XML reader's errors (`ErrorKind::Xml` for a document that is
    /// not well-formed). Inference consumes events as they arrive, so
    /// an error can leave partial statistics. Discard the learner if
    /// the failed document must have no effect.
    ///
    /// ```
    /// use exi::{ErrorKind, infer::SchemaInference};
    /// let mut learned = SchemaInference::new();
    /// let error = learned.document(b"<reading></other>").unwrap_err();
    /// assert_eq!(error.kind(), ErrorKind::Xml);
    /// ```
    pub fn document(&mut self, xml: &[u8]) -> Result<()> {
        let strings = GlobalStrings::new(GlobalConfig::new())?;
        let set = GrammarSet::schemaless();
        let g = set.attach(&strings)?;
        let mut r: XmlReader = XmlReader::new(
            XmlConfig::new().report_comments(false).report_pis(false),
            &g,
        );
        let mut rest = xml;
        loop {
            rest = &rest[r.feed(rest)..];
            if rest.is_empty() {
                r.end_input();
            }
            loop {
                match r.next()? {
                    Step::Event(e, _) => self.event(&e)?,
                    Step::NeedInput => break,
                    Step::End => return Ok(()),
                }
            }
        }
    }

    fn elem(&mut self, uri: &str, local: &str) -> usize {
        if let Some(&i) =
            self.index.get(uri).and_then(|names| names.get(local))
        {
            return i;
        }
        self.elements.push(ElemStats {
            uri: uri.into(),
            local: local.into(),
            ..ElemStats::default()
        });
        let i = self.elements.len() - 1;
        self.index.entry(uri.into()).or_default().insert(local.into(), i);
        i
    }

    fn end(&mut self) -> Result<()> {
        let f = self.stack.pop().ok_or_else(|| {
            Error::new(ErrorKind::State, "EndElement with no element open")
        })?;
        let max_enum = self.max_enum;
        let e = &mut self.elements[f.elem];
        if f.nilled {
            e.nilled += 1;
            return Ok(());
        }
        if f.runs.is_empty() {
            e.text.add(&f.text, max_enum);
            if f.text_nonws {
                e.text_seen = true;
            }
        } else {
            e.with_children += 1;
            if f.text_nonws {
                e.mixed = true;
            }
            let m = &mut e.model;
            let mut seen: Vec<usize> = Vec::new();
            for &(c, n) in &f.runs {
                if seen.contains(&c) {
                    m.sequence = false;
                } else {
                    for &b in &seen {
                        m.before.insert((b, c));
                    }
                    seen.push(c);
                }
                if n > 1 {
                    m.repeats.insert(c);
                }
                if !m.names.contains(&c) {
                    m.names.push(c);
                }
            }
            for c in seen {
                *m.present.entry(c).or_insert(0) += 1;
            }
        }
        Ok(())
    }

    // ----- writing the schemas -----

    /// The schema documents, the first one for the first document
    /// element's namespace.
    ///
    /// ```
    /// use exi::infer::SchemaInference;
    /// let mut learned = SchemaInference::new();
    /// learned.document(br#"<root xmlns="urn:root" xmlns:none="urn:child"><item xmlns=""/><none:item/></root>"#)?;
    /// let docs = learned.schemas();
    /// assert_eq!(docs[0].namespace, "urn:root");
    /// assert_eq!(docs.len(), 3);
    /// let files: std::collections::BTreeSet<_> = docs.iter().map(|d| &d.location).collect();
    /// assert_eq!(files.len(), docs.len());
    /// learned.grammars()?;
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn schemas(&self) -> Vec<InferredSchema> {
        let order = self.namespace_order();
        let prefix = self.prefix_map(&order);
        let location = |ns: &str, i: usize| -> String {
            if i == 0 {
                "schema.xsd".into()
            } else if ns.is_empty() {
                "schema-none.xsd".into()
            } else {
                format!("schema-{}.xsd", prefix[ns])
            }
        };
        let locations: BTreeMap<&str, String> = order
            .iter()
            .enumerate()
            .map(|(i, ns)| (ns.as_str(), location(ns, i)))
            .collect();
        order
            .iter()
            .map(|ns| InferredSchema {
                namespace: ns.clone(),
                location: locations[ns.as_str()].clone(),
                text: self.schema_text(ns, &prefix, &locations),
            })
            .collect()
    }

    /// A qualified reference to a global component in `uri` from the
    /// document for `from`.
    fn qref(
        prefix: &BTreeMap<String, String>,
        uri: &str,
        local: &str,
    ) -> String {
        if uri.is_empty() {
            local.to_string()
        } else {
            format!("{}:{local}", prefix[uri])
        }
    }

    fn schema_text(
        &self,
        ns: &str,
        prefix: &BTreeMap<String, String>,
        locations: &BTreeMap<&str, String>,
    ) -> String {
        let mut body = String::new();
        let mut refs: BTreeSet<String> = BTreeSet::new();
        let mut types = String::new();
        let mut type_names: BTreeSet<String> = BTreeSet::new();
        let mut elems: Vec<&ElemStats> =
            self.elements.iter().filter(|e| e.uri == ns).collect();
        elems.sort_by(|a, b| a.local.cmp(&b.local));
        for e in elems {
            self.element_text(
                e,
                prefix,
                &mut body,
                &mut refs,
                &mut types,
                &mut type_names,
            );
        }
        for ((u, l), v) in &self.globals {
            if u != ns {
                continue;
            }
            let _ = write!(body, "  <xs:attribute name=\"{}\"", esc(l));
            self.type_attr(
                &mut body,
                v,
                &format!("{l}-value"),
                &mut types,
                &mut type_names,
                prefix.get(ns).map(String::as_str),
            );
            body.push_str("/>\n");
        }
        let mut out = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"",
        );
        // Bindings for every namespace referred to, this one included.
        refs.insert(ns.to_string());
        for u in &refs {
            if !u.is_empty() && u != XML_NS {
                let _ = write!(out, " xmlns:{}=\"{}\"", prefix[u], esc(u));
            }
        }
        if !ns.is_empty() {
            let _ = write!(out, " targetNamespace=\"{}\"", esc(ns));
        }
        out.push_str(" elementFormDefault=\"qualified\">\n");
        for u in &refs {
            if u != ns {
                if u.is_empty() {
                    let _ = writeln!(
                        out,
                        "  <xs:import schemaLocation=\"{}\"/>",
                        esc(&locations[u.as_str()])
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "  <xs:import namespace=\"{}\" schemaLocation=\"{}\"/>",
                        esc(u),
                        esc(&locations[u.as_str()])
                    );
                }
            }
        }
        out.push_str(&types);
        out.push_str(&body);
        out.push_str("</xs:schema>\n");
        out
    }

    fn element_text(
        &self,
        e: &ElemStats,
        prefix: &BTreeMap<String, String>,
        out: &mut String,
        refs: &mut BTreeSet<String>,
        types: &mut String,
        names: &mut BTreeSet<String>,
    ) {
        let _ = write!(out, "  <xs:element name=\"{}\"", esc(&e.local));
        let nillable = if e.nillable { " nillable=\"true\"" } else { "" };
        let children = e.with_children > 0;
        let text_only = !children && e.text_seen;
        let mixed = children && (e.mixed || e.text_seen);
        let mut attrs: Vec<&AttrStats> = e.attrs.iter().collect();
        attrs.sort_by(|a, b| (&a.local, &a.uri).cmp(&(&b.local, &b.uri)));
        if text_only && attrs.is_empty() {
            self.type_attr(
                out,
                &e.text,
                &format!("{}-value", e.local),
                types,
                names,
                prefix.get(&e.uri).map(String::as_str),
            );
            let _ = writeln!(out, "{nillable}/>");
            return;
        }
        let _ = writeln!(out, "{nillable}>");
        if mixed {
            out.push_str("    <xs:complexType mixed=\"true\">\n");
        } else {
            out.push_str("    <xs:complexType>\n");
        }
        let mut attr_text = String::new();
        let ind = if text_only { "        " } else { "      " };
        for a in &attrs {
            let required =
                if a.count == e.count { " use=\"required\"" } else { "" };
            if a.uri.is_empty() {
                let _ = write!(
                    attr_text,
                    "{ind}<xs:attribute name=\"{}\"",
                    esc(&a.local)
                );
                self.type_attr(
                    &mut attr_text,
                    &a.values,
                    &format!("{}-{}", e.local, a.local),
                    types,
                    names,
                    prefix.get(&e.uri).map(String::as_str),
                );
                let _ = writeln!(attr_text, "{required}/>");
            } else {
                refs.insert(a.uri.clone());
                let _ = writeln!(
                    attr_text,
                    "{ind}<xs:attribute ref=\"{}\"{required}/>",
                    esc(&Self::qref(prefix, &a.uri, &a.local))
                );
            }
        }
        if text_only {
            out.push_str("      <xs:simpleContent>\n");
            let base = self.simple_name(
                &e.text,
                &format!("{}-value", e.local),
                types,
                names,
                prefix.get(&e.uri).map(String::as_str),
            );
            let _ = writeln!(
                out,
                "        <xs:extension base=\"{}\">",
                esc(&base)
            );
            out.push_str(&attr_text);
            out.push_str(
                "        </xs:extension>\n      </xs:simpleContent>\n",
            );
        } else {
            if children {
                let m = &e.model;
                // Name order, so the examples' order does not matter.
                let mut names = m.names.clone();
                names.sort_by_key(|&c| {
                    (&self.elements[c].local, &self.elements[c].uri)
                });
                let order =
                    if m.sequence { topo(&names, &m.before) } else { None };
                let (open, close, items) = match &order {
                    Some(o) => (
                        "      <xs:sequence>\n",
                        "      </xs:sequence>\n",
                        o.clone(),
                    ),
                    None => (
                        "      <xs:choice minOccurs=\"0\" maxOccurs=\"unbounded\">\n",
                        "      </xs:choice>\n",
                        names,
                    ),
                };
                out.push_str(open);
                for c in items {
                    let ce = &self.elements[c];
                    refs.insert(ce.uri.clone());
                    let _ = write!(
                        out,
                        "        <xs:element ref=\"{}\"",
                        esc(&Self::qref(prefix, &ce.uri, &ce.local))
                    );
                    if order.is_some() {
                        if m.present.get(&c).copied().unwrap_or(0)
                            < e.count - e.nilled
                        {
                            out.push_str(" minOccurs=\"0\"");
                        }
                        if m.repeats.contains(&c) {
                            out.push_str(" maxOccurs=\"unbounded\"");
                        }
                    }
                    out.push_str("/>\n");
                }
                out.push_str(close);
            }
            out.push_str(&attr_text);
        }
        out.push_str("    </xs:complexType>\n  </xs:element>\n");
    }

    /// ` type="…"` for a slot's values, adding a named restriction to
    /// `types` when needed.
    fn type_attr(
        &self,
        out: &mut String,
        v: &Values,
        name: &str,
        types: &mut String,
        names: &mut BTreeSet<String>,
        prefix: Option<&str>,
    ) {
        let t = self.simple_name(v, name, types, names, prefix);
        let _ = write!(out, " type=\"{}\"", esc(&t));
    }

    /// The QName of a slot's type: `xs:…`, or a named restriction
    /// written to `types`.
    fn simple_name(
        &self,
        v: &Values,
        name: &str,
        types: &mut String,
        names: &mut BTreeSet<String>,
        prefix: Option<&str>,
    ) -> String {
        let (base, facets) = match self.simple(v) {
            Simple::Builtin(b) => return format!("xs:{b}"),
            Simple::Range(b, lo, hi) => (
                b,
                format!(
                    "      <xs:minInclusive value=\"{lo}\"/>\n      <xs:maxInclusive value=\"{hi}\"/>\n"
                ),
            ),
            Simple::Enumeration(vs) => (
                "string",
                vs.iter()
                    .map(|x| {
                        format!(
                            "      <xs:enumeration value=\"{}\"/>\n",
                            esc(x)
                        )
                    })
                    .collect(),
            ),
        };
        let mut n = ncname(name);
        let mut i = 2;
        while names.contains(&n) {
            n = format!("{}{i}", ncname(name));
            i += 1;
        }
        names.insert(n.clone());
        let _ = write!(
            types,
            "  <xs:simpleType name=\"{n}\">\n    <xs:restriction base=\"xs:{base}\">\n{facets}    </xs:restriction>\n  </xs:simpleType>\n"
        );
        match prefix {
            Some(prefix) => format!("{prefix}:{n}"),
            None => n,
        }
    }

    fn namespace_order(&self) -> Vec<String> {
        // Namespaces: of elements and of global attributes.
        let mut spaces: BTreeSet<String> =
            self.elements.iter().map(|e| e.uri.clone()).collect();
        spaces.extend(self.globals.keys().map(|(u, _)| u.clone()));
        let mut order: Vec<String> = Vec::new();
        if let Some(f) = &self.first_root {
            order.push(f.clone());
        }
        order.extend(
            spaces.into_iter().filter(|s| Some(s) != self.first_root.as_ref()),
        );
        order
    }

    /// The prefix of each namespace in the schema documents: the
    /// examples' prefix where it is free and usable, else `ns1`, `ns2`,
    /// …
    fn prefix_map(&self, order: &[String]) -> BTreeMap<String, String> {
        let mut prefix = BTreeMap::new();
        prefix.insert(XML_NS.to_string(), "xml".to_string());
        let mut used: BTreeSet<String> = ["xs", "xml", "xmlns", "xsi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // The non-primary no-namespace document uses schema-none.xsd.
        if order.iter().skip(1).any(String::is_empty) {
            used.insert("none".into());
        }
        let mut k = 1;
        for ns in order {
            if ns.is_empty() || ns == XML_NS {
                continue;
            }
            let p = match self.prefixes.get(ns) {
                Some(p)
                    if !p.is_empty()
                        && crate::xml::chars::is_ncname(p)
                        && !used.contains(p) =>
                {
                    p.clone()
                }
                _ => loop {
                    let p = format!("ns{k}");
                    k += 1;
                    if !used.contains(&p) {
                        break p;
                    }
                },
            };
            used.insert(p.clone());
            prefix.insert(ns.clone(), p);
        }
        prefix
    }

    fn simple(&self, v: &Values) -> Simple {
        if v.count == 0 {
            return Simple::Builtin("string");
        }
        let exact = self.fit == Fit::Exact;
        if v.fits & BOOL != 0 {
            return Simple::Builtin("boolean");
        }
        if v.fits & INT != 0 {
            return match v.range {
                Some((lo, hi)) if exact => Simple::Range(
                    if lo >= 0 { "nonNegativeInteger" } else { "integer" },
                    lo,
                    hi,
                ),
                Some((lo, hi)) if lo >= 0 => {
                    Simple::Builtin(if hi <= u32::MAX as i64 {
                        "unsignedInt"
                    } else {
                        "unsignedLong"
                    })
                }
                Some((lo, hi)) => Simple::Builtin(
                    if lo >= i32::MIN as i64 && hi <= i32::MAX as i64 {
                        "int"
                    } else {
                        "long"
                    },
                ),
                None if !v.negative => Simple::Builtin("nonNegativeInteger"),
                None => Simple::Builtin("integer"),
            };
        }
        for (bit, name) in [
            (DECIMAL, "decimal"),
            (DOUBLE, "double"),
            (DATETIME, "dateTime"),
            (DATE, "date"),
            (TIME, "time"),
        ] {
            if v.fits & bit != 0 {
                return Simple::Builtin(name);
            }
        }
        if exact
            && !v.many
            && !v.distinct.is_empty()
            && v.count >= 2 * v.distinct.len() as u64
        {
            let trimmed = v.distinct.iter().all(|k| {
                !k.is_empty()
                    && k.trim_matches(|c| {
                        matches!(c, ' ' | '\t' | '\n' | '\r')
                    }) == k
            });
            if trimmed {
                return Simple::Enumeration(
                    v.distinct.iter().cloned().collect(),
                );
            }
        }
        Simple::Builtin("string")
    }

    /// The learned schema compiled: grammars for strict streams of
    /// documents like the examples.
    ///
    /// ```
    /// use exi::infer::SchemaInference;
    /// let mut learned = SchemaInference::new();
    /// learned.document(br#"<temperature xmlns="urn:sensor">21</temperature>"#)?;
    /// learned.document(br#"<alarm xmlns="urn:alarm">true</alarm>"#)?;
    /// let grammars = learned.grammars()?;
    /// assert!(grammars.name_id("urn:sensor", "temperature").is_some());
    /// assert!(grammars.name_id("urn:alarm", "alarm").is_some());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn grammars(&self) -> Result<GrammarSet> {
        let docs = self.schemas();
        if docs.is_empty() {
            return Err(Error::new(
                ErrorKind::State,
                "no document has been learned",
            ));
        }
        let mut c = XsdCompiler::new();
        for d in &docs {
            c = c.document(&d.location, d.text.as_bytes());
        }
        c.compile_documents(docs.iter().map(|d| d.location.as_str()))
    }
}

/// The child names in an order every instance follows, ties broken by
/// their order in `names`; `None` when the instances disagree.
fn topo(
    names: &[usize],
    before: &BTreeSet<(usize, usize)>,
) -> Option<Vec<usize>> {
    let mut left: Vec<usize> = names.to_vec();
    let mut out = Vec::with_capacity(names.len());
    while !left.is_empty() {
        let i = left.iter().position(|&n| {
            !left.iter().any(|&m| m != n && before.contains(&(m, n)))
        })?;
        out.push(left.remove(i));
    }
    Some(out)
}

/// A name made an NCName: other characters become `_`.
fn ncname(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if crate::xml::chars::is_name_char(c) && c != ':' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !out
        .chars()
        .next()
        .is_some_and(|c| crate::xml::chars::is_name_start(c) && c != ':')
    {
        out.insert(0, '_');
    }
    out
}

/// Text for an attribute value in double quotes.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '"' => o.push_str("&quot;"),
            '\t' => o.push_str("&#9;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            c => o.push(c),
        }
    }
    o
}

impl EventSink for SchemaInference {
    /// Learns from the events of a complete document.
    ///
    /// ```
    /// use exi::{Event, EventSink, infer::SchemaInference};
    /// let mut learned = SchemaInference::new();
    /// learned.event(&Event::StartDocument)?;
    /// learned.start_element("sensor")?;
    /// learned.attribute("id", 42_i64)?;
    /// learned.start_element("online")?;
    /// learned.text("true")?;
    /// learned.end_element()?;
    /// learned.end_element()?;
    /// learned.event(&Event::EndDocument)?;
    /// let text = &learned.schemas()[0].text;
    /// assert!(text.contains(r#"name="id" type="xs:unsignedInt" use="required""#));
    /// assert!(text.contains(r#"name="online" type="xs:boolean""#));
    /// # Ok::<(), exi::Error>(())
    /// ```
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        match *ev {
            Event::StartElement(n) => {
                let i = self.elem(n.uri(), n.local_name());
                self.elements[i].count += 1;
                match self.stack.last_mut() {
                    Some(p) => match p.runs.last_mut() {
                        Some((c, k)) if *c == i => *k += 1,
                        _ => p.runs.push((i, 1)),
                    },
                    None => {
                        if self.first_root.is_none() {
                            self.first_root = Some(n.uri().to_string());
                        }
                    }
                }
                if let Some(p) = n.prefix() {
                    self.prefixes
                        .entry(n.uri().to_string())
                        .or_insert_with(|| p.to_string());
                }
                self.stack.push(Frame {
                    elem: i,
                    runs: Vec::new(),
                    text: String::new(),
                    text_nonws: false,
                    nilled: false,
                    attrs: Vec::new(),
                });
            }
            Event::Namespace(d) => {
                if !d.prefix.is_empty() {
                    self.prefixes
                        .entry(d.uri.to_string())
                        .or_insert_with(|| d.prefix.to_string());
                }
            }
            Event::Attribute(n, v) => {
                if n.uri() == XSI {
                    if n.local_name() == "nil" {
                        let text = crate::engine::lexical_string(&v)?;
                        if matches!(lexical::parse_bool(&text), Some((true, _)))
                        {
                            let f = self.stack.last_mut().ok_or_else(|| {
                                Error::new(
                                    ErrorKind::State,
                                    "an attribute outside an element",
                                )
                            })?;
                            f.nilled = true;
                            self.elements[f.elem].nillable = true;
                        }
                    }
                    return Ok(());
                }
                let text = crate::engine::lexical_string(&v)?;
                let max_enum = self.max_enum;
                let f = self.stack.last_mut().ok_or_else(|| {
                    Error::new(
                        ErrorKind::State,
                        "an attribute outside an element",
                    )
                })?;
                let e = &mut self.elements[f.elem];
                let ai = match e.attrs.iter().position(|a| {
                    a.uri == n.uri() && a.local == n.local_name()
                }) {
                    Some(i) => i,
                    None => {
                        e.attrs.push(AttrStats {
                            uri: n.uri().into(),
                            local: n.local_name().into(),
                            ..AttrStats::default()
                        });
                        e.attrs.len() - 1
                    }
                };
                if !f.attrs.contains(&ai) {
                    f.attrs.push(ai);
                    e.attrs[ai].count += 1;
                }
                if n.uri().is_empty() {
                    e.attrs[ai].values.add(&text, max_enum);
                } else {
                    self.globals
                        .entry((n.uri().into(), n.local_name().into()))
                        .or_default()
                        .add(&text, max_enum);
                }
            }
            Event::Characters(v) => {
                let text = crate::engine::lexical_string(&v)?;
                if let Some(f) = self.stack.last_mut() {
                    if text
                        .bytes()
                        .any(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
                    {
                        f.text_nonws = true;
                    }
                    f.text.push_str(&text);
                }
            }
            Event::EndElement(_) => self.end()?,
            _ => {}
        }
        Ok(())
    }
}
