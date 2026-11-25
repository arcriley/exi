// src/grammar/informed.rs
//
// Schema-informed grammars
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! A schema compiler (or a `.exig` file, later) produces these;
//! [`GrammarBuilder`] builds them in code, which is how this skeleton's
//! tests make them. The engine never derives a grammar from a schema
//! itself: it is given the derived states, with their productions in
//! event-code order.
//!
//! # Parts
//!
//! | Part | Is |
//! |---|---|
//! | Simple type | how a value is coded: a [`Representation`] |
//! | Type grammar | numbered states, each a list of productions in code order; state 0 is the start. A type's `content` index is the first state with no AT production: states before it hold the attributes |
//! | Element declaration | a name, its type, and whether it is nillable |
//! | Global elements | the declarations that may be a document's root |
//!
//! # Production order within a state
//!
//! A state's productions get codes `0, 1, 2, …` in the order listed,
//! and that order must be:
//!
//! 1. AT(name) productions, sorted by local name, then URI;
//! 2. AT(uri:\*) productions, sorted by URI;
//! 3. AT(\*), at most one;
//! 4. SE(name) productions, in schema order (the order the schema
//!    compiler gives);
//! 5. SE(uri:\*) productions, in schema order;
//! 6. SE(\*), at most one;
//! 7. EE, at most one;
//! 8. CH, at most one.
//!
//! No two productions of a state may have the same terminal: the
//! grammar would then not say which one an event is.
//! [`GrammarBuilder::build`] checks all of this.
//!
//! # The document grammar
//!
//! | State | Productions, in code order |
//! |---|---|
//! | Document | SD → DocContent |
//! | DocContent | SE(G₀) … SE(Gₙ₋₁) → DocEnd, sorted by local name then URI; then SE(\*) → DocEnd; then (second level, when kept) DT, CM, PI |
//! | DocEnd | ED; then (second level, when kept) CM, PI |
//!
//! where G₀ … Gₙ₋₁ are the global elements. An SE(\*) at the top of a
//! strict stream uses a built-in element grammar for its element (to be
//! confirmed against a fixture).
//!
//! The fragment grammar of a schema-informed stream (option `fragment`)
//! lists every declared element name, global and local, in the same
//! sorted order, then SE(\*), then ED. When one name has declarations
//! with different types or nillability, EXI uses a merged "element
//! fragment grammar"; that is `ErrorKind::Unsupported` for now.
//!
//! # What strict adds
//!
//! Under `strict`, state 0 of an element's grammar gains, as
//! second-level codes after its declared productions (first part `n`,
//! the number of declared productions in state 0):
//!
//! | Production | Code | When |
//! |---|---|---|
//! | AT(xsi:type) → state 0, then the named type's grammar | n.0 | the element's type has named sub-types or is a union (`castable`) |
//! | AT(xsi:nil) → state 0, then TypeEmpty when the value is true | n.0, or n.1 after AT(xsi:type) | the element is nillable |
//!
//! Only one of the two may appear on one element, and before any other
//! attribute. The value of `xsi:type` is a name (URI, local name); the
//! grammar then continues with that type's grammar from its state 0.
//! The value of `xsi:nil` is a Boolean.
//!
//! **TypeEmpty** of a type is its attribute states (0 to `content − 1`)
//! with only their AT productions, each followed by an EE production,
//! and a final state holding only EE. (To be confirmed against a
//! fixture before this module is marked done.)
//!
//! Without `strict`, EXI adds undeclared productions to every state;
//! that is `ErrorKind::Unsupported` in this build.
//!
//! # Values
//!
//! An AT(name) production carries the attribute's simple type; a CH
//! production the element's. The value is coded by the type's
//! [`Representation`] ([`crate::coding::datatypes`]), in
//! `Channel::Value(name)`. String values use the session's value
//! tables; no other representation does. With `Preserve.lexicalValues`
//! every value is coded as a String instead.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::grammar::{GrammarSet, Terminal};
use crate::tables::InitialEntries;
use crate::value::{BinaryKind, DateTimeKind};
use crate::{Error, ErrorKind, NameId, OwnedName, Result};

fn schema_err(detail: &'static str) -> Error {
    Error::new(ErrorKind::Schema, detail)
}

/// Index of a simple type in its grammar set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SimpleTypeId(pub(crate) u32);
/// Index of a type (and its grammar) in its grammar set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub(crate) u32);
/// Index of an element declaration in its grammar set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId(pub(crate) u32);

/// How an Integer type is coded.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IntegerRep {
    /// Sign and magnitude.
    Signed,
    /// The type's minimum is 0 or more: an Unsigned Integer.
    Unsigned,
    /// `max − min + 1 ≤ 4096`: an n-bit offset from `min`.
    Bounded { min: i64, max: i64 },
}

/// How a simple type's values are coded (see
/// [`crate::coding::datatypes`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Representation {
    /// `charset`: a restricted character set, sorted, 1 to 255 members.
    String {
        charset: Option<Box<[char]>>,
    },
    /// `pattern`: the type has a pattern facet (2-bit form).
    Boolean {
        pattern: bool,
    },
    Integer(IntegerRep),
    Decimal,
    Float,
    Binary(BinaryKind),
    DateTime(DateTimeKind),
    List(SimpleTypeId),
    /// The values in the type's order; ordinals index this list. `base`
    /// codes them when `Preserve.lexicalValues` is off and a value is
    /// not in the list (it is not encodable under strict).
    Enumeration {
        values: Box<[Box<str>]>,
        base: SimpleTypeId,
    },
}

/// A production of a schema-informed state. `next` is a state number in
/// the same type grammar.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Prod {
    At { name: OwnedName, ty: SimpleTypeId, next: u32 },
    AtUri { uri: Box<str>, next: u32 },
    AtAny { next: u32 },
    Se { element: ElementId, next: u32 },
    SeUri { uri: Box<str>, next: u32 },
    SeAny { next: u32 },
    EE,
    Ch { ty: SimpleTypeId, next: u32 },
}

/// Builds a schema-informed [`GrammarSet`] in code.
///
/// The schema below: `<reading unit="…"><temp>int</temp></reading>`,
/// target namespace `urn:example:sensor:1`, with `unit` an attribute in
/// no namespace.
///
/// ```
/// use exi::grammar::{GrammarSet, informed::{Prod, Representation, IntegerRep}};
/// use exi::OwnedName;
/// const NS: &str = "urn:example:sensor:1";
/// let mut b = GrammarSet::builder();
/// let int = b.simple_type(None, Representation::Integer(IntegerRep::Signed));
/// let string = b.simple_type(None, Representation::String { charset: None });
/// let temp_type = b.simple_content_type(None, int);
/// let temp = b.element(OwnedName::new(NS, "temp"), temp_type, false);
/// let reading_type = b.complex_type(None, vec![
///     vec![Prod::At { name: OwnedName::new("", "unit"), ty: string, next: 1 }],
///     vec![Prod::Se { element: temp, next: 2 }],
///     vec![Prod::EE],
/// ]);
/// let reading = b.element(OwnedName::new(NS, "reading"), reading_type, false);
/// b.global(reading);
/// let g = b.build()?;
/// assert!(g.is_schema_informed() && g.is_closed());
/// let e = g.initial_entries();
/// assert_eq!(e.uris()[4], NS);                          // after the four fixed URIs
/// assert_eq!(e.locals(4), ["reading", "temp"]);         // sorted
/// assert_eq!(e.locals(0), ["unit"]);
/// assert_eq!(g.name_id(NS, "temp"), Some(exi::NameId { uri: 4, local: 1 }));
/// assert_eq!(g.stats().element_declarations, 2);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// What `build` rejects, all `ErrorKind::Schema`:
///
/// ```
/// use exi::grammar::{GrammarSet, informed::{Prod, Representation}};
/// use exi::{OwnedName, ErrorKind};
/// let string = |b: &mut exi::grammar::informed::GrammarBuilder| b.simple_type(None, Representation::String { charset: None });
/// let check = |states: Vec<Vec<Prod>>| {
///     let mut b = GrammarSet::builder();
///     let s = string(&mut b);
///     let _ = s;
///     let t = b.complex_type(None, states);
///     let e = b.element(OwnedName::new("", "e"), t, false);
///     b.global(e);
///     b.build().map(|_| ()).map_err(|e| e.kind())
/// };
/// let s0 = exi::grammar::informed::SimpleTypeId::first();
/// let at = |l: &str, next| Prod::At { name: OwnedName::new("", l), ty: s0, next };
/// assert_eq!(check(vec![vec![at("b", 0), at("a", 0), Prod::EE]]), Err(ErrorKind::Schema)); // unsorted
/// assert_eq!(check(vec![vec![at("a", 0), at("a", 0), Prod::EE]]), Err(ErrorKind::Schema)); // duplicate
/// assert_eq!(check(vec![vec![Prod::EE, at("a", 0)]]), Err(ErrorKind::Schema));             // AT after EE
/// assert_eq!(check(vec![vec![at("a", 5)]]), Err(ErrorKind::Schema));                      // no state 5
/// assert_eq!(check(vec![]), Err(ErrorKind::Schema));                                       // no state 0
/// assert_eq!(check(vec![vec![]]), Err(ErrorKind::Schema));                                 // a state with nothing
/// assert_eq!(check(vec![vec![Prod::EE, Prod::EE]]), Err(ErrorKind::Schema));
/// assert_eq!(check(vec![vec![at("a", 1)], vec![Prod::EE]]), Ok(()));
/// ```
pub struct GrammarBuilder {
    simple: Vec<(Option<OwnedName>, Representation)>,
    types: Vec<(Option<OwnedName>, Vec<Vec<Prod>>)>,
    castable: Vec<TypeId>,
    derives: Vec<(TypeId, TypeId)>,
    elements: Vec<(OwnedName, TypeId, bool)>,
    globals: Vec<ElementId>,
    /// Names and URIs for the initial string table that no declaration
    /// above uses.
    extra_names: Vec<OwnedName>,
    extra_uris: Vec<Box<str>>,
}

impl SimpleTypeId {
    /// The first simple type a builder creates. For tests.
    pub const fn first() -> Self {
        SimpleTypeId(0)
    }
}

impl GrammarBuilder {
    pub(crate) fn new() -> Self {
        GrammarBuilder {
            simple: Vec::new(),
            types: Vec::new(),
            castable: Vec::new(),
            derives: Vec::new(),
            elements: Vec::new(),
            globals: Vec::new(),
            extra_names: Vec::new(),
            extra_uris: Vec::new(),
        }
    }
    /// A simple type. `name` is its name when the schema names it.
    ///
    /// Representations are checked by `build`: a restricted character
    /// set must be sorted, without duplicates, with 1 to 255 members; a
    /// bounded integer must have `min ≤ max` and at most 4096 values;
    /// an enumeration at least one value; a list's item type must
    /// not itself be a list.
    pub fn simple_type(
        &mut self,
        name: Option<OwnedName>,
        rep: Representation,
    ) -> SimpleTypeId {
        self.simple.push((name, rep));
        SimpleTypeId(self.simple.len() as u32 - 1)
    }
    /// A type with simple content of type `ty`: states `[CH(ty) → 1]`,
    /// `[EE]`; `content` 0.
    pub fn simple_content_type(
        &mut self,
        name: Option<OwnedName>,
        ty: SimpleTypeId,
    ) -> TypeId {
        self.complex_type(
            name,
            alloc::vec![
                alloc::vec![Prod::Ch { ty, next: 1 }],
                alloc::vec![Prod::EE]
            ],
        )
    }
    /// A type with no content and no attributes: `[EE]`.
    pub fn empty_type(&mut self, name: Option<OwnedName>) -> TypeId {
        self.complex_type(name, alloc::vec![alloc::vec![Prod::EE]])
    }
    /// A type from its states, state 0 first, each in code order
    /// (module documentation). `content` is computed: the first state
    /// with no AT production.
    pub fn complex_type(
        &mut self,
        name: Option<OwnedName>,
        states: Vec<Vec<Prod>>,
    ) -> TypeId {
        self.types.push((name, states));
        TypeId(self.types.len() as u32 - 1)
    }
    /// Marks a type as having named sub-types or being a union: strict
    /// grammars then allow `xsi:type` on elements of this type.
    pub fn castable(&mut self, t: TypeId) {
        self.castable.push(t);
    }
    /// Declares that named type `sub` derives from `t`, so `xsi:type`
    /// may name `sub` where `t` is expected. Implies `castable(t)`.
    pub fn derives(&mut self, sub: TypeId, t: TypeId) {
        self.castable.push(t);
        self.derives.push((sub, t));
    }
    /// An element declaration.
    pub fn element(
        &mut self,
        name: OwnedName,
        ty: TypeId,
        nillable: bool,
    ) -> ElementId {
        self.elements.push((name, ty, nillable));
        ElementId(self.elements.len() as u32 - 1)
    }
    /// Makes a declaration global: a possible document root.
    /// A type whose states come later, from [`set_states`](Self::set_states): for types
    /// that contain themselves, and for compilers that number types
    /// before building them. `build` fails with `ErrorKind::Schema` if
    /// a reserved type never gets states.
    ///
    /// ```
    /// use exi::grammar::{GrammarSet, informed::Prod};
    /// use exi::{ErrorKind, OwnedName};
    /// let mut b = GrammarSet::builder();
    /// let t = b.reserve_type(None);
    /// let e = b.element(OwnedName::new("", "node"), t, false);
    /// b.set_states(t, vec![vec![Prod::Se { element: e, next: 0 }, Prod::EE]]);   // node*
    /// b.global(e);
    /// assert!(b.build().is_ok());
    ///
    /// let mut b = GrammarSet::builder();
    /// b.reserve_type(None);
    /// assert_eq!(b.build().unwrap_err().kind(), ErrorKind::Schema);
    /// ```
    pub fn reserve_type(&mut self, name: Option<OwnedName>) -> TypeId {
        self.types.push((name, Vec::new()));
        TypeId(self.types.len() as u32 - 1)
    }

    /// The states of a type from [`reserve_type`](Self::reserve_type). Replaces any given
    /// before.
    pub fn set_states(&mut self, t: TypeId, states: Vec<Vec<Prod>>) {
        if let Some(slot) = self.types.get_mut(t.0 as usize) {
            slot.1 = states;
        }
    }

    /// A name for the initial string table that no declaration uses,
    /// such as a global attribute no type refers to. EXI pre-loads the
    /// local names of every declaration and named type in a schema.
    pub fn name(&mut self, name: OwnedName) {
        self.extra_names.push(name);
    }

    /// A URI for the initial string table, such as a target namespace
    /// with no names.
    pub fn uri(&mut self, uri: &str) {
        self.extra_uris.push(uri.into());
    }

    pub fn global(&mut self, e: ElementId) {
        self.globals.push(e);
    }
    /// Checks everything (module documentation and the methods above)
    /// and builds the grammar set, with its initial string-table
    /// entries computed from every name used (element, attribute and
    /// type names, by URI, sorted).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Schema` for anything the checks reject;
    /// `ErrorKind::Limit` when a count exceeds `u32::MAX − 1`.
    pub fn build(self) -> Result<GrammarSet> {
        let b = self;
        let count_ok = |n: usize| n < u32::MAX as usize - 1;
        if !count_ok(b.simple.len())
            || !count_ok(b.types.len())
            || !count_ok(b.elements.len())
        {
            return Err(Error::new(
                ErrorKind::Limit,
                "more than u32::MAX - 1 definitions",
            ));
        }
        for (_, rep) in &b.simple {
            check_rep(rep, &b.simple)?;
        }
        for (_, states) in &b.types {
            if states.is_empty() {
                return Err(schema_err("a type grammar with no state 0"));
            }
            for st in states {
                check_state(st, states.len(), &b)?;
            }
        }
        let type_ok = |t: &TypeId| (t.0 as usize) < b.types.len();
        if !b.elements.iter().all(|(_, t, _)| type_ok(t))
            || !b.castable.iter().all(type_ok)
            || !b.derives.iter().all(|(s, t)| type_ok(s) && type_ok(t))
            || !b.globals.iter().all(|e| (e.0 as usize) < b.elements.len())
        {
            return Err(schema_err("unknown type or element declaration"));
        }

        // Initial entries: every name used.
        let mut ib = InitialEntries::schema();
        for (n, _, _) in &b.elements {
            ib = ib.local(&n.uri, &n.local);
        }
        for n in b
            .simple
            .iter()
            .filter_map(|(n, _)| n.as_ref())
            .chain(b.types.iter().filter_map(|(n, _)| n.as_ref()))
        {
            ib = ib.local(&n.uri, &n.local);
        }
        for n in &b.extra_names {
            ib = ib.local(&n.uri, &n.local);
        }
        for u in &b.extra_uris {
            ib = ib.uri(u);
        }
        for (_, states) in &b.types {
            for p in states.iter().flatten() {
                match p {
                    Prod::At { name, .. } => {
                        ib = ib.local(&name.uri, &name.local)
                    }
                    Prod::AtUri { uri, .. } | Prod::SeUri { uri, .. } => {
                        ib = ib.uri(uri)
                    }
                    _ => {}
                }
            }
        }
        let entries = ib.build()?;
        let uris = entries.uris();
        let uri_no = |u: &str| {
            uris.iter()
                .position(|x| *x == u)
                .expect("every URI is an initial entry") as u32
        };
        let id_of = |n: &OwnedName| {
            let uri = uri_no(&n.uri);
            let local = entries
                .locals(uri)
                .iter()
                .position(|l| *l == &*n.local)
                .expect("every name is an initial entry")
                as u32;
            NameId { uri, local }
        };

        let elements: Vec<ElemDef> = b
            .elements
            .iter()
            .map(|(n, ty, nil)| ElemDef {
                name: n.clone(),
                id: id_of(n),
                ty: *ty,
                nillable: *nil,
            })
            .collect();
        let mut closed = true;
        let mut types = Vec::with_capacity(b.types.len());
        let minimal: Vec<Vec<Vec<Prod>>> =
            b.types.iter().map(|(_, st)| minimize(st)).collect();
        for (i, ((name, _), states)) in
            b.types.iter().zip(&minimal).enumerate()
        {
            let mut out = Vec::with_capacity(states.len());
            for st in states {
                let prods: Vec<IProd> = st
                    .iter()
                    .map(|p| {
                        let (term, ty, elem) = match p {
                            Prod::At { name, ty, .. } => {
                                (Terminal::AT(id_of(name)), Some(*ty), None)
                            }
                            Prod::AtUri { uri, .. } => {
                                (Terminal::AtUri(uri_no(uri)), None, None)
                            }
                            Prod::AtAny { .. } => {
                                (Terminal::AtAny, None, None)
                            }
                            Prod::Se { element, .. } => (
                                Terminal::SE(elements[element.0 as usize].id),
                                None,
                                Some(*element),
                            ),
                            Prod::SeUri { uri, .. } => {
                                (Terminal::SeUri(uri_no(uri)), None, None)
                            }
                            Prod::SeAny { .. } => {
                                (Terminal::SeAny, None, None)
                            }
                            Prod::EE => (Terminal::EE, None, None),
                            Prod::Ch { ty, .. } => {
                                (Terminal::CH, Some(*ty), None)
                            }
                        };
                        if matches!(
                            term,
                            Terminal::AtUri(_)
                                | Terminal::AtAny
                                | Terminal::SeUri(_)
                                | Terminal::SeAny
                        ) {
                            closed = false;
                        }
                        IProd { term, next: next_of(p).unwrap_or(0), ty, elem }
                    })
                    .collect();
                out.push(prods);
            }
            let content = out
                .iter()
                .position(|st| {
                    !st.iter().any(|p| {
                        matches!(
                            p.term,
                            Terminal::AT(_)
                                | Terminal::AtUri(_)
                                | Terminal::AtAny
                        )
                    })
                })
                .unwrap_or(out.len()) as u32;
            let t = TypeId(i as u32);
            // Named types reachable through `derives`, starting from t.
            let mut casts = Vec::new();
            let mut todo = alloc::vec![t];
            while let Some(x) = todo.pop() {
                if casts.contains(&x) {
                    continue;
                }
                casts.push(x);
                todo.extend(
                    b.derives
                        .iter()
                        .filter(|(_, base)| *base == x)
                        .map(|(sub, _)| *sub),
                );
            }
            casts.retain(|c| b.types[c.0 as usize].0.is_some());
            types.push(TypeDef {
                name: name.clone(),
                states: out,
                content,
                castable: b.castable.contains(&t),
                casts,
            });
        }

        let key = |e: &ElementId| {
            let n = &elements[e.0 as usize].name;
            (n.local.clone(), n.uri.clone())
        };
        let mut globals = b.globals.clone();
        globals.sort_by_key(key);
        globals.dedup_by(|a, c| key(a) == key(c));
        let mut all: Vec<ElementId> =
            (0..elements.len() as u32).map(ElementId).collect();
        all.sort_by_key(key);
        let mut fragment = Some(Vec::new());
        for e in all {
            let f = fragment.as_mut().expect("set above");
            match f.last() {
                Some(prev) if key(prev) == key(&e) => {
                    let (p, q) =
                        (&elements[prev.0 as usize], &elements[e.0 as usize]);
                    if (p.ty, p.nillable) != (q.ty, q.nillable) {
                        fragment = None;
                        break;
                    }
                }
                _ => f.push(e),
            }
        }
        let simple = b.simple.into_iter().map(|(_, r)| r).collect();
        let schema =
            Schema { simple, types, elements, globals, fragment, closed };
        Ok(GrammarSet { entries, schema: Some(schema) })
    }
}

// ---------------------------------------------------------------------------------------
// The built form, used by readers and writers.
// ---------------------------------------------------------------------------------------

/// A production with its terminal resolved to fixed name numbers.
#[derive(Clone, Debug)]
pub(crate) struct IProd {
    pub term: Terminal,
    pub next: u32,
    /// The value's type: AT(name) and CH.
    pub ty: Option<SimpleTypeId>,
    /// The element: SE(name).
    pub elem: Option<ElementId>,
}

#[derive(Clone, Debug)]
pub(crate) struct TypeDef {
    pub name: Option<OwnedName>,
    pub states: Vec<Vec<IProd>>,
    pub content: u32,
    pub castable: bool,
    /// Named types `xsi:type` may name where this type is expected
    /// (itself included).
    pub casts: Vec<TypeId>,
}

#[derive(Clone, Debug)]
pub(crate) struct ElemDef {
    pub name: OwnedName,
    pub id: NameId,
    pub ty: TypeId,
    pub nillable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Schema {
    pub simple: Vec<Representation>,
    pub types: Vec<TypeDef>,
    pub elements: Vec<ElemDef>,
    /// Global declarations, sorted by local name then URI.
    pub globals: Vec<ElementId>,
    /// Every declared name for the fragment grammar, sorted; `None`
    /// when one name has declarations that differ (element fragment
    /// grammars, not supported yet).
    pub fragment: Option<Vec<ElementId>>,
    pub closed: bool,
}

impl Schema {
    pub fn rep(&self, t: SimpleTypeId) -> &Representation {
        &self.simple[t.0 as usize]
    }
    pub fn ty(&self, t: TypeId) -> &TypeDef {
        &self.types[t.0 as usize]
    }
    pub fn elem(&self, e: ElementId) -> &ElemDef {
        &self.elements[e.0 as usize]
    }
    /// A global declaration with this name.
    pub fn global_named(&self, id: NameId) -> Option<ElementId> {
        self.globals.iter().copied().find(|e| self.elem(*e).id == id)
    }
    /// A named type, by its name.
    pub fn type_named(&self, uri: &str, local: &str) -> Option<TypeId> {
        self.types
            .iter()
            .position(|t| {
                t.name.as_ref().is_some_and(|n| n.as_name().is(uri, local))
            })
            .map(|i| TypeId(i as u32))
    }
}

/// The type grammar with equivalent states merged: states with the same
/// productions leading to equivalent states are one state (Moore's
/// partition refinement). State 0 stays state 0, and the others keep
/// the order of their first member. No event code changes: equivalent
/// states have the same productions in the same order.
///
/// So a grammar has one form however it was built (A2): the XSD
/// compiler gives a repeated choice of N elements N states with the
/// same productions, and they become one.
///
/// ```
/// use exi::grammar::{GrammarSet, informed::{Prod, Representation}};
/// use exi::OwnedName;
/// let mut b = GrammarSet::builder();
/// let s = b.simple_type(None, Representation::String { charset: None });
/// let leaf = b.simple_content_type(None, s);
/// let x = b.element(OwnedName::new("", "x"), leaf, false);
/// let y = b.element(OwnedName::new("", "y"), leaf, false);
/// // (x | y)*: after x and after y the same productions, as a compiler may build it.
/// let any = |n| vec![Prod::Se { element: x, next: 1 }, Prod::Se { element: y, next: 2 }, Prod::EE];
/// let t = b.complex_type(None, vec![any(0), any(1), any(2)]);
/// let e = b.element(OwnedName::new("", "list"), t, false);
/// b.global(e);
/// assert_eq!(b.build()?.stats().states, 2 + 1);  // the simple content type's two states, and one
/// # Ok::<(), exi::Error>(())
/// ```
fn minimize(states: &[Vec<Prod>]) -> Vec<Vec<Prod>> {
    use hashbrown::HashMap;
    let n = states.len();
    let shape = |p: &Prod| -> Prod {
        let mut q = p.clone();
        match &mut q {
            Prod::At { next, .. }
            | Prod::AtUri { next, .. }
            | Prod::AtAny { next }
            | Prod::Se { next, .. }
            | Prod::SeUri { next, .. }
            | Prod::SeAny { next }
            | Prod::Ch { next, .. } => *next = 0,
            Prod::EE => {}
        }
        q
    };
    let mut block: Vec<u32> = alloc::vec![0; n];
    let mut count = 1;
    loop {
        let mut ids: HashMap<
            Vec<(Prod, u32)>,
            u32,
            rustc_hash::FxBuildHasher,
        > = HashMap::with_hasher(rustc_hash::FxBuildHasher);
        let mut next_block = alloc::vec![0u32; n];
        for (i, st) in states.iter().enumerate() {
            let sig: Vec<(Prod, u32)> = st
                .iter()
                .map(|p| {
                    (
                        shape(p),
                        next_of(p).map_or(u32::MAX, |x| block[x as usize]),
                    )
                })
                .collect();
            let k = ids.len() as u32;
            next_block[i] = *ids.entry(sig).or_insert(k);
        }
        let new_count = ids.len();
        block = next_block;
        if new_count == count {
            break;
        }
        count = new_count;
    }
    if count == n {
        return states.to_vec();
    }
    // Blocks numbered in the order of their first state; state 0's
    // block is 0.
    let mut order: Vec<u32> = alloc::vec![u32::MAX; count];
    let mut k = 0;
    for &b in &block {
        if order[b as usize] == u32::MAX {
            order[b as usize] = k;
            k += 1;
        }
    }
    let mut out: Vec<Option<Vec<Prod>>> = alloc::vec![None; count];
    for (i, st) in states.iter().enumerate() {
        let b = order[block[i] as usize] as usize;
        if out[b].is_none() {
            out[b] = Some(
                st.iter()
                    .map(|p| {
                        let mut q = p.clone();
                        match &mut q {
                            Prod::At { next, .. }
                            | Prod::AtUri { next, .. }
                            | Prod::AtAny { next }
                            | Prod::Se { next, .. }
                            | Prod::SeUri { next, .. }
                            | Prod::SeAny { next }
                            | Prod::Ch { next, .. } => {
                                *next = order[block[*next as usize] as usize]
                            }
                            Prod::EE => {}
                        }
                        q
                    })
                    .collect(),
            );
        }
    }
    out.into_iter().map(|s| s.expect("every block has a state")).collect()
}

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

fn next_of(p: &Prod) -> Option<u32> {
    match p {
        Prod::At { next, .. }
        | Prod::AtUri { next, .. }
        | Prod::AtAny { next } => Some(*next),
        Prod::Se { next, .. }
        | Prod::SeUri { next, .. }
        | Prod::SeAny { next }
        | Prod::Ch { next, .. } => Some(*next),
        Prod::EE => None,
    }
}

fn check_rep(
    rep: &Representation,
    all: &[(Option<OwnedName>, Representation)],
) -> Result<()> {
    let valid = |t: &SimpleTypeId| (t.0 as usize) < all.len();
    match rep {
        Representation::String { charset: Some(cs) }
            if cs.is_empty()
                || cs.len() > 255
                || !cs.windows(2).all(|w| w[0] < w[1]) =>
        {
            Err(schema_err(
                "restricted character set: 1 to 255 characters, sorted, no duplicates",
            ))
        }
        Representation::Integer(IntegerRep::Bounded { min, max })
            if min > max || (*max as i128 - *min as i128) >= 4096 =>
        {
            Err(schema_err(
                "bounded integer: min ≤ max and at most 4096 values",
            ))
        }
        Representation::List(item)
            if !valid(item)
                || matches!(
                    all[item.0 as usize].1,
                    Representation::List(_)
                ) =>
        {
            Err(schema_err("list item type: a simple type that is not a list"))
        }
        Representation::Enumeration { values, base }
            if values.is_empty() || !valid(base) =>
        {
            Err(schema_err(
                "enumeration: at least one value and a valid base type",
            ))
        }
        _ => Ok(()),
    }
}

fn check_state(
    state: &[Prod],
    n_states: usize,
    b: &GrammarBuilder,
) -> Result<()> {
    if state.is_empty() {
        return Err(schema_err("a state with no productions"));
    }
    for (i, p) in state.iter().enumerate() {
        if next_of(p).is_some_and(|n| n as usize >= n_states) {
            return Err(schema_err(
                "a production names a state that does not exist",
            ));
        }
        match p {
            Prod::At { ty, .. } | Prod::Ch { ty, .. }
                if ty.0 as usize >= b.simple.len() =>
            {
                return Err(schema_err("unknown simple type"));
            }
            Prod::Se { element, .. }
                if element.0 as usize >= b.elements.len() =>
            {
                return Err(schema_err("unknown element declaration"));
            }
            _ => {}
        }
        if i == 0 {
            continue;
        }
        let q = &state[i - 1];
        let (rq, rp) = (rank(q), rank(p));
        let ordered = match (q, p) {
            _ if rq < rp => true,
            _ if rq > rp => false,
            (Prod::At { name: a, .. }, Prod::At { name: b, .. }) => {
                (&*a.local, &*a.uri) < (&*b.local, &*b.uri)
            }
            (Prod::AtUri { uri: a, .. }, Prod::AtUri { uri: b, .. }) => a < b,
            (Prod::Se { .. }, Prod::Se { .. }) => true, /* schema order; duplicates below */
            (Prod::SeUri { .. }, Prod::SeUri { .. }) => true,
            _ => false, // a second AT(*), SE(*), EE or CH
        };
        if !ordered {
            return Err(schema_err(
                "productions out of order, or a terminal given twice",
            ));
        }
    }
    for (i, p) in state.iter().enumerate() {
        for q in &state[..i] {
            let dup = match (q, p) {
                (Prod::Se { element: a, .. }, Prod::Se { element: c, .. }) => {
                    b.elements[a.0 as usize].0 == b.elements[c.0 as usize].0
                }
                (Prod::SeUri { uri: a, .. }, Prod::SeUri { uri: c, .. }) => {
                    a == c
                }
                _ => false,
            };
            if dup {
                return Err(schema_err("a terminal given twice"));
            }
        }
    }
    Ok(())
}
