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
        // Named types first: the XML Schema built-in types by local
        // name, then the others by URI then local name.
        let mut named: Vec<usize> = (0..s.types.len())
            .filter(|&i| s.types[i].name.is_some())
            .collect();
        named.sort_by(|&a, &b| {
            let (x, y) = (
                s.types[a].name.as_ref().expect("named"),
                s.types[b].name.as_ref().expect("named"),
            );
            (&*x.uri != XS, &*x.uri, &*x.local).cmp(&(
                &*y.uri != XS,
                &*y.uri,
                &*y.local,
            ))
        });
        for &t in &named {
            w.tmap[t] = Some(w.torder.len() as u32);
            w.torder.push(t);
        }
        // Then a depth-first walk from the built-in types (so their
        // simple types come first too), the globals, then the other
        // named types.
        for &t in &named {
            if s.types[t].name.as_ref().is_some_and(|n| &*n.uri == XS) {
                w.ty(s, t);
            }
        }
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
// Names of representations in the EXIG vocabulary
// =======================================================================================

/// The XML Schema namespace: its types are the built-in ones.
const XS: &str = "http://www.w3.org/2001/XMLSchema";

/// Most productions an `.exig` may expand to, over all its types.
const MAX_PRODUCTIONS: usize = 1 << 24;

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

// =======================================================================================
// The document form
// =======================================================================================

/// An attribute use in the pool: name numbers, simple type, required.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AttrUse {
    uri: u32,
    local: u32,
    ty: u32,
    required: bool,
}

/// A production of a content state as written. `next: None` is the
/// state it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DProd {
    Se { element: u32, next: Option<u32> },
    SeSet { set: u32, next: Option<u32> },
    SeUri { uri: u32, next: Option<u32> },
    SeAny { next: Option<u32> },
    EE,
    Ch { ty: u32, next: Option<u32> },
    At { uri: u32, local: u32, ty: u32, next: Option<u32> },
    AtUri { uri: u32, next: Option<u32> },
    AtAny { next: Option<u32> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DContent {
    Empty,
    Scalar(u32),
    States(Vec<Vec<DProd>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DType {
    name: Option<(u32, u32)>,
    castable: bool,
    base: Option<u32>,
    /// Whether the shared attribute set is added, and the type's own
    /// pool numbers; `None` without attribute uses.
    attrs: Option<(bool, Vec<u32>)>,
    /// Attribute wildcards: `None` is AT(*), `Some(uri)` AT(uri:*).
    wild: Vec<Option<u32>>,
    content: DContent,
}

/// An EXIG document as written.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Doc {
    schema_id: Option<String>,
    builtin_types: bool,
    all_global: bool,
    strings: Vec<(String, Vec<String>)>,
    simple: Vec<Representation>,
    sets: Vec<Vec<u32>>,
    pool: Vec<AttrUse>,
    shared: Option<Vec<u32>>,
    types: Vec<DType>,
    elements: Vec<(u32, u32, u32, bool)>,
    globals: Vec<u32>,
}

/// The initial string table of a grammar set: names as numbers.
struct Names {
    uris: Vec<String>,
    locals: Vec<Vec<String>>,
}

impl Names {
    fn new(strings: &[(String, Vec<String>)]) -> Result<Names> {
        let mut b = InitialEntries::schema();
        for (u, ls) in strings {
            b = b.uri(u);
            for l in ls {
                b = b.local(u, l);
            }
        }
        let e = b.build()?;
        let uris: Vec<String> =
            e.uris().iter().map(|u| u.to_string()).collect();
        let locals = (0..uris.len() as u32)
            .map(|u| e.locals(u).iter().map(|l| l.to_string()).collect())
            .collect();
        Ok(Names { uris, locals })
    }

    fn uri(&self, u: &str) -> Result<u32> {
        self.uris
            .iter()
            .position(|x| x == u)
            .map(|i| i as u32)
            .ok_or_else(|| schema_err("a URI not in the string table"))
    }

    fn name(&self, n: &OwnedName) -> Result<(u32, u32)> {
        let u = self.uri(&n.uri)?;
        let l = self.locals[u as usize]
            .iter()
            .position(|x| **x == *n.local)
            .ok_or_else(|| schema_err("a name not in the string table"))?;
        Ok((u, l as u32))
    }

    fn uri_str(&self, u: u32) -> Result<&str> {
        self.uris
            .get(u as usize)
            .map(String::as_str)
            .ok_or_else(|| schema_err("a URI number out of range"))
    }

    fn get(&self, u: u32, l: u32) -> Result<OwnedName> {
        let local = self
            .locals
            .get(u as usize)
            .and_then(|ls| ls.get(l as usize))
            .ok_or_else(|| schema_err("a name number out of range"))?;
        Ok(OwnedName::new(&self.uris[u as usize], local))
    }

    /// The sort key of the attribute pool: local name, then URI.
    fn key(&self, u: u32, l: u32) -> (&str, &str) {
        (&self.locals[u as usize][l as usize], &self.uris[u as usize])
    }
}

/// The built-in types: the reference table every `builtinTypes`
/// document shares, taken from the EXIG schema's own grammars.
struct Builtins {
    simple: Vec<Representation>,
    /// The types without casts.
    types: Vec<TypeModel>,
    /// Their casts among themselves.
    casts: Vec<Vec<u32>>,
    base: Vec<Option<u32>>,
}

fn builtins() -> Result<Builtins> {
    let m = super::exig_tables::model();
    let n = m
        .types
        .iter()
        .take_while(|t| t.name.as_ref().is_some_and(|x| &*x.uri == XS))
        .count();
    let mut k = 0usize;
    for t in &m.types[..n] {
        for p in t.states.iter().flatten() {
            if let Prod::At { ty, .. } | Prod::Ch { ty, .. } = p {
                k = k.max(ty.0 as usize + 1);
            }
        }
    }
    let casts: Vec<Vec<u32>> = m.types[..n]
        .iter()
        .map(|t| {
            t.casts.iter().copied().filter(|&c| (c as usize) < n).collect()
        })
        .collect();
    let base = bases(&casts, &m.types[..n])?;
    let types = m.types[..n]
        .iter()
        .map(|t| TypeModel { casts: Vec::new(), ..t.clone() })
        .collect();
    Ok(Builtins { simple: m.simple[..k].to_vec(), types, casts, base })
}

/// Each named type's base: of the types that may be cast to it, the one
/// with the fewest casts. Fails unless the casts are exactly the
/// descendants under these bases.
fn bases(casts: &[Vec<u32>], types: &[TypeModel]) -> Result<Vec<Option<u32>>> {
    let n = casts.len();
    let mut base = alloc::vec![None; n];
    for (i, b) in base.iter_mut().enumerate() {
        if types[i].name.is_none() {
            continue;
        }
        *b = (0..n)
            .filter(|&a| casts[a].contains(&(i as u32)))
            .min_by_key(|&a| (casts[a].len(), a))
            .map(|a| a as u32);
    }
    if descendants(&base)? != casts {
        return Err(Error::new(
            ErrorKind::Arg,
            "xsi:type casts that do not follow one base per type have no .exig form",
        ));
    }
    Ok(base)
}

/// The descendants of every type under `base`, ascending.
fn descendants(base: &[Option<u32>]) -> Result<Vec<Vec<u32>>> {
    let n = base.len();
    let mut out = alloc::vec![Vec::new(); n];
    for i in 0..n {
        // Up the chain from i; a chain longer than n is a cycle.
        let mut a = base[i];
        let mut steps = 0;
        while let Some(b) = a {
            let b = b as usize;
            if b >= n || steps > n {
                return Err(schema_err(
                    "a base that does not exist or a cycle of bases",
                ));
            }
            out[b].push(i as u32);
            a = base[b];
            steps += 1;
        }
    }
    for d in &mut out {
        d.sort_unstable();
    }
    Ok(out)
}

/// `p` with its next state replaced.
fn with_next(p: &Prod, n: u32) -> Prod {
    let mut p = p.clone();
    match &mut p {
        Prod::At { next, .. }
        | Prod::AtUri { next, .. }
        | Prod::AtAny { next }
        | Prod::Se { next, .. }
        | Prod::SeUri { next, .. }
        | Prod::SeAny { next }
        | Prod::Ch { next, .. } => *next = n,
        Prod::EE => {}
    }
    p
}

fn next_of(p: &Prod) -> Option<u32> {
    match p {
        Prod::At { next, .. }
        | Prod::AtUri { next, .. }
        | Prod::AtAny { next }
        | Prod::Se { next, .. }
        | Prod::SeUri { next, .. }
        | Prod::SeAny { next }
        | Prod::Ch { next, .. } => Some(*next),
        Prod::EE => None,
    }
}

fn is_at(p: &Prod) -> bool {
    matches!(p, Prod::At { .. } | Prod::AtUri { .. } | Prod::AtAny { .. })
}

fn is_wild(p: &Prod) -> bool {
    matches!(p, Prod::AtUri { .. } | Prod::AtAny { .. })
}

/// A type grammar as attribute uses, wildcards and content states.
#[derive(Clone)]
struct Shape {
    attrs: Vec<(OwnedName, SimpleTypeId, bool)>,
    wild: Vec<Option<Box<str>>>,
    /// Content states; `next` numbers content states.
    content: Vec<Vec<Prod>>,
}

/// The states of a shape, numbered breadth-first from state 0 (the
/// schema's rules for attribute states).
fn expand(s: &Shape) -> Result<Vec<Vec<Prod>>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Node {
        A(usize),
        C(usize),
    }
    let k = s.attrs.len();
    let nattr = k + usize::from(!s.wild.is_empty());
    if s.content.is_empty() {
        return Err(schema_err("a type with no content state"));
    }
    for p in s.content.iter().flatten() {
        if next_of(p).is_some_and(|n| n as usize >= s.content.len()) {
            return Err(schema_err(
                "a production to a state that does not exist",
            ));
        }
    }
    let prods = |n: Node| -> Vec<(Prod, Option<Node>)> {
        let content = |j: usize| {
            s.content[j]
                .iter()
                .map(|p| (p.clone(), next_of(p).map(|t| Node::C(t as usize))))
        };
        match n {
            Node::C(j) => content(j).collect(),
            Node::A(i) => {
                let mut v = Vec::new();
                let mut j = i;
                while j < k {
                    let (name, ty, req) = &s.attrs[j];
                    let to = if j + 1 < nattr {
                        Node::A(j + 1)
                    } else {
                        Node::C(0)
                    };
                    v.push((
                        Prod::At { name: name.clone(), ty: *ty, next: 0 },
                        Some(to),
                    ));
                    if *req {
                        break;
                    }
                    j += 1;
                }
                for w in &s.wild {
                    let p = match w {
                        None => Prod::AtAny { next: 0 },
                        Some(u) => Prod::AtUri { uri: u.clone(), next: 0 },
                    };
                    v.push((p, Some(Node::A(i))));
                }
                if !s.attrs[i..].iter().any(|a| a.2) {
                    v.extend(content(0));
                }
                v
            }
        }
    };
    let first = if nattr > 0 { Node::A(0) } else { Node::C(0) };
    let mut order = alloc::vec![first];
    let mut i = 0;
    while i < order.len() {
        for (_, to) in prods(order[i]) {
            if let Some(to) = to {
                if !order.contains(&to) {
                    order.push(to);
                }
            }
        }
        i += 1;
    }
    let num =
        |n: Node| order.iter().position(|&x| x == n).expect("ordered") as u32;
    Ok(order
        .iter()
        .map(|&n| {
            prods(n)
                .into_iter()
                .map(|(p, to)| match to {
                    Some(to) => with_next(&p, num(to)),
                    None => p,
                })
                .collect()
        })
        .collect())
}

/// The shape of a type grammar: attribute uses and content when the
/// states follow the schema's rules, otherwise every state as content.
fn shape(states: &[Vec<Prod>]) -> Shape {
    let explicit = Shape {
        attrs: Vec::new(),
        wild: Vec::new(),
        content: states.to_vec(),
    };
    let mut attrs: Vec<(OwnedName, SimpleTypeId, bool)> = Vec::new();
    while attrs.len() < states.len() {
        match states[attrs.len()].first() {
            Some(Prod::At { name, ty, next })
                if *next as usize == attrs.len() + 1 =>
            {
                attrs.push((name.clone(), *ty, false))
            }
            _ => break,
        }
    }
    let wild: Vec<Option<Box<str>>> = states[0]
        .iter()
        .filter_map(|p| match p {
            Prod::AtAny { .. } => Some(None),
            Prod::AtUri { uri, .. } => Some(Some(uri.clone())),
            _ => None,
        })
        .collect();
    let k = attrs.len();
    let nattr = k + usize::from(!wild.is_empty());
    if k >= states.len() {
        return explicit;
    }
    // A state that does not offer the content start ends at a required
    // use.
    for (i, st) in states[..k].iter().enumerate() {
        let ats = st.iter().filter(|p| matches!(p, Prod::At { .. })).count();
        let last = i + ats - 1;
        if last >= k {
            return explicit;
        }
        if last + 1 < k || st.iter().all(is_at) {
            attrs[last].2 = true;
        }
    }
    // The content start.
    let (start, c0) = if wild.is_empty() {
        (Some(k), states[k].clone())
    } else {
        let c0: Vec<Prod> =
            states[k].iter().filter(|p| !is_wild(p)).cloned().collect();
        ((nattr..states.len()).find(|&i| states[i] == c0), c0)
    };
    // Content states, breadth-first from the start.
    let mut ids: Vec<Option<u32>> = alloc::vec![None; states.len()];
    if let Some(s) = start {
        ids[s] = Some(0);
    }
    let mut order: Vec<Vec<Prod>> = alloc::vec![c0];
    let mut i = 0;
    while i < order.len() {
        let targets: Vec<u32> = order[i].iter().filter_map(next_of).collect();
        for t in targets {
            let t = t as usize;
            if t < nattr || t >= states.len() {
                return explicit;
            }
            if ids[t].is_none() {
                ids[t] = Some(order.len() as u32);
                order.push(states[t].clone());
            }
        }
        i += 1;
    }
    if (nattr..states.len()).any(|i| ids[i].is_none()) {
        return explicit;
    }
    let content = order
        .iter()
        .map(|st| {
            st.iter()
                .map(|p| match next_of(p) {
                    Some(t) => {
                        with_next(p, ids[t as usize].expect("numbered"))
                    }
                    None => p.clone(),
                })
                .collect()
        })
        .collect();
    let s = Shape { attrs, wild, content };
    match expand(&s) {
        Ok(back) if back == states => s,
        _ => explicit,
    }
}

impl Doc {
    /// The document form of a model in canonical order.
    fn from_model(m: &Model) -> Result<Doc> {
        let names = Names::new(&m.strings)?;
        let bi = builtins()?;
        let (nb, ns) = (bi.types.len(), bi.simple.len());
        let casts: Vec<Vec<u32>> =
            m.types.iter().map(|t| t.casts.clone()).collect();
        let base = bases(&casts, &m.types)?;
        let builtin = m.types.len() >= nb
            && m.simple.len() >= ns
            && m.simple[..ns] == bi.simple[..]
            && (0..nb).all(|i| {
                let t = &m.types[i];
                t.name == bi.types[i].name
                    && t.castable == bi.types[i].castable
                    && t.states == bi.types[i].states
                    && t.casts
                        .iter()
                        .copied()
                        .filter(|&c| (c as usize) < nb)
                        .collect::<Vec<_>>()
                        == bi.casts[i]
            });
        let (t0, s0) = if builtin { (nb, ns) } else { (0, 0) };
        let mut d = Doc {
            schema_id: m.schema_id.clone(),
            builtin_types: builtin,
            strings: m.strings.clone(),
            simple: m.simple[s0..].to_vec(),
            ..Doc::default()
        };

        // Shapes, and the attribute pool: a type whose uses are not in
        // pool order is written state by state.
        let mut shapes: Vec<Shape> =
            m.types[t0..].iter().map(|t| shape(&t.states)).collect();
        let use_of = |a: &(OwnedName, SimpleTypeId, bool)| -> Result<AttrUse> {
            let (uri, local) = names.name(&a.0)?;
            Ok(AttrUse { uri, local, ty: a.1.0, required: a.2 })
        };
        let pool = |shapes: &[Shape]| -> Result<Vec<AttrUse>> {
            let mut pool: Vec<AttrUse> = Vec::new();
            for a in shapes.iter().flat_map(|s| s.attrs.iter()) {
                let u = use_of(a)?;
                if !pool.contains(&u) {
                    pool.push(u);
                }
            }
            pool.sort_by(|a, b| {
                (names.key(a.uri, a.local), a.ty, a.required).cmp(&(
                    names.key(b.uri, b.local),
                    b.ty,
                    b.required,
                ))
            });
            Ok(pool)
        };
        let numbers = |pool: &[AttrUse], s: &Shape| -> Result<Vec<u32>> {
            s.attrs
                .iter()
                .map(|a| {
                    let u = use_of(a)?;
                    Ok(pool.iter().position(|x| *x == u).expect("pooled")
                        as u32)
                })
                .collect()
        };
        let p = pool(&shapes)?;
        for (i, s) in shapes.iter_mut().enumerate() {
            let ns = numbers(&p, s)?;
            if ns.windows(2).any(|w| w[0] >= w[1]) {
                *s = Shape {
                    attrs: Vec::new(),
                    wild: Vec::new(),
                    content: m.types[t0 + i].states.clone(),
                };
            }
        }
        d.pool = pool(&shapes)?;
        let lists: Vec<Vec<u32>> = shapes
            .iter()
            .map(|s| numbers(&d.pool, s))
            .collect::<Result<_>>()?;
        // The shared set: the uses the most types have, when that pays.
        let mut count = alloc::vec![0usize; d.pool.len()];
        for l in &lists {
            for &u in l {
                count[u as usize] += 1;
            }
        }
        let top = count.iter().copied().max().unwrap_or(0);
        let shared: Vec<u32> = (0..count.len() as u32)
            .filter(|&u| count[u as usize] == top)
            .collect();
        if top >= 3 && shared.len() >= 2 {
            d.shared = Some(shared);
        }

        for (i, (s, l)) in shapes.iter().zip(&lists).enumerate() {
            let t = &m.types[t0 + i];
            let attrs = if l.is_empty() {
                None
            } else {
                match &d.shared {
                    Some(sh) if sh.iter().all(|u| l.contains(u)) => Some((
                        true,
                        l.iter()
                            .copied()
                            .filter(|u| !sh.contains(u))
                            .collect(),
                    )),
                    _ => Some((false, l.clone())),
                }
            };
            let wild = s
                .wild
                .iter()
                .map(|w| w.as_deref().map(|u| names.uri(u)).transpose())
                .collect::<Result<_>>()?;
            let content = d.content(&s.content, &names)?;
            d.types.push(DType {
                name: t.name.as_ref().map(|n| names.name(n)).transpose()?,
                castable: t.castable,
                base: base[t0 + i],
                attrs,
                wild,
                content,
            });
        }
        for (name, ty, nil) in &m.elements {
            let (u, l) = names.name(name)?;
            d.elements.push((u, l, *ty, *nil));
        }
        let order = global_order(&m.elements);
        if !m.globals.is_empty() && m.globals == order {
            d.all_global = true;
        } else {
            d.globals = m.globals.clone();
        }
        Ok(d)
    }

    /// Content states as written: element sets, `next` left out for the
    /// state itself, and the empty and scalar forms.
    fn content(
        &mut self,
        states: &[Vec<Prod>],
        names: &Names,
    ) -> Result<DContent> {
        match states {
            [s] if *s == [Prod::EE] => return Ok(DContent::Empty),
            [a, b] if *b == [Prod::EE] => {
                if let [Prod::Ch { ty, next: 1 }] = a[..] {
                    return Ok(DContent::Scalar(ty.0));
                }
            }
            _ => {}
        }
        let mut out = Vec::with_capacity(states.len());
        for (si, st) in states.iter().enumerate() {
            let nx = |n: u32| (n as usize != si).then_some(n);
            let mut ps = Vec::new();
            let mut i = 0;
            while i < st.len() {
                if let Prod::Se { next, .. } = st[i] {
                    let mut j = i;
                    while j < st.len()
                        && matches!(st[j], Prod::Se { next: n, .. } if n == next)
                    {
                        j += 1;
                    }
                    if j - i >= 3 {
                        let set: Vec<u32> = st[i..j]
                            .iter()
                            .map(|p| match p {
                                Prod::Se { element, .. } => element.0,
                                _ => unreachable!(),
                            })
                            .collect();
                        let n = match self.sets.iter().position(|s| *s == set)
                        {
                            Some(n) => n,
                            None => {
                                self.sets.push(set);
                                self.sets.len() - 1
                            }
                        };
                        ps.push(DProd::SeSet {
                            set: n as u32,
                            next: nx(next),
                        });
                        i = j;
                        continue;
                    }
                }
                ps.push(match &st[i] {
                    Prod::Se { element, next } => {
                        DProd::Se { element: element.0, next: nx(*next) }
                    }
                    Prod::SeUri { uri, next } => {
                        DProd::SeUri { uri: names.uri(uri)?, next: nx(*next) }
                    }
                    Prod::SeAny { next } => DProd::SeAny { next: nx(*next) },
                    Prod::EE => DProd::EE,
                    Prod::Ch { ty, next } => {
                        DProd::Ch { ty: ty.0, next: nx(*next) }
                    }
                    Prod::At { name, ty, next } => {
                        let (uri, local) = names.name(name)?;
                        DProd::At { uri, local, ty: ty.0, next: nx(*next) }
                    }
                    Prod::AtUri { uri, next } => {
                        DProd::AtUri { uri: names.uri(uri)?, next: nx(*next) }
                    }
                    Prod::AtAny { next } => DProd::AtAny { next: nx(*next) },
                });
                i += 1;
            }
            out.push(ps);
        }
        Ok(DContent::States(out))
    }

    /// The model this document describes; [`Model::build`] checks it.
    fn to_model(&self) -> Result<Model> {
        let names = Names::new(&self.strings)?;
        let mut m = Model {
            schema_id: self.schema_id.clone(),
            strings: self.strings.clone(),
            ..Model::default()
        };
        let mut base: Vec<Option<u32>> = Vec::new();
        if self.builtin_types {
            let bi = builtins()?;
            m.simple = bi.simple;
            m.types = bi.types;
            base = bi.base;
        }
        m.simple.extend(self.simple.iter().cloned());
        let shared = self.shared.as_deref().unwrap_or(&[]);
        let mut total = 0usize;
        for t in &self.types {
            let mut uses: Vec<u32> = match &t.attrs {
                None => Vec::new(),
                Some((with_set, own)) => {
                    if *with_set && self.shared.is_none() {
                        return Err(schema_err(
                            "a type uses an attribute set that is not given",
                        ));
                    }
                    let mut l = own.clone();
                    if *with_set {
                        l.extend_from_slice(shared);
                    }
                    l
                }
            };
            uses.sort_unstable();
            if uses.windows(2).any(|w| w[0] == w[1]) {
                return Err(schema_err("an attribute use given twice"));
            }
            let attrs = uses
                .iter()
                .map(|&u| {
                    let a = self.pool.get(u as usize).ok_or_else(|| {
                        schema_err("an attribute use that does not exist")
                    })?;
                    Ok((
                        names.get(a.uri, a.local)?,
                        SimpleTypeId(a.ty),
                        a.required,
                    ))
                })
                .collect::<Result<_>>()?;
            let wild = t
                .wild
                .iter()
                .map(|w| {
                    w.map(|u| names.uri_str(u).map(Box::from)).transpose()
                })
                .collect::<Result<_>>()?;
            let content = match &t.content {
                DContent::Empty => alloc::vec![alloc::vec![Prod::EE]],
                DContent::Scalar(ty) => alloc::vec![
                    alloc::vec![Prod::Ch { ty: SimpleTypeId(*ty), next: 1 }],
                    alloc::vec![Prod::EE],
                ],
                DContent::States(ss) => {
                    let mut out = Vec::with_capacity(ss.len());
                    for (si, st) in ss.iter().enumerate() {
                        let nx = |n: Option<u32>| n.unwrap_or(si as u32);
                        let mut ps = Vec::new();
                        for p in st {
                            match p {
                                DProd::SeSet { set, next } => {
                                    let set = self
                                        .sets
                                        .get(*set as usize)
                                        .ok_or_else(|| schema_err("an element set that does not exist"))?;
                                    total += set.len();
                                    if total > MAX_PRODUCTIONS {
                                        return Err(Error::new(
                                            ErrorKind::Limit,
                                            "an .exig with too many productions",
                                        ));
                                    }
                                    ps.extend(set.iter().map(|&e| Prod::Se {
                                        element: ElementId(e),
                                        next: nx(*next),
                                    }));
                                }
                                DProd::Se { element, next } => {
                                    ps.push(Prod::Se {
                                        element: ElementId(*element),
                                        next: nx(*next),
                                    })
                                }
                                DProd::SeUri { uri, next } => {
                                    ps.push(Prod::SeUri {
                                        uri: names.uri_str(*uri)?.into(),
                                        next: nx(*next),
                                    })
                                }
                                DProd::SeAny { next } => {
                                    ps.push(Prod::SeAny { next: nx(*next) })
                                }
                                DProd::EE => ps.push(Prod::EE),
                                DProd::Ch { ty, next } => ps.push(Prod::Ch {
                                    ty: SimpleTypeId(*ty),
                                    next: nx(*next),
                                }),
                                DProd::At { uri, local, ty, next } => {
                                    ps.push(Prod::At {
                                        name: names.get(*uri, *local)?,
                                        ty: SimpleTypeId(*ty),
                                        next: nx(*next),
                                    })
                                }
                                DProd::AtUri { uri, next } => {
                                    ps.push(Prod::AtUri {
                                        uri: names.uri_str(*uri)?.into(),
                                        next: nx(*next),
                                    })
                                }
                                DProd::AtAny { next } => {
                                    ps.push(Prod::AtAny { next: nx(*next) })
                                }
                            }
                        }
                        out.push(ps);
                    }
                    out
                }
            };
            let states = expand(&Shape { attrs, wild, content })?;
            total += states.iter().map(Vec::len).sum::<usize>();
            if total > MAX_PRODUCTIONS {
                return Err(Error::new(
                    ErrorKind::Limit,
                    "an .exig with too many productions",
                ));
            }
            m.types.push(TypeModel {
                name: t.name.map(|(u, l)| names.get(u, l)).transpose()?,
                castable: t.castable,
                casts: Vec::new(),
                states,
            });
            base.push(t.base);
        }
        for (i, b) in base.iter().enumerate() {
            if b.is_some() && m.types[i].name.is_none() {
                return Err(schema_err("an anonymous type with a base"));
            }
        }
        for (t, casts) in m.types.iter_mut().zip(descendants(&base)?) {
            t.casts = casts;
        }
        for &(u, l, ty, nil) in &self.elements {
            m.elements.push((names.get(u, l)?, ty, nil));
        }
        m.globals = if self.all_global {
            global_order(&m.elements)
        } else {
            self.globals.clone()
        };
        Ok(m)
    }
}

/// Every element declaration, by local name then URI: the order of
/// global elements.
fn global_order(elements: &[(OwnedName, u32, bool)]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..elements.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (&elements[a as usize].0, &elements[b as usize].0);
        (&*x.local, &*x.uri).cmp(&(&*y.local, &*y.uri))
    });
    order
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
    /// save, and for xsi:type casts that do not follow one base per
    /// type.
    pub fn to_exig(&self) -> Result<Vec<u8>> {
        let d = Doc::from_model(&Model::from_set(self)?)?;
        with_exig(|g| write_doc(g, &d))
    }

    /// Reads an `.exig` file and checks it (C.6) under default limits.
    pub fn from_exig(exig: &[u8]) -> Result<GrammarSet> {
        Self::from_exig_with(exig, Limits::new())
    }

    /// [`from_exig`](Self::from_exig) under `limits`.
    pub fn from_exig_with(exig: &[u8], limits: Limits) -> Result<GrammarSet> {
        let d = with_exig(|g| read_doc(g, exig, limits))?;
        d.to_model()?.build()
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

fn numbers(v: &[u32]) -> OwnedValue {
    OwnedValue::List(v.iter().map(|&x| OwnedValue::Int(x as i64)).collect())
}

/// Writes the EXIG document `d` with the EXIG grammars `g`.
fn write_doc(g: &GrammarSet, d: &Doc) -> Result<Vec<u8>> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let grammars = g.attach(&strings)?;
    let cfg = WriterConfig::new(exig_options(), &grammars)
        .header(Header::WithOptions { cookie: false });
    let mut w = Writer::new(Vec::new(), cfg)?;
    type W<'a> = Writer<'a, Vec<u8>>;
    let el = |w: &mut W<'_>, local: &str| w.start_element((EXIG_NS, local));
    let int = |w: &mut W<'_>, name: &str, v: u32| {
        w.attribute(name, OwnedValue::Int(v as i64).as_value())
    };
    let next = |w: &mut W<'_>, n: Option<u32>| -> Result<()> {
        match n {
            Some(n) => int(w, "next", n),
            None => Ok(()),
        }
    };
    let leaf = |w: &mut W<'_>, local: &str, v: OwnedValue| -> Result<()> {
        w.start_element((EXIG_NS, local))?;
        w.text(v.as_value())?;
        w.end_element()
    };
    el(&mut w, "grammars")?;
    if d.all_global {
        w.attribute("allGlobal", true)?;
    }
    if d.builtin_types {
        w.attribute("builtinTypes", true)?;
    }
    if let Some(id) = &d.schema_id {
        w.attribute("schemaId", id.as_str())?;
    }
    el(&mut w, "strings")?;
    for (u, locals) in &d.strings {
        el(&mut w, "uri")?;
        w.attribute("value", u.as_str())?;
        for l in locals {
            leaf(&mut w, "local", OwnedValue::Str(l.as_str().into()))?;
        }
        w.end_element()?;
    }
    w.end_element()?;
    el(&mut w, "simpleTypes")?;
    for r in &d.simple {
        el(&mut w, rep_name(r))?;
        match r {
            Representation::Integer(IntegerRep::Bounded { min, max }) => {
                w.attribute("maxInclusive", OwnedValue::Int(*max).as_value())?;
                w.attribute("minInclusive", OwnedValue::Int(*min).as_value())?;
            }
            Representation::List(i) => int(&mut w, "itemType", i.0)?,
            Representation::Enumeration { base, values } => {
                int(&mut w, "base", base.0)?;
                for v in values.iter() {
                    leaf(&mut w, "value", OwnedValue::Str(v.clone()))?;
                }
            }
            Representation::String { charset: Some(cs) } => leaf(
                &mut w,
                "charset",
                OwnedValue::Str(cs.iter().collect::<String>().into()),
            )?,
            _ => {}
        }
        w.end_element()?;
    }
    w.end_element()?;
    el(&mut w, "elementSets")?;
    for s in &d.sets {
        leaf(&mut w, "set", numbers(s))?;
    }
    w.end_element()?;
    el(&mut w, "attributes")?;
    for a in &d.pool {
        el(&mut w, "attribute")?;
        int(&mut w, "local", a.local)?;
        if a.required {
            w.attribute("required", true)?;
        }
        int(&mut w, "type", a.ty)?;
        int(&mut w, "uri", a.uri)?;
        w.end_element()?;
    }
    w.end_element()?;
    if let Some(s) = &d.shared {
        leaf(&mut w, "attributeSet", numbers(s))?;
    }
    el(&mut w, "types")?;
    for t in &d.types {
        el(&mut w, "type")?;
        if let Some(b) = t.base {
            int(&mut w, "base", b)?;
        }
        if t.castable {
            w.attribute("castable", true)?;
        }
        if let Some((u, l)) = t.name {
            int(&mut w, "local", l)?;
            int(&mut w, "uri", u)?;
        }
        if let Some((with_set, own)) = &t.attrs {
            el(&mut w, "attributes")?;
            if *with_set {
                int(&mut w, "set", 0)?;
            }
            w.text(numbers(own).as_value())?;
            w.end_element()?;
        }
        for wc in &t.wild {
            el(&mut w, "anyAttribute")?;
            if let Some(u) = wc {
                int(&mut w, "uri", *u)?;
            }
            w.end_element()?;
        }
        match &t.content {
            DContent::Empty => {
                el(&mut w, "empty")?;
                w.end_element()?;
            }
            DContent::Scalar(ty) => {
                el(&mut w, "scalar")?;
                int(&mut w, "type", *ty)?;
                w.end_element()?;
            }
            DContent::States(ss) => {
                el(&mut w, "content")?;
                for st in ss {
                    el(&mut w, "state")?;
                    for p in st {
                        match p {
                            DProd::Se { element, next: n } => {
                                el(&mut w, "se")?;
                                int(&mut w, "element", *element)?;
                                next(&mut w, *n)?;
                            }
                            DProd::SeSet { set, next: n } => {
                                el(&mut w, "seSet")?;
                                next(&mut w, *n)?;
                                int(&mut w, "set", *set)?;
                            }
                            DProd::SeUri { uri, next: n } => {
                                el(&mut w, "seUri")?;
                                next(&mut w, *n)?;
                                int(&mut w, "uri", *uri)?;
                            }
                            DProd::SeAny { next: n } => {
                                el(&mut w, "seAny")?;
                                next(&mut w, *n)?;
                            }
                            DProd::EE => el(&mut w, "ee")?,
                            DProd::Ch { ty, next: n } => {
                                el(&mut w, "ch")?;
                                next(&mut w, *n)?;
                                int(&mut w, "type", *ty)?;
                            }
                            DProd::At { uri, local, ty, next: n } => {
                                el(&mut w, "at")?;
                                int(&mut w, "local", *local)?;
                                next(&mut w, *n)?;
                                int(&mut w, "type", *ty)?;
                                int(&mut w, "uri", *uri)?;
                            }
                            DProd::AtUri { uri, next: n } => {
                                el(&mut w, "atUri")?;
                                next(&mut w, *n)?;
                                int(&mut w, "uri", *uri)?;
                            }
                            DProd::AtAny { next: n } => {
                                el(&mut w, "atAny")?;
                                next(&mut w, *n)?;
                            }
                        }
                        w.end_element()?;
                    }
                    w.end_element()?;
                }
                w.end_element()?;
            }
        }
        w.end_element()?;
    }
    w.end_element()?;
    el(&mut w, "elements")?;
    for &(u, l, ty, nil) in &d.elements {
        el(&mut w, "element")?;
        int(&mut w, "local", l)?;
        if nil {
            w.attribute("nillable", true)?;
        }
        int(&mut w, "type", ty)?;
        int(&mut w, "uri", u)?;
        w.end_element()?;
    }
    w.end_element()?;
    for &gl in &d.globals {
        leaf(&mut w, "global", OwnedValue::Int(gl as i64))?;
    }
    w.end_element()?;
    w.end_document()?;
    Ok(w.into_sink())
}

/// An element of a decoded EXIG document.
#[derive(Default)]
struct Node {
    name: String,
    attrs: Vec<(String, OwnedValue)>,
    text: Option<OwnedValue>,
    kids: Vec<Node>,
}

impl Node {
    fn get(&self, k: &str) -> Option<&OwnedValue> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    fn uint(&self, k: &str) -> Result<u32> {
        uint(self.get(k).ok_or_else(|| schema_err("an attribute missing"))?)
    }
    fn opt(&self, k: &str) -> Result<Option<u32>> {
        self.get(k).map(uint).transpose()
    }
    fn flag(&self, k: &str) -> bool {
        matches!(self.get(k), Some(OwnedValue::Bool(true)))
    }
    fn kid(&self, k: &str) -> Option<&Node> {
        self.kids.iter().find(|n| n.name == k)
    }
    fn numbers(&self) -> Result<Vec<u32>> {
        match &self.text {
            None => Ok(Vec::new()),
            Some(OwnedValue::List(items)) => items.iter().map(uint).collect(),
            Some(_) => Err(schema_err("a list of numbers expected")),
        }
    }
}

/// Reads an EXIG document; [`Doc::to_model`] and [`Model::build`] check
/// it.
fn read_doc(g: &GrammarSet, exig: &[u8], limits: Limits) -> Result<Doc> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let grammars = g.attach(&strings)?;
    let mut r: Reader =
        Reader::new(ReaderConfig::new(&grammars).limits(limits))?;
    // The strict EXIG grammars hold the document to the EXIG schema, so
    // this only collects it into a tree.
    let mut stack: Vec<Node> = alloc::vec![Node::default()];
    let mut rest = exig;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        match r.next()? {
            Step::Event(e, _) => match e.to_owned() {
                OwnedEvent::StartElement(n) => stack.push(Node {
                    name: n.as_name().local_name().to_string(),
                    ..Node::default()
                }),
                OwnedEvent::Attribute(n, v) => stack
                    .last_mut()
                    .expect("open")
                    .attrs
                    .push((n.as_name().local_name().to_string(), v)),
                OwnedEvent::Characters(v) => {
                    stack.last_mut().expect("open").text = Some(v)
                }
                OwnedEvent::EndElement(_) => {
                    let done = stack.pop().expect("open");
                    stack
                        .last_mut()
                        .ok_or_else(|| schema_err("unbalanced"))?
                        .kids
                        .push(done);
                }
                _ => {}
            },
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
    let root = stack
        .pop()
        .and_then(|mut n| n.kids.pop())
        .ok_or_else(|| schema_err("an empty .exig"))?;
    doc_of(&root)
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

fn node_text(n: &Node) -> Result<String> {
    n.text.as_ref().map_or(Ok(String::new()), text)
}

fn doc_of(root: &Node) -> Result<Doc> {
    let mut d = Doc {
        schema_id: root.get("schemaId").map(text).transpose()?,
        builtin_types: root.flag("builtinTypes"),
        all_global: root.flag("allGlobal"),
        ..Doc::default()
    };
    for k in &root.kids {
        match k.name.as_str() {
            "strings" => {
                for u in &k.kids {
                    let uri = text(u.get("value").ok_or_else(|| {
                        schema_err("a URI without a value")
                    })?)?;
                    let locals =
                        u.kids.iter().map(node_text).collect::<Result<_>>()?;
                    d.strings.push((uri, locals));
                }
            }
            "simpleTypes" => {
                for s in &k.kids {
                    d.simple.push(simple_of(s)?);
                }
            }
            "elementSets" => {
                for s in &k.kids {
                    d.sets.push(s.numbers()?);
                }
            }
            "attributes" => {
                for a in &k.kids {
                    d.pool.push(AttrUse {
                        uri: a.uint("uri")?,
                        local: a.uint("local")?,
                        ty: a.uint("type")?,
                        required: a.flag("required"),
                    });
                }
            }
            "attributeSet" => d.shared = Some(k.numbers()?),
            "types" => {
                for t in &k.kids {
                    d.types.push(type_of(t)?);
                }
            }
            "elements" => {
                for e in &k.kids {
                    d.elements.push((
                        e.uint("uri")?,
                        e.uint("local")?,
                        e.uint("type")?,
                        e.flag("nillable"),
                    ));
                }
            }
            "global" => d.globals.push(uint(
                k.text
                    .as_ref()
                    .ok_or_else(|| schema_err("an empty global"))?,
            )?),
            _ => {}
        }
    }
    Ok(d)
}

fn simple_of(s: &Node) -> Result<Representation> {
    use DateTimeKind::*;
    let bad = || {
        schema_err("a simple type whose facets do not fit its representation")
    };
    let long = |k: &str| match s.get(k) {
        Some(OwnedValue::Int(i)) => Ok(*i),
        _ => Err(bad()),
    };
    Ok(match s.name.as_str() {
        "string" => Representation::String {
            charset: s
                .kid("charset")
                .map(|c| node_text(c).map(|t| t.chars().collect()))
                .transpose()?,
        },
        "boolean" => Representation::Boolean { pattern: false },
        "booleanPattern" => Representation::Boolean { pattern: true },
        "integer" => Representation::Integer(IntegerRep::Signed),
        "unsignedInteger" => Representation::Integer(IntegerRep::Unsigned),
        "boundedInteger" => Representation::Integer(IntegerRep::Bounded {
            min: long("minInclusive")?,
            max: long("maxInclusive")?,
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
        "list" => Representation::List(SimpleTypeId(s.uint("itemType")?)),
        "enumeration" => Representation::Enumeration {
            values: s
                .kids
                .iter()
                .map(|v| node_text(v).map(Box::from))
                .collect::<Result<Vec<Box<str>>>>()?
                .into(),
            base: SimpleTypeId(s.uint("base")?),
        },
        _ => return Err(bad()),
    })
}

fn type_of(t: &Node) -> Result<DType> {
    let name = match (t.opt("uri")?, t.opt("local")?) {
        (Some(u), Some(l)) => Some((u, l)),
        (None, None) => None,
        _ => {
            return Err(schema_err(
                "a type name without its URI or local name",
            ));
        }
    };
    let mut d = DType {
        name,
        castable: t.flag("castable"),
        base: t.opt("base")?,
        attrs: None,
        wild: Vec::new(),
        content: DContent::Empty,
    };
    for k in &t.kids {
        match k.name.as_str() {
            "attributes" => {
                d.attrs = Some((k.get("set").is_some(), k.numbers()?))
            }
            "anyAttribute" => d.wild.push(k.opt("uri")?),
            "empty" => d.content = DContent::Empty,
            "scalar" => d.content = DContent::Scalar(k.uint("type")?),
            "content" => {
                let mut states = Vec::with_capacity(k.kids.len());
                for s in &k.kids {
                    let mut ps = Vec::with_capacity(s.kids.len());
                    for p in &s.kids {
                        let next = p.opt("next")?;
                        ps.push(match p.name.as_str() {
                            "se" => {
                                DProd::Se { element: p.uint("element")?, next }
                            }
                            "seSet" => {
                                DProd::SeSet { set: p.uint("set")?, next }
                            }
                            "seUri" => {
                                DProd::SeUri { uri: p.uint("uri")?, next }
                            }
                            "seAny" => DProd::SeAny { next },
                            "ee" => DProd::EE,
                            "ch" => DProd::Ch { ty: p.uint("type")?, next },
                            "at" => DProd::At {
                                uri: p.uint("uri")?,
                                local: p.uint("local")?,
                                ty: p.uint("type")?,
                                next,
                            },
                            "atUri" => {
                                DProd::AtUri { uri: p.uint("uri")?, next }
                            }
                            "atAny" => DProd::AtAny { next },
                            _ => {
                                return Err(schema_err(
                                    "an unknown production",
                                ));
                            }
                        });
                    }
                    states.push(ps);
                }
                d.content = DContent::States(states);
            }
            _ => {}
        }
    }
    if let Some((true, _)) = &d.attrs {
        // `set` names the one shared set.
        if t.kid("attributes")
            .and_then(|a| a.get("set"))
            .map(uint)
            .transpose()?
            != Some(0)
        {
            return Err(schema_err("an attribute set that does not exist"));
        }
    }
    Ok(d)
}

// =======================================================================================
// Generating the constant tables
// =======================================================================================

/// Validates an `.exig` file and emits a Rust expression of type
/// `exi::Result<GrammarSet>`. Write it to a file in a host build
/// script, then use `include!` in the target crate. The target only
/// needs the core `exi` dependency under that name, with no `exig`
/// feature.
///
/// The expression constructs the grammar once with the ordinary
/// builder. The target needs an allocator and can share the result
/// across streams.
///
/// ```
/// use exi::grammar::exig::compile_to_rust;
/// let bytes = include_bytes!(concat!(
///     env!("CARGO_MANIFEST_DIR"), "/fixtures/input/complex.exig"
/// ));
/// let source = compile_to_rust(bytes)?;
/// assert!(source.contains("::exi::grammar::GrammarSet::builder()"));
/// assert!(compile_to_rust(b"invalid").is_err());
/// # Ok::<(), exi::Error>(())
/// ```
pub fn compile_to_rust(exig: &[u8]) -> Result<String> {
    use core::fmt::Write;

    let model = Model::from_set(&GrammarSet::from_exig(exig)?)?;
    let mut out = String::from(concat!(
        "// Generated by exi from a validated .exig file.
",
        "{
",
        "    extern crate alloc;
",
        "    use alloc::vec;
",
        "    #[allow(unused_imports)]
",
        "    use ::exi::grammar::informed::{IntegerRep, Prod, Representation};
",
        "    fn n(uri: &str, local: &str) -> ::exi::OwnedName {
",
        "        ::exi::OwnedName::new(uri, local)
",
        "    }
",
        "    let mut b = ::exi::grammar::GrammarSet::builder();
",
    ));
    let simple = |i| format!("_s{i}");
    let element = |i| format!("_e{i}");
    let mut built = alloc::vec![false; model.simple.len()];
    let mut pending = Vec::new();
    // List items and enumeration bases can occur later in the EXIG
    // tables. Emit each dependency before the builder call using it.
    for start in 0..model.simple.len() {
        let mut i = start;
        while !built[i] {
            if pending.contains(&i) {
                return Err(schema_err("cyclic simple type references"));
            }
            pending.push(i);
            match &model.simple[i] {
                Representation::List(dep)
                | Representation::Enumeration { base: dep, .. } => {
                    i = dep.0 as usize;
                }
                _ => break,
            }
        }
        while let Some(i) = pending.pop() {
            let rep = render_rep(&model.simple[i], simple, "::exi");
            let id = simple(i as u32);
            let _ =
                writeln!(out, "    let {id} = b.simple_type(None, {rep});");
            built[i] = true;
        }
    }
    for (i, t) in model.types.iter().enumerate() {
        let name = t.name.as_ref().map_or_else(
            || "None".into(),
            |n| format!("Some({})", render_name(n)),
        );
        let _ = writeln!(out, "    let _t{i} = b.reserve_type({name});");
    }
    for (i, (name, ty, nillable)) in model.elements.iter().enumerate() {
        let name = render_name(name);
        let _ = writeln!(
            out,
            "    let _e{i} = b.element({name}, _t{ty}, {nillable});"
        );
    }
    for (i, t) in model.types.iter().enumerate() {
        let _ = writeln!(out, "    b.set_states(_t{i}, vec![");
        for st in &t.states {
            let prods: Vec<_> =
                st.iter().map(|p| render_prod(p, simple, element)).collect();
            let _ = writeln!(out, "        vec![{}],", prods.join(", "));
        }
        let _ = writeln!(out, "    ]);");
        if t.castable {
            let _ = writeln!(out, "    b.castable(_t{i});");
        }
        for c in &t.casts {
            let _ = writeln!(out, "    b.derives(_t{c}, _t{i});");
        }
    }
    for e in &model.globals {
        let _ = writeln!(out, "    b.global(_e{e});");
    }
    for (uri, locals) in &model.strings {
        let _ = writeln!(out, "    b.uri({uri:?});");
        for local in locals {
            let _ = writeln!(out, "    b.name(n({uri:?}, {local:?}));");
        }
    }
    out.push_str(
        "    b.build()
}
",
    );
    Ok(out)
}

fn render_name(n: &OwnedName) -> String {
    format!("n({:?}, {:?})", &*n.uri, &*n.local)
}

fn render_rep(
    r: &Representation,
    simple: impl Fn(u32) -> String,
    exi: &str,
) -> String {
    let q = |s: &str| format!("{s:?}");
    match r {
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
            format!("Representation::Binary({exi}::BinaryKind::{k:?})")
        }
        Representation::DateTime(k) => {
            format!("Representation::DateTime({exi}::DateTimeKind::{k:?})")
        }
        Representation::List(i) => {
            format!("Representation::List({})", simple(i.0))
        }
        Representation::Enumeration { values, base } => format!(
            "Representation::Enumeration {{ values: vec![{}].into(), base: {} }}",
            values
                .iter()
                .map(|v| format!("{}.into()", q(v)))
                .collect::<Vec<_>>()
                .join(", "),
            simple(base.0)
        ),
    }
}

fn render_prod(
    p: &Prod,
    simple: impl Fn(u32) -> String,
    element_id: impl Fn(u32) -> String,
) -> String {
    match p {
        Prod::At { name: x, ty, next } => format!(
            "Prod::At {{ name: {}, ty: {}, next: {next} }}",
            render_name(x),
            simple(ty.0)
        ),
        Prod::AtUri { uri, next } => format!(
            "Prod::AtUri {{ uri: {}.into(), next: {next} }}",
            format_args!("{uri:?}")
        ),
        Prod::AtAny { next } => format!("Prod::AtAny {{ next: {next} }}"),
        Prod::Se { element, next } => format!(
            "Prod::Se {{ element: {}, next: {next} }}",
            element_id(element.0)
        ),
        Prod::SeUri { uri, next } => format!(
            "Prod::SeUri {{ uri: {}.into(), next: {next} }}",
            format_args!("{uri:?}")
        ),
        Prod::SeAny { next } => format!("Prod::SeAny {{ next: {next} }}"),
        Prod::EE => "Prod::EE".into(),
        Prod::Ch { ty, next } => {
            format!("Prod::Ch {{ ty: {}, next: {next} }}", simple(ty.0))
        }
    }
}

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
    let _ = writeln!(o, "//! Generated from `schemas/exig-1.xsd` by");
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
        let s = render_rep(r, |i| format!("SimpleTypeId({i})"), "crate");
        let _ = writeln!(o, "            {s},");
    }
    let _ = writeln!(o, "        ],");
    let _ = writeln!(o, "        types: vec![");
    for t in &m.types {
        let _ = writeln!(o, "            TypeModel {{");
        let _ = writeln!(
            o,
            "                name: {},",
            t.name.as_ref().map_or("None".to_string(), |x| format!(
                "Some({})",
                render_name(x)
            ))
        );
        let _ = writeln!(o, "                castable: {},", t.castable);
        let _ = writeln!(o, "                casts: vec!{:?},", t.casts);
        let _ = writeln!(o, "                states: vec![");
        for st in &t.states {
            let ps: Vec<String> = st
                .iter()
                .map(|p| {
                    render_prod(
                        p,
                        |i| format!("SimpleTypeId({i})"),
                        |i| format!("ElementId({i})"),
                    )
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
        let _ = writeln!(o, "            ({}, {t}, {nil}),", render_name(x));
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
