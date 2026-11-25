// src/xsd/mod.rs
//
// XML Schema compiler (EXI §8.5)
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! [`XsdCompiler`] reads XSD documents and builds a [`GrammarSet`] in
//! memory: the same tables `GrammarSet::from_exig` loads from an
//! `.exig` file and [`GrammarBuilder`] builds in code (A2). A set from
//! a schema is used directly, or saved with `GrammarSet::to_exig`.
//!
//! ```
//! use exi::grammar::GrammarSet;
//! let xsd = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
//!                          targetNamespace="urn:sensor" elementFormDefault="qualified">
//!   <xs:element name="reading">
//!     <xs:complexType>
//!       <xs:sequence><xs:element name="temp" type="xs:int"/></xs:sequence>
//!       <xs:attribute name="unit" type="xs:string" use="required"/>
//!     </xs:complexType>
//!   </xs:element>
//! </xs:schema>"#;
//! let set = GrammarSet::from_xsd(xsd)?;
//! assert!(set.is_schema_informed());
//! assert!(set.name_id("urn:sensor", "temp").is_some());
//! assert!(set.name_id("", "unit").is_some());
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # Documents
//!
//! A schema is often several documents. Give every document the
//! compiler may need with [`XsdCompiler::document`], each under the
//! location the others use for it; `xs:include` and `xs:import` find
//! documents by that location, resolved against the including document'
//! s own location like a relative URL. An `xs:import` without a
//! `schemaLocation`, or with one that matches no document, uses any
//! given document with that target namespace. Nothing is fetched: the
//! caller decides where documents come from.
//!
//! ```
//! use exi::xsd::XsdCompiler;
//! let main = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:t"
//!                          targetNamespace="urn:m">
//!   <xs:import namespace="urn:t" schemaLocation="types/t.xsd"/>
//!   <xs:element name="m" type="t:T"/>
//! </xs:schema>"#;
//! let types = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t">
//!   <xs:complexType name="T"><xs:attribute name="a"/></xs:complexType>
//! </xs:schema>"#;
//! let set = XsdCompiler::new()
//!     .document("schemas/main.xsd", main)
//!     .document("schemas/types/t.xsd", types)
//!     .compile("schemas/main.xsd")?;
//! assert!(set.name_id("urn:t", "T").is_some());
//!
//! // A missing document is an error that names nothing it cannot see.
//! let err = XsdCompiler::new().document("main.xsd", main).compile("main.xsd").unwrap_err();
//! assert_eq!(err.kind(), exi::ErrorKind::Schema);
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # What is derived
//!
//! | XSD | Grammars |
//! |---|---|
//! | Global element declarations | Globals of DocContent, in name order |
//! | Every element declaration | An element grammar, and a name in FragmentContent |
//! | Named types, built-in types included | Type grammars `xsi:type` can name |
//! | A type some named type derives from, or a union | Castable: strict adds `AT(xsi:type)` |
//! | `nillable="true"` | Strict adds `AT(xsi:nil)` |
//! | Attribute uses | Attribute states, sorted by local name then URI |
//! | `xs:anyAttribute` | `AT(*)` or `AT(uri:*)` in every attribute state |
//! | Sequences, choices, occurrences, `xs:any`, substitution groups | Content states: a deterministic automaton over the particles |
//! | `xs:all` | One state that accepts its elements in any order and number (EXI §8.5.4.1.6) |
//! | `mixed="true"` | `CH` in every content state |
//! | Simple types | Representations (table below) |
//!
//! | Simple type | Representation |
//! |---|---|
//! | `boolean` | Boolean; the 2-bit form with a `pattern` facet |
//! | `decimal` | Decimal |
//! | `integer` and types derived from it | n-bit when the facets bound it to 4096 values or fewer; unsigned when the lower bound is 0 or more; signed otherwise |
//! | `float`, `double` | Float |
//! | `dateTime`, `time`, `date`, `gYearMonth`, `gYear`, `gMonthDay`, `gDay`, `gMonth` | Date-time of that kind |
//! | `hexBinary`, `base64Binary` | Binary |
//! | A list | List of its item type |
//! | An `enumeration` facet | Enumeration, except on unions, `QName` and `NOTATION` (String), and lists (List) |
//! | A union, and every other type | String; with a restricted character set when a pattern of the type allows fewer than 256 characters ([`regex`]) |
//!
//! # Limits
//!
//! Content models with large `maxOccurs` values are unrolled, so a
//! schema can make large grammars. [`XsdCompiler::max_states`] caps the
//! states of one content model (`ErrorKind::Limit`). Not supported
//! (`ErrorKind::Unsupported`): `xs:redefine`, `xs:override` and other
//! XSD 1.1 constructs.
//!
//! [`GrammarBuilder`]: crate::grammar::informed::GrammarBuilder

pub mod regex;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::grammar::GrammarSet;
use crate::grammar::informed::{
    ElementId, GrammarBuilder, IntegerRep, Prod, Representation, SimpleTypeId,
    TypeId,
};
use crate::strings::{GlobalConfig, GlobalStrings};
use crate::value::{BinaryKind, DateTimeKind};
use crate::xml::{XmlConfig, XmlReader};
use crate::{Error, ErrorKind, Event, Limits, OwnedName, Result, Step};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema_err(detail: &'static str) -> Error {
    Error::new(ErrorKind::Schema, detail)
}

fn unsupported(detail: &'static str) -> Error {
    Error::new(ErrorKind::Unsupported, detail)
}

impl GrammarSet {
    /// Compiles one XSD document that includes or imports nothing. See
    /// [`XsdCompiler`] for several documents.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Xml` when the document is not well-formed;
    /// `ErrorKind::Schema` when it is not a schema this compiler
    /// understands; `ErrorKind::Unsupported` for XSD features listed in
    /// the module documentation.
    pub fn from_xsd(xsd: &[u8]) -> Result<GrammarSet> {
        XsdCompiler::new().document("schema.xsd", xsd).compile("schema.xsd")
    }
}

/// Compiles XSD documents into a [`GrammarSet`]. See the module
/// documentation.
pub struct XsdCompiler<'a> {
    docs: Vec<(String, &'a [u8])>,
    limits: Limits,
    max_states: usize,
}

impl Default for XsdCompiler<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> XsdCompiler<'a> {
    pub fn new() -> Self {
        XsdCompiler {
            docs: Vec::new(),
            limits: Limits::new(),
            max_states: 65536,
        }
    }

    /// A document the schema may use, under the location includes and
    /// imports name it by.
    pub fn document(mut self, location: &str, xsd: &'a [u8]) -> Self {
        self.docs.push((normalize_path(location), xsd));
        self
    }

    /// Limits for reading the documents as XML.
    pub fn limits(mut self, l: Limits) -> Self {
        self.limits = l;
        self
    }

    /// The most states one content model may unroll to. Default 65536.
    ///
    /// ```
    /// use exi::xsd::XsdCompiler;
    /// let xsd = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
    ///   <xs:element name="r"><xs:complexType><xs:sequence>
    ///     <xs:element name="a" maxOccurs="100"/>
    ///   </xs:sequence></xs:complexType></xs:element>
    /// </xs:schema>"#;
    /// assert!(XsdCompiler::new().document("s.xsd", xsd).compile("s.xsd").is_ok());
    /// let err = XsdCompiler::new().max_states(50).document("s.xsd", xsd).compile("s.xsd").unwrap_err();
    /// assert_eq!(err.kind(), exi::ErrorKind::Limit);
    /// ```
    pub fn max_states(mut self, n: usize) -> Self {
        self.max_states = n;
        self
    }

    /// Reads `main` and every document it includes or imports, and
    /// builds the grammars.
    pub fn compile(self, main: &str) -> Result<GrammarSet> {
        let mut schema = Schema::default();
        schema.load(&self, &normalize_path(main), None)?;
        schema.index()?;
        let mut c = Compiler::new(&schema, self.max_states);
        c.run()?;
        c.b.build()
    }
}

// =======================================================================================
// Reading documents
// =======================================================================================

/// An element of a schema document, with the namespace bindings in
/// scope.
#[derive(Debug, Default)]
struct Node {
    uri: String,
    local: String,
    attrs: Vec<(String, String)>,
    ns: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Node {
    fn is(&self, local: &str) -> bool {
        self.uri == XS && self.local == local
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// The XSD children, without annotations.
    fn xs(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(|c| c.uri == XS && c.local != "annotation")
    }

    fn child(&self, local: &str) -> Option<&Node> {
        self.xs().find(|c| c.local == local)
    }

    /// A QName attribute resolved with the bindings in scope.
    fn qname(&self, name: &str) -> Result<Option<(String, String)>> {
        let Some(v) = self.attr(name) else { return Ok(None) };
        let v = v.trim();
        let (p, l) = v.split_once(':').unwrap_or(("", v));
        let uri = if p == "xml" {
            "http://www.w3.org/XML/1998/namespace".to_string()
        } else {
            match self.ns.iter().rev().find(|(q, _)| q == p) {
                Some((_, u)) => u.clone(),
                None if p.is_empty() => String::new(),
                None => {
                    return Err(schema_err(
                        "an undeclared prefix in a QName attribute",
                    ));
                }
            }
        };
        Ok(Some((uri, l.to_string())))
    }

    fn occurs(&self) -> Result<(u64, Option<u64>)> {
        let min = match self.attr("minOccurs") {
            Some(v) => {
                v.trim().parse().map_err(|_| schema_err("minOccurs"))?
            }
            None => 1,
        };
        let max = match self.attr("maxOccurs").map(str::trim) {
            Some("unbounded") => None,
            Some(v) => Some(v.parse().map_err(|_| schema_err("maxOccurs"))?),
            None => Some(1),
        };
        Ok((min, max))
    }
}

fn parse(bytes: &[u8], limits: &Limits) -> Result<Node> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings)?;
    let mut r: XmlReader = XmlReader::new(
        XmlConfig::new()
            .limits(*limits)
            .report_comments(false)
            .report_pis(false),
        &g,
    );
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut rest = bytes;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next()? {
                Step::Event(ev, _) => match ev {
                    Event::StartElement(n) => {
                        let ns = stack
                            .last()
                            .map(|p| p.ns.clone())
                            .unwrap_or_default();
                        stack.push(Node {
                            uri: n.uri().into(),
                            local: n.local_name().into(),
                            ns,
                            ..Node::default()
                        });
                    }
                    Event::Namespace(d) => {
                        if let Some(top) = stack.last_mut() {
                            top.ns.push((d.prefix.into(), d.uri.into()));
                        }
                    }
                    Event::Attribute(n, v) => {
                        if let Some(top) = stack.last_mut() {
                            // Schema attributes are in no namespace;
                            // others are kept by local name.
                            if n.uri().is_empty() {
                                top.attrs.push((
                                    n.local_name().into(),
                                    v.as_str().unwrap_or("").into(),
                                ));
                            }
                        }
                    }
                    Event::EndElement(_) => {
                        let done = stack.pop().ok_or_else(|| {
                            schema_err("unbalanced document")
                        })?;
                        match stack.last_mut() {
                            Some(p) => p.children.push(done),
                            None => root = Some(done),
                        }
                    }
                    _ => {}
                },
                Step::NeedInput => break,
                Step::End => {
                    return root
                        .ok_or_else(|| schema_err("no document element"));
                }
            }
        }
    }
}

/// `a/./b/../c` to `a/c`.
fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "." => {}
            ".." if out.last().is_some_and(|s| *s != "..") => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

fn resolve(base: &str, rel: &str) -> String {
    if rel.contains("://") || rel.starts_with('/') {
        return normalize_path(rel);
    }
    match base.rfind('/') {
        Some(i) => normalize_path(&format!("{}/{}", &base[..i], rel)),
        None => normalize_path(rel),
    }
}

// =======================================================================================
// The schema: documents and their components
// =======================================================================================

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Element,
    Attribute,
    Type,
    Group,
    AttributeGroup,
}

/// One document, read with an effective target namespace (a chameleon
/// include takes the includer's).
struct Doc {
    location: String,
    tns: String,
    elem_qualified: bool,
    attr_qualified: bool,
    root: Node,
}

/// A global component: its kind, URI and local name.
type Component = (Kind, String, String);

#[derive(Default)]
struct Schema {
    docs: Vec<Doc>,
    /// Component → document and child index.
    index: BTreeMap<Component, (usize, usize)>,
}

impl Schema {
    fn load(
        &mut self,
        src: &XsdCompiler<'_>,
        location: &str,
        tns: Option<&str>,
    ) -> Result<()> {
        if self
            .docs
            .iter()
            .any(|d| d.location == location && tns.is_none_or(|t| d.tns == t))
        {
            return Ok(());
        }
        let bytes = src.docs.iter().find(|(l, _)| l == location).map(|(_, b)| *b).ok_or_else(|| schema_err("an included or imported document was not given to the compiler"))?;
        let root = parse(bytes, &src.limits)?;
        if !root.is("schema") {
            return Err(schema_err("a document whose root is not xs:schema"));
        }
        let own = root.attr("targetNamespace").unwrap_or("").to_string();
        if tns.is_some_and(|t| !own.is_empty() && own != t) {
            return Err(schema_err(
                "an included document with another target namespace",
            ));
        }
        let tns =
            if own.is_empty() { tns.unwrap_or("").to_string() } else { own };
        let doc = Doc {
            location: location.into(),
            elem_qualified: root.attr("elementFormDefault")
                == Some("qualified"),
            attr_qualified: root.attr("attributeFormDefault")
                == Some("qualified"),
            tns: tns.clone(),
            root,
        };
        let mut refs = Vec::new();
        for c in doc.root.xs() {
            match c.local.as_str() {
                "include" => refs.push((
                    resolve(location, c.attr("schemaLocation").unwrap_or("")),
                    Some(tns.clone()),
                    None,
                )),
                "import" => {
                    let ns = c.attr("namespace").unwrap_or("").to_string();
                    let loc =
                        c.attr("schemaLocation").map(|l| resolve(location, l));
                    refs.push((loc.unwrap_or_default(), None, Some(ns)));
                }
                "redefine" | "override" => {
                    return Err(unsupported("xs:redefine and xs:override"));
                }
                _ => {}
            }
        }
        self.docs.push(doc);
        for (loc, include_tns, import_ns) in refs {
            match import_ns {
                None => self.load(src, &loc, include_tns.as_deref())?,
                Some(ns) => {
                    if src.docs.iter().any(|(l, _)| *l == loc) {
                        self.load(src, &loc, None)?;
                    } else if self.docs.iter().any(|d| d.tns == ns) {
                        // already read
                    } else {
                        // Any given document with that target
                        // namespace.
                        let found = src
                            .docs
                            .iter()
                            .map(|(l, b)| (l.clone(), *b))
                            .find(|(_, b)| {
                                parse(b, &src.limits).ok().is_some_and(|r| {
                                    r.attr("targetNamespace").unwrap_or("")
                                        == ns
                                })
                            });
                        // An import nothing refers to is harmless:
                        // unresolved references fail later.
                        if let Some((l, _)) = found {
                            self.load(src, &l, None)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn index(&mut self) -> Result<()> {
        for (d, doc) in self.docs.iter().enumerate() {
            for (i, c) in doc.root.children.iter().enumerate() {
                if c.uri != XS {
                    continue;
                }
                let kind = match c.local.as_str() {
                    "element" => Kind::Element,
                    "attribute" => Kind::Attribute,
                    "complexType" | "simpleType" => Kind::Type,
                    "group" => Kind::Group,
                    "attributeGroup" => Kind::AttributeGroup,
                    _ => continue,
                };
                let name = c.attr("name").ok_or_else(|| {
                    schema_err("a global component without a name")
                })?;
                // The first declaration wins; repeats of the same
                // document are harmless.
                self.index
                    .entry((kind, doc.tns.clone(), name.to_string()))
                    .or_insert((d, i));
            }
        }
        Ok(())
    }

    fn get(
        &self,
        kind: Kind,
        uri: &str,
        local: &str,
    ) -> Option<(usize, &Node)> {
        self.index
            .get(&(kind, uri.to_string(), local.to_string()))
            .map(|&(d, i)| (d, &self.docs[d].root.children[i]))
    }
}

// =======================================================================================
// Built-in types
// =======================================================================================

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Prim {
    String,
    Boolean,
    Decimal,
    Integer,
    Float,
    Binary(BinaryKind),
    DateTime(DateTimeKind),
    /// QName and NOTATION: strings that are never enumerations.
    QName,
    /// Lists and unions have no primitive.
    None,
}

/// Name, base, primitive, integer bounds, list item.
type Builtin = (
    &'static str,
    &'static str,
    Prim,
    Option<i128>,
    Option<i128>,
    Option<&'static str>,
);

const BUILTINS: &[Builtin] = &[
    ("anySimpleType", "anyType", Prim::String, None, None, None),
    ("string", "anySimpleType", Prim::String, None, None, None),
    ("boolean", "anySimpleType", Prim::Boolean, None, None, None),
    ("decimal", "anySimpleType", Prim::Decimal, None, None, None),
    ("float", "anySimpleType", Prim::Float, None, None, None),
    ("double", "anySimpleType", Prim::Float, None, None, None),
    ("duration", "anySimpleType", Prim::String, None, None, None),
    (
        "dateTime",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::DateTime),
        None,
        None,
        None,
    ),
    (
        "time",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::Time),
        None,
        None,
        None,
    ),
    (
        "date",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::Date),
        None,
        None,
        None,
    ),
    (
        "gYearMonth",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::GYearMonth),
        None,
        None,
        None,
    ),
    (
        "gYear",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::GYear),
        None,
        None,
        None,
    ),
    (
        "gMonthDay",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::GMonthDay),
        None,
        None,
        None,
    ),
    (
        "gDay",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::GDay),
        None,
        None,
        None,
    ),
    (
        "gMonth",
        "anySimpleType",
        Prim::DateTime(DateTimeKind::GMonth),
        None,
        None,
        None,
    ),
    (
        "hexBinary",
        "anySimpleType",
        Prim::Binary(BinaryKind::Hex),
        None,
        None,
        None,
    ),
    (
        "base64Binary",
        "anySimpleType",
        Prim::Binary(BinaryKind::Base64),
        None,
        None,
        None,
    ),
    ("anyURI", "anySimpleType", Prim::String, None, None, None),
    ("QName", "anySimpleType", Prim::QName, None, None, None),
    ("NOTATION", "anySimpleType", Prim::QName, None, None, None),
    ("normalizedString", "string", Prim::String, None, None, None),
    ("token", "normalizedString", Prim::String, None, None, None),
    ("language", "token", Prim::String, None, None, None),
    ("Name", "token", Prim::String, None, None, None),
    ("NCName", "Name", Prim::String, None, None, None),
    ("ID", "NCName", Prim::String, None, None, None),
    ("IDREF", "NCName", Prim::String, None, None, None),
    ("ENTITY", "NCName", Prim::String, None, None, None),
    ("NMTOKEN", "token", Prim::String, None, None, None),
    ("IDREFS", "anySimpleType", Prim::None, None, None, Some("IDREF")),
    ("ENTITIES", "anySimpleType", Prim::None, None, None, Some("ENTITY")),
    ("NMTOKENS", "anySimpleType", Prim::None, None, None, Some("NMTOKEN")),
    ("integer", "decimal", Prim::Integer, None, None, None),
    ("nonPositiveInteger", "integer", Prim::Integer, None, Some(0), None),
    (
        "negativeInteger",
        "nonPositiveInteger",
        Prim::Integer,
        None,
        Some(-1),
        None,
    ),
    (
        "long",
        "integer",
        Prim::Integer,
        Some(i64::MIN as i128),
        Some(i64::MAX as i128),
        None,
    ),
    (
        "int",
        "long",
        Prim::Integer,
        Some(i32::MIN as i128),
        Some(i32::MAX as i128),
        None,
    ),
    (
        "short",
        "int",
        Prim::Integer,
        Some(i16::MIN as i128),
        Some(i16::MAX as i128),
        None,
    ),
    (
        "byte",
        "short",
        Prim::Integer,
        Some(i8::MIN as i128),
        Some(i8::MAX as i128),
        None,
    ),
    ("nonNegativeInteger", "integer", Prim::Integer, Some(0), None, None),
    (
        "unsignedLong",
        "nonNegativeInteger",
        Prim::Integer,
        Some(0),
        Some(u64::MAX as i128),
        None,
    ),
    (
        "unsignedInt",
        "unsignedLong",
        Prim::Integer,
        Some(0),
        Some(u32::MAX as i128),
        None,
    ),
    (
        "unsignedShort",
        "unsignedInt",
        Prim::Integer,
        Some(0),
        Some(u16::MAX as i128),
        None,
    ),
    (
        "unsignedByte",
        "unsignedShort",
        Prim::Integer,
        Some(0),
        Some(u8::MAX as i128),
        None,
    ),
    (
        "positiveInteger",
        "nonNegativeInteger",
        Prim::Integer,
        Some(1),
        None,
        None,
    ),
];

// =======================================================================================
// Compiling
// =======================================================================================

/// What a simple type is, beyond its representation: what derived types
/// need.
#[derive(Clone, Debug)]
struct SInfo {
    prim: Prim,
    union: bool,
    min: Option<i128>,
    max: Option<i128>,
    /// The patterns of the nearest type with patterns of its own,
    /// OR-ed.
    patterns: Option<Vec<String>>,
    /// For an enumeration: the type its values are of.
    enum_base: Option<SimpleTypeId>,
    rep: Representation,
}

/// Where a type comes from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TypeKey {
    Named(String, String),
    /// An anonymous type: its node's address (nodes never move once
    /// read) and document.
    Anon(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Wild {
    Any,
    /// URIs, sorted.
    List(Vec<String>),
}

#[derive(Clone, Debug)]
struct AttrUse {
    name: OwnedName,
    ty: SimpleTypeId,
    required: bool,
}

#[derive(Clone, Debug)]
enum Term {
    /// The element and the ones its substitution group adds, sorted by
    /// name.
    Elem(Vec<(ElementId, OwnedName)>),
    Any(Wild),
    Seq(Vec<Particle>),
    Choice(Vec<Particle>),
    All(Vec<Particle>),
}

#[derive(Clone, Debug)]
struct Particle {
    term: Term,
    min: u64,
    max: Option<u64>,
}

#[derive(Clone, Debug)]
enum Content {
    Empty,
    Simple(SimpleTypeId),
    Model(Particle),
}

/// A type's parts, before states are made.
#[derive(Clone, Debug)]
struct Parts {
    attrs: Vec<AttrUse>,
    wild: Option<Wild>,
    content: Content,
    mixed: bool,
}

struct Compiler<'s> {
    s: &'s Schema,
    b: GrammarBuilder,
    max_states: usize,
    /// Indexed like the builder's simple types.
    simple: Vec<SInfo>,
    simple_named: BTreeMap<(String, String), SimpleTypeId>,
    simple_anon: BTreeMap<usize, SimpleTypeId>,
    types: BTreeMap<TypeKey, TypeId>,
    pending: Vec<(TypeKey, TypeId)>,
    parts: BTreeMap<TypeKey, Parts>,
    globals: BTreeMap<(String, String), ElementId>,
    locals: BTreeMap<usize, (ElementId, OwnedName)>,
    string: SimpleTypeId,
    any_type: Option<TypeId>,
}

fn key_of(n: &Node) -> usize {
    n as *const Node as usize
}

fn plain_string() -> SInfo {
    SInfo {
        prim: Prim::String,
        union: false,
        min: None,
        max: None,
        patterns: None,
        enum_base: None,
        rep: Representation::String { charset: None },
    }
}

impl<'s> Compiler<'s> {
    fn new(s: &'s Schema, max_states: usize) -> Self {
        let mut b = GrammarSet::builder();
        let string =
            b.simple_type(None, Representation::String { charset: None });
        Compiler {
            s,
            b,
            max_states,
            simple: alloc::vec![plain_string()],
            simple_named: BTreeMap::new(),
            simple_anon: BTreeMap::new(),
            types: BTreeMap::new(),
            pending: Vec::new(),
            parts: BTreeMap::new(),
            globals: BTreeMap::new(),
            locals: BTreeMap::new(),
            string,
            any_type: None,
        }
    }

    fn run(&mut self) -> Result<()> {
        // Built-in types: a simple type and a type grammar each,
        // derived as XSD derives them.
        let any =
            self.type_of(&TypeKey::Named(XS.into(), "anyType".into()))?;
        self.any_type = Some(any);
        for (name, base, ..) in BUILTINS {
            let t =
                self.type_of(&TypeKey::Named(XS.into(), (*name).into()))?;
            let bt =
                self.type_of(&TypeKey::Named(XS.into(), (*base).into()))?;
            self.b.derives(t, bt);
        }
        // Every global of every document.
        let entries: Vec<(Component, (usize, usize))> =
            self.s.index.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for ((kind, uri, local), _) in &entries {
            match kind {
                Kind::Element => {
                    let e = self.global_element(uri, local)?;
                    self.b.global(e);
                }
                Kind::Type => {
                    self.type_of(&TypeKey::Named(uri.clone(), local.clone()))?;
                }
                Kind::Attribute => self.b.name(OwnedName::new(uri, local)),
                _ => {}
            }
        }
        for d in &self.s.docs {
            self.b.uri(&d.tns);
        }
        // Derivations among named user types: what makes a type
        // castable.
        for ((kind, uri, local), (d, i)) in &entries {
            if *kind != Kind::Type {
                continue;
            }
            let node = &self.s.docs[*d].root.children[*i];
            let t =
                self.type_of(&TypeKey::Named(uri.clone(), local.clone()))?;
            let base = match self.base_of(node)? {
                Some((bu, bl)) => self.type_of(&TypeKey::Named(bu, bl))?,
                None => any,
            };
            self.b.derives(t, base);
            if node.is("simpleType") && node.child("union").is_some() {
                self.b.castable(t);
            }
        }
        while let Some((k, t)) = self.pending.pop() {
            let states = self.states_of(&k)?;
            self.b.set_states(t, states);
        }
        Ok(())
    }

    /// The named base a type definition derives from; lists and unions
    /// derive from anySimpleType; a complex type with neither content
    /// element derives from anyType (`None`).
    fn base_of(&self, node: &Node) -> Result<Option<(String, String)>> {
        if node.is("simpleType") {
            if let Some(r) = node.child("restriction") {
                if let Some(q) = r.qname("base")? {
                    return Ok(Some(q));
                }
            }
            return Ok(Some((XS.into(), "anySimpleType".into())));
        }
        for c in ["simpleContent", "complexContent"] {
            if let Some(cc) = node.child(c) {
                for d in ["extension", "restriction"] {
                    if let Some(x) = cc.child(d) {
                        return x.qname("base");
                    }
                }
            }
        }
        Ok(None)
    }

    // ----- simple types -----

    fn push_simple(
        &mut self,
        name: Option<OwnedName>,
        info: SInfo,
    ) -> SimpleTypeId {
        let id = self.b.simple_type(name, info.rep.clone());
        self.simple.push(info);
        id
    }

    fn info(&self, t: SimpleTypeId) -> &SInfo {
        &self.simple[t.0 as usize]
    }

    /// The simple type a QName names.
    fn simple_named(
        &mut self,
        uri: &str,
        local: &str,
    ) -> Result<SimpleTypeId> {
        if let Some(t) = self.simple_named.get(&(uri.into(), local.into())) {
            return Ok(*t);
        }
        let t = if uri == XS {
            let &(name, _, prim, min, max, list) =
                BUILTINS.iter().find(|b| b.0 == local).ok_or_else(|| {
                    schema_err("an unknown built-in simple type")
                })?;
            let mut info = SInfo { prim, min, max, ..plain_string() };
            info.rep = match list {
                Some(item) => {
                    Representation::List(self.simple_named(XS, item)?)
                }
                None => self.rep_for(&info),
            };
            self.push_simple(Some(OwnedName::new(XS, name)), info)
        } else {
            let (d, node) =
                self.s.get(Kind::Type, uri, local).ok_or_else(|| {
                    schema_err("a reference to an undeclared type")
                })?;
            if !node.is("simpleType") {
                return Err(schema_err(
                    "a complex type where a simple type is needed",
                ));
            }
            let info = self.simple_info(d, node)?;
            self.push_simple(Some(OwnedName::new(uri, local)), info)
        };
        self.simple_named.insert((uri.into(), local.into()), t);
        Ok(t)
    }

    /// The simple type of an `xs:simpleType` node.
    fn simple_node(&mut self, d: usize, node: &Node) -> Result<SimpleTypeId> {
        if let Some(t) = self.simple_anon.get(&key_of(node)) {
            return Ok(*t);
        }
        let info = self.simple_info(d, node)?;
        let t = self.push_simple(None, info);
        self.simple_anon.insert(key_of(node), t);
        Ok(t)
    }

    /// A type attribute or an inline `xs:simpleType` child; `None` when
    /// neither is there.
    fn simple_ref(
        &mut self,
        d: usize,
        node: &Node,
        attr: &str,
    ) -> Result<Option<SimpleTypeId>> {
        if let Some((u, l)) = node.qname(attr)? {
            return Ok(Some(self.simple_named(&u, &l)?));
        }
        match node.child("simpleType") {
            Some(st) => Ok(Some(self.simple_node(d, st)?)),
            None => Ok(None),
        }
    }

    fn simple_info(&mut self, d: usize, node: &Node) -> Result<SInfo> {
        if let Some(l) = node.child("list") {
            let item = self
                .simple_ref(d, l, "itemType")?
                .ok_or_else(|| schema_err("xs:list without an item type"))?;
            return Ok(SInfo {
                prim: Prim::None,
                rep: Representation::List(item),
                ..plain_string()
            });
        }
        if node.child("union").is_some() {
            return Ok(SInfo {
                prim: Prim::None,
                union: true,
                ..plain_string()
            });
        }
        let r = node.child("restriction").ok_or_else(|| {
            schema_err("xs:simpleType without restriction, list or union")
        })?;
        let base = self
            .simple_ref(d, r, "base")?
            .ok_or_else(|| schema_err("xs:restriction without a base"))?;
        self.restrict(base, r)
    }

    /// `base` with the facets of a restriction node.
    fn restrict(&mut self, base: SimpleTypeId, r: &Node) -> Result<SInfo> {
        let mut info = self.info(base).clone();
        let facet = |n: &str| {
            r.child(n)
                .and_then(|f| f.attr("value"))
                .map(|v| v.trim().trim_start_matches('+').to_string())
        };
        let int = |v: Option<String>, adj: i128| {
            v.and_then(|s| s.parse::<i128>().ok()).map(|x| x + adj)
        };
        if info.prim == Prim::Integer {
            if let Some(v) =
                int(facet("minInclusive"), 0).or(int(facet("minExclusive"), 1))
            {
                info.min = Some(info.min.map_or(v, |m| m.max(v)));
            }
            if let Some(v) = int(facet("maxInclusive"), 0)
                .or(int(facet("maxExclusive"), -1))
            {
                info.max = Some(info.max.map_or(v, |m| m.min(v)));
            }
        }
        let patterns: Vec<String> = r
            .xs()
            .filter(|c| c.local == "pattern")
            .filter_map(|c| c.attr("value"))
            .map(String::from)
            .collect();
        if !patterns.is_empty() {
            info.patterns = Some(patterns);
        }
        let values: Vec<Box<str>> = r
            .xs()
            .filter(|c| c.local == "enumeration")
            .filter_map(|c| c.attr("value"))
            .map(Into::into)
            .collect();
        if !values.is_empty() {
            // The values are of the nearest base that is not an
            // enumeration.
            let eb = self.info(base).enum_base.unwrap_or(base);
            let ebi = self.info(eb);
            info.rep = if ebi.union || ebi.prim == Prim::QName {
                Representation::String { charset: None }
            } else if let Representation::List(item) = ebi.rep {
                Representation::List(item)
            } else {
                Representation::Enumeration { values: values.into(), base: eb }
            };
            info.enum_base = Some(eb);
        } else if info.enum_base.is_none() {
            info.rep = self.rep_for(&info);
        }
        Ok(info)
    }

    /// The representation of an atomic or union type from its facts
    /// (table in the module documentation). Lists are made where their
    /// item is known.
    fn rep_for(&self, info: &SInfo) -> Representation {
        if let Representation::List(item) = info.rep {
            return Representation::List(item);
        }
        if info.union {
            return Representation::String { charset: None };
        }
        match info.prim {
            Prim::Boolean => {
                Representation::Boolean { pattern: info.patterns.is_some() }
            }
            Prim::Decimal => Representation::Decimal,
            Prim::Float => Representation::Float,
            Prim::Binary(k) => Representation::Binary(k),
            Prim::DateTime(k) => Representation::DateTime(k),
            Prim::Integer => match (info.min, info.max) {
                (Some(lo), Some(hi))
                    if hi >= lo
                        && hi - lo < 4096
                        && lo >= i64::MIN as i128
                        && hi <= i64::MAX as i128 =>
                {
                    Representation::Integer(IntegerRep::Bounded {
                        min: lo as i64,
                        max: hi as i64,
                    })
                }
                (Some(lo), _) if lo >= 0 => {
                    Representation::Integer(IntegerRep::Unsigned)
                }
                _ => Representation::Integer(IntegerRep::Signed),
            },
            Prim::String => {
                let charset = info.patterns.as_ref().and_then(|ps| {
                    let mut all = BTreeSet::new();
                    for p in ps {
                        all.extend(regex::char_set(p)?);
                    }
                    (all.len() <= 255).then(|| {
                        all.into_iter()
                            .collect::<Vec<char>>()
                            .into_boxed_slice()
                    })
                });
                Representation::String { charset }
            }
            Prim::QName | Prim::None => {
                Representation::String { charset: None }
            }
        }
    }

    // ----- types -----

    /// The type grammar of a key, reserved now and built later.
    fn type_of(&mut self, k: &TypeKey) -> Result<TypeId> {
        if let Some(t) = self.types.get(k) {
            return Ok(*t);
        }
        let name = match k {
            TypeKey::Named(u, l) => {
                if u != XS && self.s.get(Kind::Type, u, l).is_none() {
                    return Err(schema_err(
                        "a reference to an undeclared type",
                    ));
                }
                if u == XS
                    && l != "anyType"
                    && !BUILTINS.iter().any(|b| b.0 == l)
                {
                    return Err(schema_err("an unknown built-in type"));
                }
                Some(OwnedName::new(u, l))
            }
            TypeKey::Anon(..) => None,
        };
        let t = self.b.reserve_type(name);
        self.types.insert(k.clone(), t);
        self.pending.push((k.clone(), t));
        Ok(t)
    }

    /// The node and document of a user type.
    fn type_node(&self, k: &TypeKey) -> Option<(usize, &'s Node)> {
        match k {
            TypeKey::Named(u, l) => self.s.get(Kind::Type, u, l),
            TypeKey::Anon(addr, d) => {
                // SAFETY-free lookup: walk the document for the node
                // with this address.
                fn find(n: &Node, addr: usize) -> Option<&Node> {
                    if key_of(n) == addr {
                        return Some(n);
                    }
                    n.children.iter().find_map(|c| find(c, addr))
                }
                find(&self.s.docs[*d].root, *addr).map(|n| (*d, n))
            }
        }
    }

    fn states_of(&mut self, k: &TypeKey) -> Result<Vec<Vec<Prod>>> {
        if let TypeKey::Named(u, l) = k {
            if u == XS {
                if l == "anyType" {
                    return Ok(self.ur_states());
                }
                let st = self.simple_named(XS, l)?;
                return Ok(simple_states(st));
            }
        }
        let (d, node) = self
            .type_node(k)
            .ok_or_else(|| schema_err("a reference to an undeclared type"))?;
        if node.is("simpleType") {
            let st = match k {
                TypeKey::Named(u, l) => self.simple_named(u, l)?,
                TypeKey::Anon(..) => self.simple_node(d, node)?,
            };
            return Ok(simple_states(st));
        }
        let parts = self.parts_of(k)?;
        self.assemble(&parts)
    }

    /// anyType (EXI §8.5.4.1.3.3): any attributes, then any content,
    /// mixed.
    fn ur_states(&self) -> Vec<Vec<Prod>> {
        let s = self.string;
        alloc::vec![
            alloc::vec![
                Prod::AtAny { next: 0 },
                Prod::SeAny { next: 1 },
                Prod::EE,
                Prod::Ch { ty: s, next: 1 }
            ],
            alloc::vec![
                Prod::SeAny { next: 1 },
                Prod::EE,
                Prod::Ch { ty: s, next: 1 }
            ],
        ]
    }

    /// The parts of a complex type, with what it derives from merged
    /// in.
    fn parts_of(&mut self, k: &TypeKey) -> Result<Parts> {
        if let Some(p) = self.parts.get(k) {
            return Ok(p.clone());
        }
        if *k == TypeKey::Named(XS.into(), "anyType".into()) {
            return Ok(Parts {
                attrs: Vec::new(),
                wild: Some(Wild::Any),
                content: Content::Empty,
                mixed: true,
            });
        }
        let (d, node) = self
            .type_node(k)
            .ok_or_else(|| schema_err("a reference to an undeclared type"))?;
        if !node.is("complexType") {
            return Err(schema_err(
                "a simple type where a complex type is needed",
            ));
        }
        let mut mixed = node.attr("mixed") == Some("true");
        let p = if let Some(sc) = node.child("simpleContent") {
            let (x, ext) = match (
                sc.child("extension"),
                sc.child("restriction"),
            ) {
                (Some(x), _) => (x, true),
                (None, Some(x)) => (x, false),
                _ => {
                    return Err(schema_err(
                        "xs:simpleContent without extension or restriction",
                    ));
                }
            };
            let (bu, bl) = x
                .qname("base")?
                .ok_or_else(|| schema_err("a derivation without a base"))?;
            let base_key = TypeKey::Named(bu.clone(), bl.clone());
            let base_simple = bu == XS && bl != "anyType"
                || self
                    .s
                    .get(Kind::Type, &bu, &bl)
                    .is_some_and(|(_, n)| n.is("simpleType"));
            let (mut attrs, mut wild, base_st) = if base_simple {
                (Vec::new(), None, self.simple_named(&bu, &bl)?)
            } else {
                let bp = self.parts_of(&base_key)?;
                let st = match bp.content {
                    Content::Simple(st) => st,
                    _ => self.string,
                };
                (bp.attrs, bp.wild, st)
            };
            let st = if ext {
                base_st
            } else {
                // A restriction: the base's simple type with this
                // node's facets, or an inline simple type.
                match x.child("simpleType") {
                    Some(inner) => self.simple_node(d, inner)?,
                    None => {
                        let info = self.restrict(base_st, x)?;
                        self.push_simple(None, info)
                    }
                }
            };
            self.attributes(d, x, &mut attrs, &mut wild, ext)?;
            Parts { attrs, wild, content: Content::Simple(st), mixed: false }
        } else if let Some(cc) = node.child("complexContent") {
            if let Some(m) = cc.attr("mixed") {
                mixed = m == "true";
            }
            let (x, ext) = match (
                cc.child("extension"),
                cc.child("restriction"),
            ) {
                (Some(x), _) => (x, true),
                (None, Some(x)) => (x, false),
                _ => {
                    return Err(schema_err(
                        "xs:complexContent without extension or restriction",
                    ));
                }
            };
            let (bu, bl) = x
                .qname("base")?
                .ok_or_else(|| schema_err("a derivation without a base"))?;
            let base_any = bu == XS && bl == "anyType";
            let bp = if base_any {
                Parts {
                    attrs: Vec::new(),
                    wild: None,
                    content: Content::Empty,
                    mixed: false,
                }
            } else {
                self.parts_of(&TypeKey::Named(bu, bl))?
            };
            let own = self.model(d, x)?;
            let content = if ext {
                match (bp.content, own) {
                    (Content::Model(b), Some(o)) => Content::Model(Particle {
                        term: Term::Seq(alloc::vec![b, o]),
                        min: 1,
                        max: Some(1),
                    }),
                    (Content::Model(b), None) => Content::Model(b),
                    (_, Some(o)) => Content::Model(o),
                    (c, None) => c,
                }
            } else {
                own.map_or(Content::Empty, Content::Model)
            };
            let (mut attrs, mut wild) = (bp.attrs, bp.wild);
            if !ext {
                wild = None;
            }
            self.attributes(d, x, &mut attrs, &mut wild, ext)?;
            Parts { attrs, wild, content, mixed }
        } else {
            let mut attrs = Vec::new();
            let mut wild = None;
            let content =
                self.model(d, node)?.map_or(Content::Empty, Content::Model);
            self.attributes(d, node, &mut attrs, &mut wild, true)?;
            Parts { attrs, wild, content, mixed }
        };
        self.parts.insert(k.clone(), p.clone());
        Ok(p)
    }

    /// Adds the attribute uses, groups and wildcard of `node` to
    /// `attrs` and `wild`. In a restriction (`ext` false) a use
    /// replaces the base's of the same name, and `use="prohibited"`
    /// removes it.
    fn attributes(
        &mut self,
        d: usize,
        node: &Node,
        attrs: &mut Vec<AttrUse>,
        wild: &mut Option<Wild>,
        ext: bool,
    ) -> Result<()> {
        for c in node.xs() {
            match c.local.as_str() {
                "attribute" => {
                    let (name, ty) = self.attribute_decl(d, c)?;
                    attrs.retain(|a| a.name != name);
                    if c.attr("use") != Some("prohibited") {
                        attrs.push(AttrUse {
                            name,
                            ty,
                            required: c.attr("use") == Some("required"),
                        });
                    }
                }
                "attributeGroup" => {
                    let (u, l) = c.qname("ref")?.ok_or_else(|| {
                        schema_err("xs:attributeGroup without ref")
                    })?;
                    let (gd, g) = self
                        .s
                        .get(Kind::AttributeGroup, &u, &l)
                        .ok_or_else(|| {
                            schema_err(
                                "a reference to an undeclared attribute group",
                            )
                        })?;
                    self.attributes(gd, g, attrs, wild, true)?;
                }
                "anyAttribute" => {
                    let w = self.wildcard(d, c);
                    *wild = Some(match (wild.take(), w) {
                        (Some(a), b) if ext => union_wild(a, b),
                        (_, b) => b,
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The name and type of an attribute declaration or reference.
    fn attribute_decl(
        &mut self,
        d: usize,
        c: &Node,
    ) -> Result<(OwnedName, SimpleTypeId)> {
        if let Some((u, l)) = c.qname("ref")? {
            let (gd, g) =
                self.s.get(Kind::Attribute, &u, &l).ok_or_else(|| {
                    if u == "http://www.w3.org/XML/1998/namespace" {
                        schema_err("xml: attributes need the xml.xsd document")
                    } else {
                        schema_err("a reference to an undeclared attribute")
                    }
                })?;
            let ty = self.simple_ref(gd, g, "type")?.unwrap_or(self.string);
            return Ok((OwnedName::new(&u, &l), ty));
        }
        let local = c
            .attr("name")
            .ok_or_else(|| schema_err("xs:attribute without name or ref"))?;
        let doc = &self.s.docs[d];
        let qualified = match c.attr("form") {
            Some(f) => f == "qualified",
            None => doc.attr_qualified,
        };
        let uri = if qualified { doc.tns.clone() } else { String::new() };
        let ty = self.simple_ref(d, c, "type")?.unwrap_or(self.string);
        Ok((OwnedName::new(&uri, local), ty))
    }

    fn wildcard(&self, d: usize, c: &Node) -> Wild {
        let tns = &self.s.docs[d].tns;
        match c.attr("namespace").map(str::trim).unwrap_or("##any") {
            "##any" | "##other" => Wild::Any,
            list => {
                let mut v: Vec<String> = list
                    .split_whitespace()
                    .map(|u| match u {
                        "##local" => String::new(),
                        "##targetNamespace" => tns.clone(),
                        u => u.to_string(),
                    })
                    .collect();
                v.sort();
                v.dedup();
                Wild::List(v)
            }
        }
    }

    /// The content model particle of a complex type or derivation node.
    fn model(&mut self, d: usize, node: &Node) -> Result<Option<Particle>> {
        for c in node.xs() {
            if matches!(
                c.local.as_str(),
                "sequence" | "choice" | "all" | "group"
            ) {
                return Ok(Some(self.particle(d, c)?));
            }
        }
        Ok(None)
    }

    fn particle(&mut self, d: usize, c: &Node) -> Result<Particle> {
        let (min, max) = c.occurs()?;
        let term = match c.local.as_str() {
            "element" => Term::Elem(self.element_particle(d, c)?),
            "any" => Term::Any(self.wildcard(d, c)),
            "group" => {
                let (u, l) = c
                    .qname("ref")?
                    .ok_or_else(|| schema_err("xs:group without ref"))?;
                let (gd, g) =
                    self.s.get(Kind::Group, &u, &l).ok_or_else(|| {
                        schema_err("a reference to an undeclared group")
                    })?;
                let inner = g
                    .xs()
                    .find(|x| {
                        matches!(
                            x.local.as_str(),
                            "sequence" | "choice" | "all"
                        )
                    })
                    .ok_or_else(|| {
                        schema_err("xs:group without a model group")
                    })?;
                // The group's model with the reference's occurrences.
                let p = self.particle(gd, inner)?;
                return Ok(Particle { term: p.term, min, max });
            }
            kind => {
                let mut items = Vec::new();
                for x in c.xs() {
                    if matches!(
                        x.local.as_str(),
                        "element" | "any" | "group" | "sequence" | "choice"
                    ) {
                        items.push(self.particle(d, x)?);
                    }
                }
                match kind {
                    "sequence" => Term::Seq(items),
                    "choice" => Term::Choice(items),
                    "all" => Term::All(items),
                    _ => return Err(unsupported("an XSD 1.1 particle")),
                }
            }
        };
        Ok(Particle { term, min, max })
    }

    /// The elements an element particle accepts: itself and its
    /// substitution group.
    fn element_particle(
        &mut self,
        d: usize,
        c: &Node,
    ) -> Result<Vec<(ElementId, OwnedName)>> {
        if let Some((u, l)) = c.qname("ref")? {
            let mut names: Vec<(String, String)> = alloc::vec![(u, l)];
            // Substitution group members, transitively.
            let mut i = 0;
            while i < names.len() {
                let (hu, hl) = names[i].clone();
                for ((kind, mu, ml), &(md, mi)) in &self.s.index {
                    if *kind != Kind::Element {
                        continue;
                    }
                    let m = &self.s.docs[md].root.children[mi];
                    if let Some((su, sl)) = m.qname("substitutionGroup")? {
                        if su == hu
                            && sl == hl
                            && !names.contains(&(mu.clone(), ml.clone()))
                        {
                            names.push((mu.clone(), ml.clone()));
                        }
                    }
                }
                i += 1;
            }
            names.sort_by(|a, b| (&a.1, &a.0).cmp(&(&b.1, &b.0)));
            let mut out = Vec::new();
            for (u, l) in names {
                out.push((
                    self.global_element(&u, &l)?,
                    OwnedName::new(&u, &l),
                ));
            }
            return Ok(out);
        }
        self.local_element(d, c).map(|e| alloc::vec![e])
    }

    fn global_element(&mut self, uri: &str, local: &str) -> Result<ElementId> {
        if let Some(e) = self.globals.get(&(uri.into(), local.into())) {
            return Ok(*e);
        }
        let (d, node) =
            self.s.get(Kind::Element, uri, local).ok_or_else(|| {
                schema_err("a reference to an undeclared element")
            })?;
        let ty = self.element_type(d, node)?;
        let e = self.b.element(
            OwnedName::new(uri, local),
            ty,
            node.attr("nillable") == Some("true"),
        );
        self.globals.insert((uri.into(), local.into()), e);
        Ok(e)
    }

    fn local_element(
        &mut self,
        d: usize,
        node: &Node,
    ) -> Result<(ElementId, OwnedName)> {
        if let Some(e) = self.locals.get(&key_of(node)) {
            return Ok(e.clone());
        }
        let local = node
            .attr("name")
            .ok_or_else(|| schema_err("xs:element without name or ref"))?;
        let doc = &self.s.docs[d];
        let qualified = match node.attr("form") {
            Some(f) => f == "qualified",
            None => doc.elem_qualified,
        };
        let uri = if qualified { doc.tns.clone() } else { String::new() };
        let ty = self.element_type(d, node)?;
        let name = OwnedName::new(&uri, local);
        let e = self.b.element(
            name.clone(),
            ty,
            node.attr("nillable") == Some("true"),
        );
        self.locals.insert(key_of(node), (e, name.clone()));
        Ok((e, name))
    }

    /// An element's type: its type attribute, an inline type, its
    /// substitution group head's type, or anyType.
    fn element_type(&mut self, d: usize, node: &Node) -> Result<TypeId> {
        if let Some((u, l)) = node.qname("type")? {
            return self.type_of(&TypeKey::Named(u, l));
        }
        if let Some(inline) = node
            .xs()
            .find(|c| c.local == "complexType" || c.local == "simpleType")
        {
            return self.type_of(&TypeKey::Anon(key_of(inline), d));
        }
        if let Some((u, l)) = node.qname("substitutionGroup")? {
            let (hd, head) =
                self.s.get(Kind::Element, &u, &l).ok_or_else(|| {
                    schema_err(
                        "a substitution group head that is not declared",
                    )
                })?;
            return self.element_type(hd, head);
        }
        self.type_of(&TypeKey::Named(XS.into(), "anyType".into()))
    }

    // ----- states -----

    /// The states of a complex type: attribute states, then content
    /// states.
    fn assemble(&mut self, p: &Parts) -> Result<Vec<Vec<Prod>>> {
        let content = match &p.content {
            Content::Empty if p.mixed => alloc::vec![alloc::vec![
                Prod::EE,
                Prod::Ch { ty: self.string, next: 0 }
            ]],
            Content::Empty => alloc::vec![alloc::vec![Prod::EE]],
            Content::Simple(st) => simple_states(*st),
            Content::Model(m) => self.content_states(m, p.mixed)?,
        };
        let mut attrs = p.attrs.clone();
        attrs.sort_by(|a, b| {
            (&*a.name.local, &*a.name.uri).cmp(&(&*b.name.local, &*b.name.uri))
        });
        let n = attrs.len() as u32;
        let wild: Vec<Prod> = match &p.wild {
            None => Vec::new(),
            Some(Wild::Any) => alloc::vec![Prod::AtAny { next: 0 }],
            Some(Wild::List(us)) => us
                .iter()
                .map(|u| Prod::AtUri { uri: u.as_str().into(), next: 0 })
                .collect(),
        };
        let with_wild = |state: u32| {
            wild.iter().map(|w| set_next(w, state)).collect::<Vec<_>>()
        };
        // The start tag's last state is a copy of the content's first,
        // with the wildcard; without a wildcard it is the content's
        // first state itself.
        let copy = !wild.is_empty();
        let off = n + copy as u32;
        let shift = |prods: &[Prod]| {
            prods.iter().map(|q| shift_next(q, off)).collect::<Vec<_>>()
        };
        let mut states: Vec<Vec<Prod>> =
            Vec::with_capacity(off as usize + content.len());
        // Built from the last attribute back to the first.
        let mut after: Vec<Prod> = shift(&content[0]);
        if copy {
            let mut s = after.clone();
            s.extend(with_wild(n));
            after = s.clone();
            states.push(s);
        }
        let mut rev: Vec<Vec<Prod>> = Vec::new();
        for i in (0..n).rev() {
            let a = &attrs[i as usize];
            // State i + 1 is the next attribute's, or, after the last,
            // the copy or the content's first state: both are number n.
            let next = i + 1;
            let mut s =
                alloc::vec![Prod::At { name: a.name.clone(), ty: a.ty, next }];
            s.extend(with_wild(i));
            if !a.required {
                s.extend(
                    after
                        .iter()
                        .filter(|q| {
                            !matches!(
                                q,
                                Prod::AtAny { .. } | Prod::AtUri { .. }
                            )
                        })
                        .cloned(),
                );
            }
            sort_prods(&mut s);
            after = s.clone();
            rev.push(s);
        }
        rev.reverse();
        let mut out = rev;
        out.extend(states);
        for c in &content {
            out.push(shift(c));
        }
        for s in &mut out {
            sort_prods(s);
        }
        Ok(out)
    }

    /// The content states of a particle: a deterministic automaton over
    /// its positions.
    fn content_states(
        &mut self,
        m: &Particle,
        mixed: bool,
    ) -> Result<Vec<Vec<Prod>>> {
        let ch = |s: &mut Vec<Prod>, me: u32| {
            if mixed {
                s.push(Prod::Ch { ty: self.string, next: me });
            }
        };
        // xs:all: one state that loops over its elements (EXI
        // §8.5.4.1.6).
        if let Term::All(items) = &m.term {
            let mut s = Vec::new();
            for it in items {
                if let Term::Elem(es) = &it.term {
                    for (e, _) in es {
                        s.push(Prod::Se { element: *e, next: 0 });
                    }
                }
            }
            s.push(Prod::EE);
            ch(&mut s, 0);
            return Ok(alloc::vec![s]);
        }
        let mut g = Glushkov::default();
        let f = g.particle(m, self.max_states)?;
        // Subset construction from the start position.
        const START: usize = usize::MAX;
        let mut sets: Vec<Vec<usize>> = alloc::vec![alloc::vec![START]];
        let mut out: Vec<Vec<Prod>> = Vec::new();
        let mut i = 0;
        while i < sets.len() {
            let set = sets[i].clone();
            let follows: Vec<usize> = {
                let mut v = BTreeSet::new();
                for &p in &set {
                    if p == START {
                        v.extend(f.first.iter().copied());
                    } else {
                        v.extend(g.follow[p].iter().copied());
                    }
                }
                v.into_iter().collect()
            };
            // Group by what each position matches, in order of first
            // appearance.
            let mut groups: Vec<(TermKey, Vec<usize>)> = Vec::new();
            for &q in &follows {
                for k in &g.keys[q] {
                    match groups.iter_mut().find(|(gk, _)| gk.same(k)) {
                        Some((_, v)) => v.push(q),
                        None => groups.push((k.clone(), alloc::vec![q])),
                    }
                }
            }
            groups.sort_by_key(|(k, _)| (k.rank(), g.order(k)));
            let mut s = Vec::new();
            for (k, mut target) in groups {
                target.sort();
                target.dedup();
                let next = match sets.iter().position(|x| *x == target) {
                    Some(j) => j,
                    None => {
                        if sets.len() >= self.max_states {
                            return Err(Error::new(
                                ErrorKind::Limit,
                                "a content model with more states than max_states",
                            ));
                        }
                        sets.push(target);
                        sets.len() - 1
                    }
                } as u32;
                s.push(match k {
                    TermKey::Elem(e, _) => Prod::Se { element: e, next },
                    TermKey::Uri(u) => Prod::SeUri { uri: u.into(), next },
                    TermKey::Any => Prod::SeAny { next },
                });
            }
            let accepts = set.iter().any(|p| {
                if *p == START { f.nullable } else { f.last.contains(p) }
            });
            if accepts {
                s.push(Prod::EE);
            }
            ch(&mut s, i as u32);
            out.push(s);
            i += 1;
        }
        Ok(out)
    }
}

/// What a position matches. Elements compare by name: two declarations
/// of one name in one content model are one terminal.
#[derive(Clone, Debug)]
enum TermKey {
    Elem(ElementId, OwnedName),
    Uri(String),
    Any,
}

impl TermKey {
    fn same(&self, o: &TermKey) -> bool {
        match (self, o) {
            (TermKey::Elem(_, a), TermKey::Elem(_, b)) => a == b,
            (TermKey::Uri(a), TermKey::Uri(b)) => a == b,
            (TermKey::Any, TermKey::Any) => true,
            _ => false,
        }
    }
    fn rank(&self) -> u8 {
        match self {
            TermKey::Elem(..) => 0,
            TermKey::Uri(_) => 1,
            TermKey::Any => 2,
        }
    }
}

/// Positions of a content model (Glushkov's construction): each element
/// or wildcard occurrence, after occurrences are unrolled, is one
/// position.
#[derive(Default)]
struct Glushkov {
    keys: Vec<Vec<TermKey>>,
    follow: Vec<BTreeSet<usize>>,
    /// Terminals in order of first appearance: schema order for SE
    /// productions.
    seen: Vec<TermKey>,
}

struct Frag {
    nullable: bool,
    first: BTreeSet<usize>,
    last: BTreeSet<usize>,
}

impl Glushkov {
    fn order(&self, k: &TermKey) -> usize {
        self.seen.iter().position(|s| s.same(k)).unwrap_or(usize::MAX)
    }

    fn empty() -> Frag {
        Frag { nullable: true, first: BTreeSet::new(), last: BTreeSet::new() }
    }

    fn seq(&mut self, a: Frag, b: Frag) -> Frag {
        for &l in &a.last {
            self.follow[l].extend(b.first.iter().copied());
        }
        let first = if a.nullable {
            a.first.union(&b.first).copied().collect()
        } else {
            a.first
        };
        let last = if b.nullable {
            a.last.union(&b.last).copied().collect()
        } else {
            b.last
        };
        Frag { nullable: a.nullable && b.nullable, first, last }
    }

    fn particle(&mut self, p: &Particle, cap: usize) -> Result<Frag> {
        if self.keys.len() > cap {
            return Err(Error::new(
                ErrorKind::Limit,
                "a content model with more states than max_states",
            ));
        }
        let mut out = Self::empty();
        for _ in 0..p.min {
            let t = self.term(&p.term, cap)?;
            out = self.seq(out, t);
        }
        match p.max {
            None => {
                let t = self.term(&p.term, cap)?;
                for &l in &t.last {
                    let f: Vec<usize> = t.first.iter().copied().collect();
                    self.follow[l].extend(f);
                }
                let star =
                    Frag { nullable: true, first: t.first, last: t.last };
                out = self.seq(out, star);
            }
            Some(max) => {
                // Up to max − min more copies, each optional: t? t? t?
                for _ in p.min..max {
                    let t = self.term(&p.term, cap)?;
                    let opt = Frag { nullable: true, ..t };
                    out = self.seq(out, opt);
                }
            }
        }
        Ok(out)
    }

    fn term(&mut self, t: &Term, cap: usize) -> Result<Frag> {
        match t {
            Term::Elem(es) => {
                let keys: Vec<TermKey> = es
                    .iter()
                    .map(|(e, n)| TermKey::Elem(*e, n.clone()))
                    .collect();
                Ok(self.position(keys))
            }
            Term::Any(Wild::Any) => {
                Ok(self.position(alloc::vec![TermKey::Any]))
            }
            Term::Any(Wild::List(us)) => Ok(self.position(
                us.iter().map(|u| TermKey::Uri(u.clone())).collect(),
            )),
            Term::Seq(items) => {
                let mut out = Self::empty();
                for it in items {
                    let f = self.particle(it, cap)?;
                    out = self.seq(out, f);
                }
                Ok(out)
            }
            Term::Choice(items) | Term::All(items) => {
                let mut out = Frag {
                    nullable: items.is_empty(),
                    first: BTreeSet::new(),
                    last: BTreeSet::new(),
                };
                for it in items {
                    let f = self.particle(it, cap)?;
                    out.nullable |= f.nullable;
                    out.first.extend(f.first);
                    out.last.extend(f.last);
                }
                Ok(out)
            }
        }
    }

    fn position(&mut self, keys: Vec<TermKey>) -> Frag {
        let p = self.keys.len();
        for k in &keys {
            if !self.seen.iter().any(|s| s.same(k)) {
                self.seen.push(k.clone());
            }
        }
        self.keys.push(keys);
        self.follow.push(BTreeSet::new());
        Frag { nullable: false, first: [p].into(), last: [p].into() }
    }
}

fn simple_states(st: SimpleTypeId) -> Vec<Vec<Prod>> {
    alloc::vec![
        alloc::vec![Prod::Ch { ty: st, next: 1 }],
        alloc::vec![Prod::EE]
    ]
}

fn set_next(p: &Prod, n: u32) -> Prod {
    match p {
        Prod::AtAny { .. } => Prod::AtAny { next: n },
        Prod::AtUri { uri, .. } => Prod::AtUri { uri: uri.clone(), next: n },
        other => other.clone(),
    }
}

fn shift_next(p: &Prod, by: u32) -> Prod {
    let mut q = p.clone();
    match &mut q {
        Prod::At { next, .. }
        | Prod::AtUri { next, .. }
        | Prod::AtAny { next }
        | Prod::Se { next, .. }
        | Prod::SeUri { next, .. }
        | Prod::SeAny { next }
        | Prod::Ch { next, .. } => *next += by,
        Prod::EE => {}
    }
    q
}

/// Orders a state's productions as EXI assigns event codes: attributes
/// by local name then URI, then AT(uri:*) by URI, AT(*), the SE
/// productions in the order given (schema order), SE(uri:*), SE(*), EE,
/// CH. The sort is stable, so SE order is kept.
fn sort_prods(s: &mut Vec<Prod>) {
    fn rank(p: &Prod) -> u8 {
        match p {
            Prod::At { .. } => 1,
            Prod::AtUri { .. } => 2,
            Prod::AtAny { .. } => 3,
            Prod::Se { .. } => 4,
            Prod::SeUri { .. } => 5,
            Prod::SeAny { .. } => 6,
            Prod::EE => 7,
            Prod::Ch { .. } => 8,
        }
    }
    s.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
            (Prod::At { name: x, .. }, Prod::At { name: y, .. }) => {
                (&*x.local, &*x.uri).cmp(&(&*y.local, &*y.uri))
            }
            (Prod::AtUri { uri: x, .. }, Prod::AtUri { uri: y, .. }) => {
                x.cmp(y)
            }
            _ => core::cmp::Ordering::Equal,
        })
    });
    s.dedup_by(|a, b| match (&*a, &*b) {
        (Prod::AtAny { .. }, Prod::AtAny { .. }) | (Prod::EE, Prod::EE) => {
            true
        }
        (Prod::At { name: x, .. }, Prod::At { name: y, .. }) => x == y,
        (Prod::AtUri { uri: x, .. }, Prod::AtUri { uri: y, .. }) => x == y,
        _ => false,
    });
}

fn union_wild(a: Wild, b: Wild) -> Wild {
    match (a, b) {
        (Wild::List(mut x), Wild::List(y)) => {
            x.extend(y);
            x.sort();
            x.dedup();
            Wild::List(x)
        }
        _ => Wild::Any,
    }
}
