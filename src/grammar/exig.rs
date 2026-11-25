// src/grammar/exig.rs
//
// Schema-informed grammars stored as EXI
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! `.exig` grammar files: schema-informed grammars saved as EXI, and
//! loaded back without an XSD compiler (Appendix C of the blueprint;
//! the vocabulary is `schemas/exig-1.xsd`).
//!
//! A grammar set from [`GrammarSet::from_xsd`](crate::grammar::GrammarSet::from_xsd), from
//! [`GrammarBuilder`](crate::grammar::informed::GrammarBuilder) or from an `.exig` file is
//! the same in memory, and [`GrammarSet::to_exig`] writes any of them.
//! Loading with [`GrammarSet::from_exig`] gives grammars that code
//! every stream exactly as the originals do.
//!
//! ```
//! # #[cfg(feature = "xsd")] {
//! use exi::grammar::GrammarSet;
//! let xsd = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:s">
//!   <xs:element name="reading"><xs:complexType>
//!     <xs:sequence><xs:element name="temp" type="xs:int" form="qualified"/></xs:sequence>
//!     <xs:attribute name="unit" type="xs:string" use="required"/>
//!   </xs:complexType></xs:element>
//! </xs:schema>"#;
//! let compiled = GrammarSet::from_xsd(xsd)?;
//! let file = compiled.to_exig()?;
//! let loaded = GrammarSet::from_exig(&file)?;
//! assert_eq!(loaded.to_exig()?, file);                       // the same grammars
//! assert_eq!(loaded.stats(), compiled.stats());
//! assert_eq!(loaded.digest()?, compiled.digest()?);
//! assert!(compiled.digest()?.starts_with("sha256:"));
//! # }
//! # Ok::<(), exi::Error>(())
//! ```
//!
//! # The file
//!
//! An `.exig` file is an EXIG document (`schemas/exig-1.xsd`) encoded
//! schema-informed under the EXIG schema, strict, byte-aligned, with
//! the options in the header. Everything in the document is in a
//! canonical order (see the schema's comments), and the writer has one
//! way to write each event, so equal grammars give equal bytes.
//! [`GrammarSet::digest`], the SHA-256 of those bytes, is then a name
//! for the grammars that any implementation can compute, usable as an
//! EXI `schemaId`.
//!
//! Reading checks everything a stream relies on before the grammars are
//! used (C.6): every number refers to something that exists,
//! productions are in event-code order with no repeats, and facets fit
//! their representation. A file that fails any check is
//! `ErrorKind::Schema`.
//!
//! ```
//! use exi::grammar::GrammarSet;
//! assert_eq!(GrammarSet::from_exig(b"\x80").unwrap_err().kind(), exi::ErrorKind::Header);
//! // Schema-less grammars have nothing to save.
//! assert_eq!(GrammarSet::schemaless().to_exig().unwrap_err().kind(), exi::ErrorKind::Arg);
//! ```
//!
//! # The EXIG schema's own grammars
//!
//! Reading an `.exig` file needs the EXIG schema's grammars, so those
//! cannot come from a file. They are constant tables in this crate
//! ([`exig_grammars`]), generated from `schemas/exig-1.xsd` by this
//! crate's own XSD compiler. A test regenerates them and fails when the
//! committed tables differ; another checks the fixed point: the EXIG
//! schema's own `.exig`, decoded with the constant tables and written
//! again, is the same file.

use alloc::boxed::Box;
#[cfg(all(test, feature = "xsd", feature = "std"))]
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::grammar::informed::{
    ElementId, IntegerRep, Prod, Representation, SimpleTypeId, TypeId,
};
use crate::grammar::{GrammarSet, Terminal};
use crate::header::Header;
use crate::reader::{Reader, ReaderConfig};
use crate::strings::{GlobalConfig, GlobalStrings};
use crate::tables::InitialEntries;
use crate::value::{BinaryKind, DateTimeKind, OwnedValue};
use crate::writer::{Writer, WriterConfig};
use crate::{
    Coding, Error, ErrorKind, EventSink, Limits, Options, OwnedEvent,
    OwnedName, Result, Step,
};

/// The EXIG namespace (a placeholder until one is assigned).
pub const EXIG_NS: &str = "urn:exig:draft-1";

/// The EXIG schema, `schemas/exig-1.xsd`.
pub const EXIG_SCHEMA: &str = include_str!("../../schemas/exig-1.xsd");

fn schema_err(detail: &'static str) -> Error {
    Error::new(ErrorKind::Schema, detail)
}

// =======================================================================================
// The model: a grammar set as builder input, in canonical order
// =======================================================================================

/// A type grammar as the builder takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypeModel {
    pub name: Option<OwnedName>,
    pub castable: bool,
    /// Named types other than this one that xsi:type may name for it.
    pub casts: Vec<u32>,
    pub states: Vec<Vec<Prod>>,
}

/// A grammar set as builder input. From a set, its parts are in
/// canonical order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct Model {
    pub schema_id: Option<String>,
    /// Initial string table entries beyond the fixed ones: URI and its
    /// local names.
    pub strings: Vec<(String, Vec<String>)>,
    pub simple: Vec<Representation>,
    pub types: Vec<TypeModel>,
    pub elements: Vec<(OwnedName, u32, bool)>,
    pub globals: Vec<u32>,
}

impl Model {
    /// The canonical model of a schema-informed set (the order in
    /// `schemas/exig-1.xsd`).
    pub(crate) fn from_set(set: &GrammarSet) -> Result<Model> {
        let s = set.schema.as_ref().ok_or_else(|| {
            Error::new(
                ErrorKind::Arg,
                "schema-less grammars have no .exig form",
            )
        })?;
        let entries = &set.entries;
        let name_of = |id: crate::NameId| -> OwnedName {
            OwnedName::new(
                entries.uris()[id.uri as usize],
                entries.locals(id.uri)[id.local as usize],
            )
        };
        let mut w = Walk {
            tmap: alloc::vec![None; s.types.len()],
            emap: alloc::vec![None; s.elements.len()],
            smap: alloc::vec![None; s.simple.len()],
            ..Walk::default()
        };
        // Named types first, by URI then local name.
        let mut named: Vec<usize> = (0..s.types.len())
            .filter(|&i| s.types[i].name.is_some())
            .collect();
        named.sort_by(|&a, &b| {
            let (x, y) = (
                s.types[a].name.as_ref().expect("named"),
                s.types[b].name.as_ref().expect("named"),
            );
            (&*x.uri, &*x.local).cmp(&(&*y.uri, &*y.local))
        });
        for &t in &named {
            w.tmap[t] = Some(w.torder.len() as u32);
            w.torder.push(t);
        }
        // Then a depth-first walk from the globals, then from the named
        // types.
        let mut globals: Vec<ElementId> = s.globals.clone();
        globals.sort_by(|a, b| {
            let (x, y) = (&s.elem(*a).name, &s.elem(*b).name);
            (&*x.local, &*x.uri).cmp(&(&*y.local, &*y.uri))
        });
        for g in &globals {
            w.element(s, g.0 as usize);
        }
        for &t in &named {
            w.ty(s, t);
        }
        // Elements no walk reaches still name FragmentContent entries.
        for e in 0..s.elements.len() {
            w.element(s, e);
        }

        let mut m = Model { schema_id: None, ..Model::default() };
        let fixed = InitialEntries::schema().build()?;
        for (u, uri) in entries.uris().iter().enumerate() {
            let known = fixed.uris().iter().position(|x| x == uri);
            let locals: Vec<String> = entries
                .locals(u as u32)
                .iter()
                .filter(|l| {
                    known.is_none_or(|k| !fixed.locals(k as u32).contains(l))
                })
                .map(|l| l.to_string())
                .collect();
            if known.is_none() || !locals.is_empty() {
                m.strings.push((uri.to_string(), locals));
            }
        }
        for &old in &w.sorder {
            m.simple.push(match &s.simple[old] {
                Representation::List(i) => Representation::List(SimpleTypeId(
                    w.smap[i.0 as usize].expect("walked"),
                )),
                Representation::Enumeration { values, base } => {
                    Representation::Enumeration {
                        values: values.clone(),
                        base: SimpleTypeId(
                            w.smap[base.0 as usize].expect("walked"),
                        ),
                    }
                }
                r => r.clone(),
            });
        }
        for &old in &w.torder {
            let td = &s.types[old];
            let order = state_order(&td.states);
            let mut states = Vec::with_capacity(order.len());
            for &st in &order {
                let new_state = |n: u32| {
                    order
                        .iter()
                        .position(|&o| o == n as usize)
                        .expect("reachable") as u32
                };
                let prods = td.states[st]
                    .iter()
                    .map(|p| {
                        let next = new_state(p.next);
                        match p.term {
                            Terminal::AT(id) => Prod::At {
                                name: name_of(id),
                                ty: SimpleTypeId(
                                    w.smap[p.ty.expect("typed").0 as usize]
                                        .expect("walked"),
                                ),
                                next,
                            },
                            Terminal::AtUri(u) => Prod::AtUri {
                                uri: entries.uris()[u as usize].into(),
                                next,
                            },
                            Terminal::AtAny => Prod::AtAny { next },
                            Terminal::SE(_) => Prod::Se {
                                element: ElementId(
                                    w.emap
                                        [p.elem.expect("element").0 as usize]
                                        .expect("walked"),
                                ),
                                next,
                            },
                            Terminal::SeUri(u) => Prod::SeUri {
                                uri: entries.uris()[u as usize].into(),
                                next,
                            },
                            Terminal::SeAny => Prod::SeAny { next },
                            Terminal::CH => Prod::Ch {
                                ty: SimpleTypeId(
                                    w.smap[p.ty.expect("typed").0 as usize]
                                        .expect("walked"),
                                ),
                                next,
                            },
                            _ => Prod::EE,
                        }
                    })
                    .collect();
                states.push(prods);
            }
            let mut casts: Vec<u32> = td
                .casts
                .iter()
                .filter(|c| c.0 as usize != old)
                .map(|c| w.tmap[c.0 as usize].expect("named"))
                .collect();
            casts.sort();
            m.types.push(TypeModel {
                name: td.name.clone(),
                castable: td.castable,
                casts,
                states,
            });
        }
        for &old in &w.eorder {
            let e = &s.elements[old];
            m.elements.push((
                e.name.clone(),
                w.tmap[e.ty.0 as usize].expect("walked"),
                e.nillable,
            ));
        }
        m.globals = globals
            .iter()
            .map(|g| w.emap[g.0 as usize].expect("walked"))
            .collect();
        Ok(m)
    }

    /// The grammar set: the builder checks everything (C.6).
    pub(crate) fn build(&self) -> Result<GrammarSet> {
        let mut b = GrammarSet::builder();
        for r in &self.simple {
            b.simple_type(None, r.clone());
        }
        for t in &self.types {
            b.reserve_type(t.name.clone());
        }
        let n = self.types.len() as u32;
        for (i, t) in self.types.iter().enumerate() {
            let me = TypeId(i as u32);
            b.set_states(me, t.states.clone());
            if t.castable {
                b.castable(me);
            }
            for &c in &t.casts {
                if c >= n {
                    return Err(schema_err(
                        "a cast to a type that does not exist",
                    ));
                }
                b.derives(TypeId(c), me);
            }
        }
        for (name, ty, nillable) in &self.elements {
            b.element(name.clone(), TypeId(*ty), *nillable);
        }
        for g in &self.globals {
            b.global(ElementId(*g));
        }
        for (u, locals) in &self.strings {
            b.uri(u);
            for l in locals {
                b.name(OwnedName::new(u, l));
            }
        }
        b.build()
    }
}

/// States in breadth-first order from state 0, following productions in
/// order.
fn state_order(states: &[Vec<crate::grammar::informed::IProd>]) -> Vec<usize> {
    let mut order = alloc::vec![0usize];
    let mut i = 0;
    while i < order.len() {
        for p in &states[order[i]] {
            if p.term != Terminal::EE && !order.contains(&(p.next as usize)) {
                order.push(p.next as usize);
            }
        }
        i += 1;
    }
    order
}

/// The canonical walk: first-reach numbers of types, elements and
/// simple types.
#[derive(Default)]
struct Walk {
    tmap: Vec<Option<u32>>,
    torder: Vec<usize>,
    walked: Vec<usize>,
    emap: Vec<Option<u32>>,
    eorder: Vec<usize>,
    smap: Vec<Option<u32>>,
    sorder: Vec<usize>,
}

impl Walk {
    fn element(&mut self, s: &crate::grammar::informed::Schema, e: usize) {
        if self.emap[e].is_some() {
            return;
        }
        self.emap[e] = Some(self.eorder.len() as u32);
        self.eorder.push(e);
        self.ty(s, s.elements[e].ty.0 as usize);
    }

    fn ty(&mut self, s: &crate::grammar::informed::Schema, t: usize) {
        if self.walked.contains(&t) {
            return;
        }
        self.walked.push(t);
        if self.tmap[t].is_none() {
            self.tmap[t] = Some(self.torder.len() as u32);
            self.torder.push(t);
        }
        let td = &s.types[t];
        for st in state_order(&td.states) {
            for p in &td.states[st] {
                if let Some(ty) = p.ty {
                    self.simple(s, ty.0 as usize);
                }
                if let Some(e) = p.elem {
                    self.element(s, e.0 as usize);
                }
            }
        }
    }

    fn simple(&mut self, s: &crate::grammar::informed::Schema, t: usize) {
        if self.smap[t].is_some() {
            return;
        }
        self.smap[t] = Some(self.sorder.len() as u32);
        self.sorder.push(t);
        match &s.simple[t] {
            Representation::List(i) => self.simple(s, i.0 as usize),
            Representation::Enumeration { base, .. } => {
                self.simple(s, base.0 as usize)
            }
            _ => {}
        }
    }
}

// =======================================================================================
// Names of representations and events in the EXIG vocabulary
// =======================================================================================

fn rep_name(r: &Representation) -> &'static str {
    use DateTimeKind::*;
    match r {
        Representation::String { .. } => "string",
        Representation::Boolean { pattern: false } => "boolean",
        Representation::Boolean { pattern: true } => "booleanPattern",
        Representation::Integer(IntegerRep::Signed) => "integer",
        Representation::Integer(IntegerRep::Unsigned) => "unsignedInteger",
        Representation::Integer(IntegerRep::Bounded { .. }) => {
            "boundedInteger"
        }
        Representation::Decimal => "decimal",
        Representation::Float => "float",
        Representation::Binary(BinaryKind::Hex) => "hexBinary",
        Representation::Binary(BinaryKind::Base64) => "base64Binary",
        Representation::DateTime(GYear) => "gYear",
        Representation::DateTime(GYearMonth) => "gYearMonth",
        Representation::DateTime(Date) => "date",
        Representation::DateTime(DateTime) => "dateTime",
        Representation::DateTime(GMonth) => "gMonth",
        Representation::DateTime(GMonthDay) => "gMonthDay",
        Representation::DateTime(GDay) => "gDay",
        Representation::DateTime(Time) => "time",
        Representation::List(_) => "list",
        Representation::Enumeration { .. } => "enumeration",
    }
}

/// Facts collected for one `simpleType` element.
#[derive(Default)]
struct SimpleFacts {
    rep: String,
    min: Option<i64>,
    max: Option<i64>,
    item: Option<u32>,
    base: Option<u32>,
    values: Vec<Box<str>>,
    charset: Option<String>,
}

fn rep_from(f: SimpleFacts) -> Result<Representation> {
    use DateTimeKind::*;
    let bad = || {
        schema_err("a simple type whose facets do not fit its representation")
    };
    Ok(match f.rep.as_str() {
        "string" => Representation::String {
            charset: f.charset.map(|c| c.chars().collect()),
        },
        "boolean" => Representation::Boolean { pattern: false },
        "booleanPattern" => Representation::Boolean { pattern: true },
        "integer" => Representation::Integer(IntegerRep::Signed),
        "unsignedInteger" => Representation::Integer(IntegerRep::Unsigned),
        "boundedInteger" => Representation::Integer(IntegerRep::Bounded {
            min: f.min.ok_or_else(bad)?,
            max: f.max.ok_or_else(bad)?,
        }),
        "decimal" => Representation::Decimal,
        "float" => Representation::Float,
        "hexBinary" => Representation::Binary(BinaryKind::Hex),
        "base64Binary" => Representation::Binary(BinaryKind::Base64),
        "gYear" => Representation::DateTime(GYear),
        "gYearMonth" => Representation::DateTime(GYearMonth),
        "date" => Representation::DateTime(Date),
        "dateTime" => Representation::DateTime(DateTime),
        "gMonth" => Representation::DateTime(GMonth),
        "gMonthDay" => Representation::DateTime(GMonthDay),
        "gDay" => Representation::DateTime(GDay),
        "time" => Representation::DateTime(Time),
        "list" => Representation::List(SimpleTypeId(f.item.ok_or_else(bad)?)),
        "enumeration" => Representation::Enumeration {
            values: f.values.into(),
            base: SimpleTypeId(f.base.ok_or_else(bad)?),
        },
        _ => return Err(bad()),
    })
}

// =======================================================================================
// Writing and reading
// =======================================================================================

/// The EXIG schema's grammars, from the constant tables.
pub fn exig_grammars() -> Result<GrammarSet> {
    super::exig_tables::model().build()
}

#[cfg(feature = "std")]
fn with_exig<T>(f: impl FnOnce(&GrammarSet) -> Result<T>) -> Result<T> {
    static SET: std::sync::OnceLock<GrammarSet> = std::sync::OnceLock::new();
    if let Some(s) = SET.get() {
        return f(s);
    }
    let built = exig_grammars()?;
    f(SET.get_or_init(|| built))
}

#[cfg(not(feature = "std"))]
fn with_exig<T>(f: impl FnOnce(&GrammarSet) -> Result<T>) -> Result<T> {
    f(&exig_grammars()?)
}

fn exig_options() -> Options {
    Options::builder()
        .coding(Coding::ByteAligned)
        .strict(true)
        .build()
        .expect("valid options")
}

impl GrammarSet {
    /// This set as an `.exig` file. See the [module documentation](self).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` for schema-less grammars, which have nothing to
    /// save.
    pub fn to_exig(&self) -> Result<Vec<u8>> {
        let m = Model::from_set(self)?;
        with_exig(|g| write_model(g, &m))
    }

    /// Reads an `.exig` file and checks it (C.6) under default limits.
    pub fn from_exig(exig: &[u8]) -> Result<GrammarSet> {
        Self::from_exig_with(exig, Limits::new())
    }

    /// [`from_exig`](Self::from_exig) under `limits`.
    pub fn from_exig_with(exig: &[u8], limits: Limits) -> Result<GrammarSet> {
        let m = with_exig(|g| read_model(g, exig, limits))?;
        m.build()
    }

    /// `sha256:` and the 64 lowercase hex digits of the SHA-256 of [`to_exig`](Self::to_exig):
    /// a name for these grammars that is the same in every
    /// implementation of the format.
    pub fn digest(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        let bytes = self.to_exig()?;
        let hash = Sha256::digest(&bytes);
        let mut out = String::with_capacity(71);
        out.push_str("sha256:");
        for b in hash {
            out.push(char::from_digit((b >> 4) as u32, 16).expect("hex"));
            out.push(char::from_digit((b & 15) as u32, 16).expect("hex"));
        }
        Ok(out)
    }
}

/// Writes the EXIG document of `m` with the EXIG grammars `g`.
fn write_model(g: &GrammarSet, m: &Model) -> Result<Vec<u8>> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let grammars = g.attach(&strings)?;
    let cfg = WriterConfig::new(exig_options(), &grammars)
        .header(Header::WithOptions { cookie: false });
    let mut w = Writer::new(Vec::new(), cfg)?;
    let el = |w: &mut Writer<'_, Vec<u8>>, local: &str| {
        w.start_element((EXIG_NS, local))
    };
    let int = |w: &mut Writer<'_, Vec<u8>>, name: &str, v: u64| {
        w.attribute(name, OwnedValue::Int(v as i64).as_value())
    };
    let leaf = |w: &mut Writer<'_, Vec<u8>>,
                local: &str,
                v: OwnedValue|
     -> Result<()> {
        w.start_element((EXIG_NS, local))?;
        w.text(v.as_value())?;
        w.end_element()
    };
    el(&mut w, "grammars")?;
    if let Some(id) = &m.schema_id {
        w.attribute("schemaId", id.as_str())?;
    }
    el(&mut w, "strings")?;
    for (u, locals) in &m.strings {
        el(&mut w, "uri")?;
        w.attribute("value", u.as_str())?;
        for l in locals {
            leaf(&mut w, "local", OwnedValue::Str(l.as_str().into()))?;
        }
        w.end_element()?;
    }
    w.end_element()?;
    for r in &m.simple {
        el(&mut w, "simpleType")?;
        w.attribute("representation", rep_name(r))?;
        match r {
            Representation::Integer(IntegerRep::Bounded { min, max }) => {
                w.attribute("minInclusive", OwnedValue::Int(*min).as_value())?;
                w.attribute("maxInclusive", OwnedValue::Int(*max).as_value())?;
            }
            Representation::List(i) => int(&mut w, "itemType", i.0 as u64)?,
            Representation::Enumeration { base, .. } => {
                int(&mut w, "base", base.0 as u64)?
            }
            _ => {}
        }
        if let Representation::Enumeration { values, .. } = r {
            for v in values.iter() {
                leaf(&mut w, "enumeration", OwnedValue::Str(v.clone()))?;
            }
        }
        if let Representation::String { charset: Some(cs) } = r {
            leaf(
                &mut w,
                "charset",
                OwnedValue::Str(cs.iter().collect::<String>().into()),
            )?;
        }
        w.end_element()?;
    }
    for t in &m.types {
        el(&mut w, "type")?;
        if let Some(n) = &t.name {
            w.attribute("uri", &*n.uri)?;
            w.attribute("local", &*n.local)?;
        }
        if t.castable {
            w.attribute("castable", true)?;
        }
        for st in &t.states {
            el(&mut w, "state")?;
            for p in st {
                el(&mut w, "production")?;
                let (event, next) = match p {
                    Prod::At { name, ty, next } => {
                        w.attribute("uri", &*name.uri)?;
                        w.attribute("local", &*name.local)?;
                        int(&mut w, "type", ty.0 as u64)?;
                        ("AT", Some(*next))
                    }
                    Prod::AtUri { uri, next } => {
                        w.attribute("uri", &**uri)?;
                        ("ATuri", Some(*next))
                    }
                    Prod::AtAny { next } => ("ATany", Some(*next)),
                    Prod::Se { element, next } => {
                        int(&mut w, "element", element.0 as u64)?;
                        ("SE", Some(*next))
                    }
                    Prod::SeUri { uri, next } => {
                        w.attribute("uri", &**uri)?;
                        ("SEuri", Some(*next))
                    }
                    Prod::SeAny { next } => ("SEany", Some(*next)),
                    Prod::EE => ("EE", None),
                    Prod::Ch { ty, next } => {
                        int(&mut w, "type", ty.0 as u64)?;
                        ("CH", Some(*next))
                    }
                };
                w.attribute("event", event)?;
                if let Some(n) = next {
                    int(&mut w, "next", n as u64)?;
                }
                w.end_element()?;
            }
            w.end_element()?;
        }
        for &c in &t.casts {
            leaf(&mut w, "cast", OwnedValue::Int(c as i64))?;
        }
        w.end_element()?;
    }
    for (name, ty, nillable) in &m.elements {
        el(&mut w, "element")?;
        w.attribute("uri", &*name.uri)?;
        w.attribute("local", &*name.local)?;
        int(&mut w, "type", *ty as u64)?;
        if *nillable {
            w.attribute("nillable", true)?;
        }
        w.end_element()?;
    }
    for &gl in &m.globals {
        leaf(&mut w, "global", OwnedValue::Int(gl as i64))?;
    }
    w.end_element()?;
    w.end_document()?;
    Ok(w.into_sink())
}

/// Reads an EXIG document into a model; [`Model::build`] checks it.
fn read_model(g: &GrammarSet, exig: &[u8], limits: Limits) -> Result<Model> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let grammars = g.attach(&strings)?;
    let mut r: Reader =
        Reader::new(ReaderConfig::new(&grammars).limits(limits))?;
    let mut events = Vec::new();
    let mut rest = exig;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        match r.next()? {
            Step::Event(e, _) => events.push(e.to_owned()),
            Step::NeedInput if rest.is_empty() => {
                return Err(Error::new(
                    ErrorKind::Truncated,
                    "the .exig ends early",
                ));
            }
            Step::NeedInput => {}
            Step::End => break,
        }
    }
    let opts = r.options().ok_or_else(|| {
        Error::new(ErrorKind::Header, "an .exig without options in its header")
    })?;
    if !opts.strict() {
        return Err(Error::new(
            ErrorKind::Header,
            "an .exig that is not strict",
        ));
    }
    parse_events(&events)
}

fn uint(v: &OwnedValue) -> Result<u32> {
    match v {
        OwnedValue::Int(i) => {
            u32::try_from(*i).map_err(|_| schema_err("a number out of range"))
        }
        _ => Err(schema_err("a number expected")),
    }
}

fn text(v: &OwnedValue) -> Result<String> {
    match v {
        OwnedValue::Str(s) => Ok(s.to_string()),
        OwnedValue::Enum { text, .. } => Ok(text.to_string()),
        _ => Err(schema_err("text expected")),
    }
}

/// The model of a decoded EXIG document's events. The strict grammars
/// already hold the document to the EXIG schema, so this only collects.
fn parse_events(events: &[OwnedEvent]) -> Result<Model> {
    let mut m = Model::default();
    // The open elements, by local name.
    let mut path: Vec<String> = Vec::new();
    let mut facts = SimpleFacts::default();
    let mut ty = TypeModel {
        name: None,
        castable: false,
        casts: Vec::new(),
        states: Vec::new(),
    };
    let mut tname: (Option<String>, Option<String>) = (None, None);
    let mut prod: Vec<(String, OwnedValue)> = Vec::new();
    let mut elem: Vec<(String, OwnedValue)> = Vec::new();
    for e in events {
        match e {
            OwnedEvent::StartElement(n) => {
                let local = n.as_name().local_name().to_string();
                match local.as_str() {
                    "simpleType" => facts = SimpleFacts::default(),
                    "type" => {
                        ty = TypeModel {
                            name: None,
                            castable: false,
                            casts: Vec::new(),
                            states: Vec::new(),
                        };
                        tname = (None, None);
                    }
                    "state" => ty.states.push(Vec::new()),
                    "production" => prod.clear(),
                    "element" => elem.clear(),
                    "uri" => m.strings.push((String::new(), Vec::new())),
                    _ => {}
                }
                path.push(local);
            }
            OwnedEvent::Attribute(n, v) => {
                let a = n.as_name().local_name();
                match path.last().map(String::as_str) {
                    Some("grammars") if a == "schemaId" => {
                        m.schema_id = Some(text(v)?)
                    }
                    Some("uri") if a == "value" => {
                        m.strings.last_mut().expect("pushed").0 = text(v)?
                    }
                    Some("simpleType") => match a {
                        "representation" => facts.rep = text(v)?,
                        "minInclusive" | "maxInclusive" => {
                            let OwnedValue::Int(i) = v else {
                                return Err(schema_err("a number expected"));
                            };
                            if a == "minInclusive" {
                                facts.min = Some(*i);
                            } else {
                                facts.max = Some(*i);
                            }
                        }
                        "itemType" => facts.item = Some(uint(v)?),
                        "base" => facts.base = Some(uint(v)?),
                        _ => {}
                    },
                    Some("type") => match a {
                        "uri" => tname.0 = Some(text(v)?),
                        "local" => tname.1 = Some(text(v)?),
                        "castable" => {
                            ty.castable = matches!(v, OwnedValue::Bool(true))
                        }
                        _ => {}
                    },
                    Some("production") => {
                        prod.push((a.to_string(), v.clone()))
                    }
                    Some("element") => elem.push((a.to_string(), v.clone())),
                    _ => {}
                }
            }
            OwnedEvent::Characters(v) => match path.last().map(String::as_str)
            {
                Some("local") => m
                    .strings
                    .last_mut()
                    .ok_or_else(|| schema_err("a local name outside a URI"))?
                    .1
                    .push(text(v)?),
                Some("enumeration") => facts.values.push(text(v)?.into()),
                Some("charset") => facts.charset = Some(text(v)?),
                Some("cast") => ty.casts.push(uint(v)?),
                Some("global") => m.globals.push(uint(v)?),
                _ => {}
            },
            OwnedEvent::EndElement(_) => {
                let local = path.pop().unwrap_or_default();
                match local.as_str() {
                    "simpleType" => {
                        m.simple.push(rep_from(core::mem::take(&mut facts))?)
                    }
                    "type" => {
                        ty.name = match core::mem::take(&mut tname) {
                            (Some(u), Some(l)) => Some(OwnedName::new(&u, &l)),
                            (None, None) => None,
                            _ => {
                                return Err(schema_err(
                                    "a type name without its URI or local name",
                                ));
                            }
                        };
                        m.types.push(core::mem::replace(
                            &mut ty,
                            TypeModel {
                                name: None,
                                castable: false,
                                casts: Vec::new(),
                                states: Vec::new(),
                            },
                        ));
                    }
                    "production" => {
                        let p = production(&prod)?;
                        ty.states
                            .last_mut()
                            .ok_or_else(|| {
                                schema_err("a production outside a state")
                            })?
                            .push(p);
                    }
                    "element" => {
                        let get = |k: &str| {
                            elem.iter().find(|(n, _)| n == k).map(|(_, v)| v)
                        };
                        let name = OwnedName::new(
                            &text(get("uri").ok_or_else(|| {
                                schema_err("an element without a URI")
                            })?)?,
                            &text(get("local").ok_or_else(|| {
                                schema_err("an element without a name")
                            })?)?,
                        );
                        let t = uint(get("type").ok_or_else(|| {
                            schema_err("an element without a type")
                        })?)?;
                        m.elements.push((
                            name,
                            t,
                            matches!(
                                get("nillable"),
                                Some(OwnedValue::Bool(true))
                            ),
                        ));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(m)
}

fn production(a: &[(String, OwnedValue)]) -> Result<Prod> {
    let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    let need = |k: &str| {
        get(k).ok_or_else(|| {
            schema_err("a production without an attribute its event needs")
        })
    };
    let next = || uint(need("next")?);
    let event = text(need("event")?)?;
    Ok(match event.as_str() {
        "AT" => Prod::At {
            name: OwnedName::new(&text(need("uri")?)?, &text(need("local")?)?),
            ty: SimpleTypeId(uint(need("type")?)?),
            next: next()?,
        },
        "ATuri" => {
            Prod::AtUri { uri: text(need("uri")?)?.into(), next: next()? }
        }
        "ATany" => Prod::AtAny { next: next()? },
        "SE" => Prod::Se {
            element: ElementId(uint(need("element")?)?),
            next: next()?,
        },
        "SEuri" => {
            Prod::SeUri { uri: text(need("uri")?)?.into(), next: next()? }
        }
        "SEany" => Prod::SeAny { next: next()? },
        "EE" => Prod::EE,
        "CH" => {
            Prod::Ch { ty: SimpleTypeId(uint(need("type")?)?), next: next()? }
        }
        _ => return Err(schema_err("an unknown event")),
    })
}

// =======================================================================================
// Generating the constant tables
// =======================================================================================

/// Rust source for [`exig_tables`](super::exig_tables): a function returning `m`.
#[cfg(all(test, feature = "xsd", feature = "std"))]
pub(crate) fn render_tables(m: &Model) -> String {
    use core::fmt::Write;
    let mut o = String::from(concat!(
        "// src/grammar/exig_tables.rs\n",
        "//\n",
        "// Generated EXIG schema grammar tables\n",
        "// Copyright © 2026 Arc Riley\n",
        "//\n",
        "// This file is free software and available for distribution by\n",
        "// the terms of the Apache 2.0 or MIT licenses. Code contrubutions\n",
        "// are welcome only if this notice remains unchanged clearly\n",
        "// communicating that changes are made under these same conditions.\n",
        "//\n",
    ));
    let q = |s: &str| format!("{s:?}");
    let name = |n: &OwnedName| format!("n({}, {})", q(&n.uri), q(&n.local));
    let _ = writeln!(
        o,
        "//! Generated from `schemas/exig-1.xsd` by"
    );
    let _ = writeln!(
        o,
        "//! `cargo test --features xsd exig_tables -- --ignored` (EXIG_REGEN=1 writes this file)."
    );
    let _ = writeln!(o, "//! Do not edit.");
    let _ = writeln!(o);
    let _ = writeln!(o, "use alloc::vec;");
    let _ = writeln!(o);
    let _ = writeln!(o, "use super::exig::{{Model, TypeModel}};");
    let _ = writeln!(
        o,
        "use super::informed::{{ElementId, IntegerRep, Prod, Representation, SimpleTypeId}};"
    );
    let _ = writeln!(o, "use crate::OwnedName;");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "fn n(uri: &str, local: &str) -> OwnedName {{\n    OwnedName::new(uri, local)\n}}"
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "#[allow(unused_imports)]");
    let _ = writeln!(o, "pub(crate) fn model() -> Model {{");
    let _ = writeln!(o, "    Model {{");
    let _ = writeln!(o, "        schema_id: None,");
    let _ = writeln!(o, "        strings: vec![");
    for (u, ls) in &m.strings {
        let _ = writeln!(
            o,
            "            ({}.into(), vec![{}]),",
            q(u),
            ls.iter()
                .map(|l| format!("{}.into()", q(l)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let _ = writeln!(o, "        ],");
    let _ = writeln!(o, "        simple: vec![");
    for r in &m.simple {
        let s = match r {
            Representation::String { charset: None } => {
                "Representation::String { charset: None }".to_string()
            }
            Representation::String { charset: Some(cs) } => format!(
                "Representation::String {{ charset: Some({}.chars().collect()) }}",
                q(&cs.iter().collect::<String>())
            ),
            Representation::Boolean { pattern } => {
                format!("Representation::Boolean {{ pattern: {pattern} }}")
            }
            Representation::Integer(IntegerRep::Signed) => {
                "Representation::Integer(IntegerRep::Signed)".into()
            }
            Representation::Integer(IntegerRep::Unsigned) => {
                "Representation::Integer(IntegerRep::Unsigned)".into()
            }
            Representation::Integer(IntegerRep::Bounded { min, max }) => {
                format!(
                    "Representation::Integer(IntegerRep::Bounded {{ min: {min}, max: {max} }})"
                )
            }
            Representation::Decimal => "Representation::Decimal".into(),
            Representation::Float => "Representation::Float".into(),
            Representation::Binary(k) => {
                format!("Representation::Binary(crate::BinaryKind::{k:?})")
            }
            Representation::DateTime(k) => {
                format!("Representation::DateTime(crate::DateTimeKind::{k:?})")
            }
            Representation::List(i) => {
                format!("Representation::List(SimpleTypeId({}))", i.0)
            }
            Representation::Enumeration { values, base } => format!(
                "Representation::Enumeration {{ values: vec![{}].into(), base: SimpleTypeId({}) }}",
                values
                    .iter()
                    .map(|v| format!("{}.into()", q(v)))
                    .collect::<Vec<_>>()
                    .join(", "),
                base.0
            ),
        };
        let _ = writeln!(o, "            {s},");
    }
    let _ = writeln!(o, "        ],");
    let _ = writeln!(o, "        types: vec![");
    for t in &m.types {
        let _ = writeln!(o, "            TypeModel {{");
        let _ = writeln!(
            o,
            "                name: {},",
            t.name
                .as_ref()
                .map_or("None".to_string(), |x| format!("Some({})", name(x)))
        );
        let _ = writeln!(o, "                castable: {},", t.castable);
        let _ = writeln!(o, "                casts: vec!{:?},", t.casts);
        let _ = writeln!(o, "                states: vec![");
        for st in &t.states {
            let ps: Vec<String> = st
                .iter()
                .map(|p| match p {
                    Prod::At { name: x, ty, next } => format!("Prod::At {{ name: {}, ty: SimpleTypeId({}), next: {next} }}", name(x), ty.0),
                    Prod::AtUri { uri, next } => format!("Prod::AtUri {{ uri: {}.into(), next: {next} }}", q(uri)),
                    Prod::AtAny { next } => format!("Prod::AtAny {{ next: {next} }}"),
                    Prod::Se { element, next } => format!("Prod::Se {{ element: ElementId({}), next: {next} }}", element.0),
                    Prod::SeUri { uri, next } => format!("Prod::SeUri {{ uri: {}.into(), next: {next} }}", q(uri)),
                    Prod::SeAny { next } => format!("Prod::SeAny {{ next: {next} }}"),
                    Prod::EE => "Prod::EE".into(),
                    Prod::Ch { ty, next } => format!("Prod::Ch {{ ty: SimpleTypeId({}), next: {next} }}", ty.0),
                })
                .collect();
            let _ =
                writeln!(o, "                    vec![{}],", ps.join(", "));
        }
        let _ = writeln!(o, "                ],");
        let _ = writeln!(o, "            }},");
    }
    let _ = writeln!(o, "        ],");
    let _ = writeln!(o, "        elements: vec![");
    for (x, t, nil) in &m.elements {
        let _ = writeln!(o, "            ({}, {t}, {nil}),", name(x));
    }
    let _ = writeln!(o, "        ],");
    let _ = writeln!(o, "        globals: vec!{:?},", m.globals);
    let _ = writeln!(o, "    }}");
    let _ = writeln!(o, "}}");
    o
}

#[cfg(all(test, feature = "xsd", feature = "std"))]
mod tests {
    use super::*;

    /// The committed tables are the EXIG schema compiled by this crate.
    /// With EXIG_REGEN=1 the file is written instead.
    #[test]
    fn exig_tables_are_current() {
        let set = GrammarSet::from_xsd(EXIG_SCHEMA.as_bytes()).unwrap();
        let src = render_tables(&Model::from_set(&set).unwrap());
        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/grammar/exig_tables.rs");
        if std::env::var("EXIG_REGEN").is_ok() {
            std::fs::write(path, &src).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            src,
            "run with EXIG_REGEN=1 to regenerate"
        );
    }

    /// The fixed point: the EXIG schema's own .exig, read with the
    /// constant tables and written again, is the same file; and equals
    /// what compiling the schema gives.
    #[test]
    fn exig_schema_fixed_point() {
        let compiled = GrammarSet::from_xsd(EXIG_SCHEMA.as_bytes()).unwrap();
        let constant = exig_grammars().unwrap();
        let file = compiled.to_exig().unwrap();
        assert_eq!(constant.to_exig().unwrap(), file);
        let back = GrammarSet::from_exig(&file).unwrap();
        assert_eq!(back.to_exig().unwrap(), file);
        std::println!(
            "EXIG schema .exig: {} octets, {}",
            file.len(),
            back.digest().unwrap()
        );
    }
}
