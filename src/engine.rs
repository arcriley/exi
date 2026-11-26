// src/engine.rs
//
// The grammar state machine shared by the EXI reader and writer.
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
// The Apache license grants a patent license from every contributor
// so that changes are not submitted that violate a contributor's own
// patents in order to extort users for a separate licensing fee.
//
//!
//! # Shape
//!
//! [`State`] holds everything a stream's grammars track: the top-level
//! state, the stack of open elements, and the built-in element grammars
//! learned so far, and the learning budget. The session string tables
//! are kept beside it, not in it, so a reader can decode an
//! event against a counting view of the tables (see [`Probe`]) while it
//! reads the state.
//!
//! An event is handled in three steps, the same for both sides:
//!
//! 1. [`State::table`] gives the current state's code table. The writer
//!    finds the production for its event in it; the reader reads a code
//!    and looks the production up.
//! 2. The event's content is coded: names, then the value.
//! 3. One `on_*` method moves the state: learning, the next state,
//!    pushing or popping an element.
//!
//! Step 3 never fails for a reason the input or the caller can fix,
//! except the depth and learning limits, which it checks before
//! changing anything.

use alloc::vec::Vec;

use hashbrown::HashMap;
use rustc_hash::FxBuildHasher;

use crate::grammar::builtin::{
    CodeTable, ElementGrammar, Keep, Learn, TopLevel,
};
use crate::grammar::informed::{
    ElementId, IProd, Schema, SimpleTypeId, TypeId,
};
use crate::grammar::{EventCode, Grammars, Terminal};
use crate::options::{Coding, SchemaId};
use crate::tables::ValueRules;
use crate::{Error, ErrorKind, Limits, NameId, Options, Preserve, Result};

mod wire;

#[cfg(feature = "compression")]
pub(crate) use wire::decode_value;

pub(crate) use wire::{
    DVal, Decoded, Prepared, Probe, decode, emit_value, lexical_string,
    owned_name, prepare, value_str,
};

pub(crate) const XSI_NIL: NameId = NameId { uri: 2, local: 0 };
pub(crate) const XSI_TYPE: NameId = NameId { uri: 2, local: 1 };

type Map<K, V> = HashMap<K, V, FxBuildHasher>;

/// A code table from full codes, keeping the productions the options
/// keep.
fn pruned(full: &[(Terminal, EventCode)], keep: Keep) -> CodeTable {
    CodeTable::pruned(full, crate::grammar::builtin::keeps(keep))
}

pub(crate) fn err(kind: ErrorKind, detail: &'static str) -> Error {
    Error::new(kind, detail)
}

fn internal(detail: &'static str) -> Error {
    err(ErrorKind::Internal, detail)
}

/// Checks that this build can code a stream with `opts` and `g` (reader
/// and writer tables).
pub(crate) fn check_options(opts: &Options, g: &Grammars<'_>) -> Result<()> {
    match opts.coding() {
        Coding::ByteAligned => {}
        Coding::BitPacked if cfg!(feature = "bit-packed") => {}
        Coding::BitPacked => {
            return Err(err(
                ErrorKind::Unsupported,
                "bit-packed bodies need the feature bit-packed",
            ));
        }
        Coding::PreCompression | Coding::Compression
            if cfg!(feature = "compression") => {}
        Coding::PreCompression | Coding::Compression => {
            return Err(err(
                ErrorKind::Unsupported,
                "pre-compression and compression need the feature compression",
            ));
        }
    }
    let informed = g.set().schema.as_ref();
    match (opts.schema_id(), informed.is_some()) {
        (SchemaId::Nil, true) => {
            return Err(err(
                ErrorKind::Schema,
                "schemaId nil with schema-informed grammars",
            ));
        }
        (SchemaId::Id(_), false) => {
            return Err(err(
                ErrorKind::Schema,
                "schemaId names a schema but the grammars are schema-less",
            ));
        }
        (SchemaId::BuiltinTypes, _) => {
            return Err(err(
                ErrorKind::Unsupported,
                "schemaId \"\" (built-in types only)",
            ));
        }
        _ => {}
    }
    if let Some(s) = informed {
        if !opts.strict() && opts.self_contained() {
            return Err(err(
                ErrorKind::Unsupported,
                "self-contained elements with schema-informed grammars",
            ));
        }
        if opts.fragment() && s.fragment.is_none() {
            return Err(err(
                ErrorKind::Unsupported,
                "element fragment grammars",
            ));
        }
    }
    Ok(())
}

/// The value rules of a stream's options.
pub(crate) fn value_rules(o: &Options) -> ValueRules {
    ValueRules::unbounded()
        .capacity(o.value_partition_capacity())
        .max_length(o.value_max_length())
}

/// Where an element is in its grammar.
#[derive(Copy, Clone, Debug)]
pub(crate) enum FrameKind {
    /// `content` false: StartTagContent; true: ElementContent.
    Builtin { content: bool },
    /// State `state` of type `ty`. `tag`: in the start tag, which ends
    /// at the first event that is not an attribute. `fresh`: in the
    /// first start tag (state 0, entered by SE or xsi:type and left by
    /// no production but its own attribute loops), where xsi:type
    /// and xsi:nil are allowed. `empty`: xsi:nil was true, the grammar
    /// is TypeEmpty.
    ///
    /// The start tag matters because EXI gives start-tag states more
    /// added productions than content states when strict is off (EXI
    /// 8.5.4.4.1): the last attribute state and the first content
    /// state have the same productions but are two states.
    Informed {
        ty: TypeId,
        state: u32,
        tag: bool,
        fresh: bool,
        empty: bool,
        nillable: bool,
    },
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct Frame {
    pub name: NameId,
    pub prefix: Option<u32>,
    pub kind: FrameKind,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Top {
    /// DocContent or FragmentContent.
    Content,
    /// DocEnd.
    End,
    /// After ED.
    Done,
}

/// How a value is coded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// A string with the value tables.
    Str,
    /// A name: `xsi:type` in built-in grammars.
    QName,
    /// `xsi:nil` under strict: a Boolean.
    Nil,
    /// `xsi:type` under strict: a type's name.
    TypeName,
    Typed(SimpleTypeId),
    /// A typed value under `Preserve.lexicalValues`: a string, with the
    /// restricted character set of the type's representation (EXI Table
    /// 7-2).
    Lexical(SimpleTypeId),
    /// `xsi:nil` under strict and `Preserve.lexicalValues`: a string
    /// with the Boolean character set, which still decides the grammar.
    NilText,
}

/// The event codes of the current state. Ordinary strict states use
/// production indexes directly as their one-part codes.
pub(crate) enum Table<'a> {
    Flat(&'a [IProd]),
    Borrowed(&'a CodeTable),
    Ref(core::cell::Ref<'a, CodeTable>),
}

impl Table<'_> {
    fn explicit(&self) -> Option<&CodeTable> {
        match self {
            Table::Flat(_) => None,
            Table::Borrowed(t) => Some(t),
            Table::Ref(t) => Some(t),
        }
    }

    pub fn code(&self, term: Terminal) -> Option<EventCode> {
        match self {
            Table::Flat(prods) => prods
                .iter()
                .position(|p| p.term == term)
                .map(|i| EventCode::new(&[i as u32])),
            Table::Borrowed(t) => t.code(term),
            Table::Ref(t) => t.code(term),
        }
    }

    pub fn code_with_len(&self, term: Terminal, len: u8) -> Option<EventCode> {
        match self.explicit() {
            Some(t) => t
                .entries()
                .iter()
                .find(|(t, c)| *t == term && c.len() == len)
                .map(|(_, c)| *c),
            None if len == 1 => self.code(term),
            None => None,
        }
    }

    pub fn count(&self, prefix: &[u32]) -> u32 {
        match self {
            Table::Flat(prods) if prefix.is_empty() => prods.len() as u32,
            Table::Flat(_) => 0,
            Table::Borrowed(t) => t.count(prefix),
            Table::Ref(t) => t.count(prefix),
        }
    }

    fn lookup(&self, parts: &[u32]) -> Option<Terminal> {
        match self {
            Table::Flat(prods) => match parts {
                &[i] => prods.get(i as usize).map(|p| p.term),
                _ => None,
            },
            Table::Borrowed(t) => t.lookup(parts),
            Table::Ref(t) => t.lookup(parts),
        }
    }
}

/// Content event codes, their corresponding element declarations,
/// and the document-end event codes.
type InformedTop = (CodeTable, Vec<Option<ElementId>>, CodeTable);

/// Immutable document or fragment tables shared within a grammar
/// attachment. A built-in fragment becomes private when it learns.
pub(crate) struct TopTables {
    builtin: TopLevel,
    informed: Option<InformedTop>,
}

pub(crate) struct State<'g> {
    pub schema: Option<&'g Schema>,
    #[cfg(feature = "std")]
    grammars: &'g Grammars<'g>,
    /// Per-stream cache of built code tables; populated from `grammars`
    /// on first miss under `std`, so hot-path lookups avoid the mutex
    /// and `Arc` refcount churn.
    tables: core::cell::RefCell<crate::grammar::TableCache>,
    pub opts: Options,
    pub keep: Keep,
    pub limits: Limits,
    pub budget: u64,
    #[cfg(feature = "std")]
    top_tables: alloc::sync::Arc<TopTables>,
    #[cfg(not(feature = "std"))]
    informed_top: Option<InformedTop>,
    /// Built-in tables owned by this stream. With std, these are needed
    /// only inside SC or after a fragment learns.
    builtin_top: Option<TopLevel>,
    pub top: Top,
    elems: Map<NameId, ElementGrammar>,
    pub stack: Vec<Frame>,
    /// The grammar state outside each open self-contained element,
    /// innermost last.
    saved: Vec<Saved>,
}

/// What a self-contained element moves aside: everything learned, and
/// the open elements.
struct Saved {
    builtin_top: Option<TopLevel>,
    top: Top,
    elems: Map<NameId, ElementGrammar>,
    stack: Vec<Frame>,
}

impl TopTables {
    pub(crate) fn new(
        schema: Option<&Schema>,
        opts: &Options,
        keep: Keep,
    ) -> Self {
        let builtin_top = if opts.fragment() {
            TopLevel::fragment(keep)
        } else {
            TopLevel::document(keep)
        };
        let informed_top = schema.map(|s| {
            let list = if opts.fragment() {
                s.fragment.as_deref().unwrap_or_default()
            } else {
                &s.globals
            };
            let mut terms: Vec<Terminal> =
                list.iter().map(|e| Terminal::SE(s.elem(*e).id)).collect();
            let mut elems: Vec<Option<ElementId>> =
                list.iter().copied().map(Some).collect();
            terms.push(Terminal::SeAny);
            elems.push(None);
            if opts.fragment() {
                terms.push(Terminal::ED);
                elems.push(None);
            }
            if opts.strict() {
                return (
                    CodeTable::flat(terms),
                    elems,
                    CodeTable::flat([Terminal::ED]),
                );
            }
            // Not strict: DT, CM and PI after the first level, as in
            // the built-in document and fragment grammars, pruned by
            // the options.
            let n = terms.len() as u32;
            let mut full: Vec<(Terminal, EventCode)> = terms
                .into_iter()
                .enumerate()
                .map(|(i, t)| (t, EventCode::new(&[i as u32])))
                .collect();
            let mut end: Vec<(Terminal, EventCode)> =
                alloc::vec![(Terminal::ED, EventCode::new(&[0]))];
            if opts.fragment() {
                full.push((Terminal::CM, EventCode::new(&[n, 0])));
                full.push((Terminal::PI, EventCode::new(&[n, 1])));
            } else {
                full.push((Terminal::DT, EventCode::new(&[n, 0])));
                full.push((Terminal::CM, EventCode::new(&[n, 1, 0])));
                full.push((Terminal::PI, EventCode::new(&[n, 1, 1])));
                end.push((Terminal::CM, EventCode::new(&[1, 0])));
                end.push((Terminal::PI, EventCode::new(&[1, 1])));
            }
            (pruned(&full, keep), elems, pruned(&end, keep))
        });
        Self { builtin: builtin_top, informed: informed_top }
    }
}

impl<'g> State<'g> {
    pub fn new(
        opts: Options,
        g: &'g Grammars<'g>,
        limits: Limits,
    ) -> State<'g> {
        let keep = Keep::from_options(&opts);
        let schema = g.set().schema.as_ref();
        #[cfg(feature = "std")]
        let top_tables = g.top_tables(&opts, keep);
        #[cfg(not(feature = "std"))]
        let top_tables = TopTables::new(schema, &opts, keep);
        State {
            schema,
            #[cfg(feature = "std")]
            grammars: g,
            tables: Default::default(),
            keep,
            budget: limits.get_max_learned_productions(),
            limits,
            #[cfg(feature = "std")]
            top_tables,
            #[cfg(not(feature = "std"))]
            informed_top: top_tables.informed,
            #[cfg(feature = "std")]
            builtin_top: None,
            #[cfg(not(feature = "std"))]
            builtin_top: Some(top_tables.builtin),
            top: Top::Content,
            elems: Map::with_hasher(FxBuildHasher),
            stack: Vec::new(),
            saved: Vec::new(),
            opts,
        }
    }

    /// Open elements, counting those outside self-contained elements
    /// once.
    pub fn depth(&self) -> u32 {
        (self.saved.iter().map(|s| s.stack.len()).sum::<usize>()
            + self.stack.len()
            - self.saved.len()) as u32
    }

    /// Inside a self-contained element.
    pub fn in_sc(&self) -> bool {
        !self.saved.is_empty()
    }

    /// The top level uses the fragment grammar: option `fragment`, or
    /// inside a self-contained element.
    fn fragment(&self) -> bool {
        self.opts.fragment() || self.in_sc()
    }

    /// After SC: moves the learned grammars and open elements aside and
    /// starts again from the fragment grammar (built-in grammars
    /// module, "Self-contained elements").
    pub fn begin_sc(&mut self) {
        let fresh_top = Some(TopLevel::fragment(self.keep));
        self.saved.push(Saved {
            builtin_top: core::mem::replace(&mut self.builtin_top, fresh_top),
            top: core::mem::replace(&mut self.top, Top::Content),
            elems: core::mem::replace(
                &mut self.elems,
                Map::with_hasher(FxBuildHasher),
            ),
            stack: core::mem::take(&mut self.stack),
        });
    }

    /// After the self-contained element's ED: puts the outer state back
    /// and ends the element there as if it were an ordinary child.
    pub fn end_sc(&mut self) -> Result<()> {
        let s = self
            .saved
            .pop()
            .ok_or_else(|| internal("no self-contained element is open"))?;
        self.builtin_top = s.builtin_top;
        self.top = s.top;
        self.elems = s.elems;
        self.stack = s.stack;
        self.stack.pop().ok_or_else(|| {
            internal("a self-contained element without its frame")
        })?;
        Ok(())
    }

    pub fn lexical(&self) -> bool {
        self.opts.preserve().contains(Preserve::LEXICAL_VALUES)
    }

    pub fn prefixes(&self) -> bool {
        self.opts.preserve().contains(Preserve::PREFIXES)
    }

    fn schema(&self) -> Result<&'g Schema> {
        self.schema
            .ok_or_else(|| internal("schema-informed frame without a schema"))
    }

    fn informed_top(&self) -> Option<&InformedTop> {
        #[cfg(feature = "std")]
        let tables = &self.top_tables.informed;
        #[cfg(not(feature = "std"))]
        let tables = &self.informed_top;
        tables.as_ref()
    }

    fn builtin_top(&self) -> &TopLevel {
        #[cfg(feature = "std")]
        return self.builtin_top.as_ref().unwrap_or(&self.top_tables.builtin);
        #[cfg(not(feature = "std"))]
        self.builtin_top.as_ref().expect("owned tables")
    }

    /// The code table of the current state.
    pub fn table(&self) -> Result<Table<'_>> {
        let Some(f) = self.stack.last() else {
            let informed_top =
                if self.in_sc() { None } else { self.informed_top() };
            return Ok(match (self.top, informed_top) {
                (Top::Done, _) => Table::Flat(&[]),
                (Top::Content, Some((t, _, _))) => Table::Borrowed(t),
                (Top::End, Some((_, _, end))) => Table::Borrowed(end),
                (Top::Content, None) => {
                    Table::Borrowed(self.builtin_top().content())
                }
                (Top::End, None) => Table::Borrowed(self.builtin_top().end()),
            });
        };
        match f.kind {
            FrameKind::Builtin { content } => {
                let g = self.elems.get(&f.name).ok_or_else(|| {
                    internal("open element without a grammar")
                })?;
                Ok(Table::Borrowed(if content {
                    g.content()
                } else {
                    g.start_tag()
                }))
            }
            FrameKind::Informed { ty, state, tag, fresh, empty, nillable } => {
                let td = self.schema()?.ty(ty);
                let key = crate::grammar::TableKey {
                    ty: ty.0,
                    state,
                    flags: (tag as u16)
                        | (fresh as u16) << 1
                        | (empty as u16) << 2
                        | (nillable as u16) << 3
                        | self.keep_bits() << 4,
                };
                if !self.opts.strict() {
                    return Ok(self.cached(key, || {
                        self.non_strict_table(td, state, tag, fresh, empty)
                    }));
                }
                if empty {
                    let mut terms: Vec<Terminal> = Vec::new();
                    if tag {
                        terms.extend(
                            td.states[state as usize]
                                .iter()
                                .map(|p| p.term)
                                .filter(|t| {
                                    matches!(
                                        t,
                                        Terminal::AT(_)
                                            | Terminal::AtUri(_)
                                            | Terminal::AtAny
                                    )
                                }),
                        );
                    }
                    terms.push(Terminal::EE);
                    return Ok(self.cached(key, || CodeTable::flat(terms)));
                }
                let prods = &td.states[state as usize];
                if state == 0 && fresh && (td.castable || nillable) {
                    return Ok(self.cached(key, || {
                        let mut t =
                            CodeTable::flat(prods.iter().map(|p| p.term));
                        let n = prods.len() as u32;
                        let mut k = 0;
                        if td.castable {
                            t.push(Terminal::AT(XSI_TYPE), &[n, k]);
                            k += 1;
                        }
                        if nillable {
                            t.push(Terminal::AT(XSI_NIL), &[n, k]);
                        }
                        t
                    }));
                }
                Ok(Table::Flat(prods))
            }
        }
    }

    /// The options bits of a [`TableKey`](crate::grammar::TableKey): strictness and what
    /// is kept.
    fn keep_bits(&self) -> u16 {
        let k = self.keep;
        (self.opts.strict() as u16)
            | (k.ns as u16) << 1
            | (k.sc as u16) << 2
            | (k.dtd as u16) << 3
            | (k.comments as u16) << 4
            | (k.pis as u16) << 5
    }

    /// The table for `key`, cached per stream and shared across streams
    /// via `grammars` when `std` is enabled.
    fn cached(
        &self,
        key: crate::grammar::TableKey,
        make: impl FnOnce() -> CodeTable,
    ) -> Table<'_> {
        if let Ok(t) = core::cell::Ref::filter_map(self.tables.borrow(), |m| {
            m.get(&key).map(|t| &**t)
        }) {
            return Table::Ref(t);
        }
        #[cfg(feature = "std")]
        let t = {
            let cache = self.grammars.table_cache();
            let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
            map.entry(key)
                .or_insert_with(|| alloc::sync::Arc::new(make()))
                .clone()
        };
        #[cfg(not(feature = "std"))]
        let t = alloc::sync::Arc::new(make());
        self.tables.borrow_mut().insert(key, t);
        Table::Ref(core::cell::Ref::map(self.tables.borrow(), |m| &*m[&key]))
    }

    /// The code table of an informed state without strict: its
    /// productions, then those EXI adds so that any content can be
    /// encoded (EXI 8.5.4.4.1), in EXIficient's order:
    ///
    /// | State | Second level, in order |
    /// |---|---|
    /// | First start tag | EE (when no first-level EE), AT(xsi:type), AT(xsi:nil), AT(\*), AT [untyped value] (third level: each declared AT(qname), then AT(\*)), NS, SC, SE(\*), CH [untyped value], ER, CM and PI (third level) |
    /// | Other start tag | EE, AT(\*), AT [untyped value], SE(\*), CH, ER, CM and PI |
    /// | Element content | EE, SE(\*), CH, ER, CM and PI |
    ///
    /// NS, SC, ER, CM and PI are kept only when the options keep them.
    fn non_strict_table(
        &self,
        td: &crate::grammar::informed::TypeDef,
        state: u32,
        tag: bool,
        fresh: bool,
        empty: bool,
    ) -> CodeTable {
        let prods = &td.states[state as usize];
        let start = tag;
        let first_level: Vec<Terminal> = if empty {
            let mut v: Vec<Terminal> = if start {
                prods
                    .iter()
                    .map(|p| p.term)
                    .filter(|t| {
                        matches!(
                            t,
                            Terminal::AT(_)
                                | Terminal::AtUri(_)
                                | Terminal::AtAny
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            v.push(Terminal::EE);
            v
        } else {
            prods.iter().map(|p| p.term).collect()
        };
        let n = first_level.len() as u32;
        let has_ee = first_level.contains(&Terminal::EE);
        let declared_count = first_level
            .iter()
            .filter(|t| matches!(t, Terminal::AT(_)))
            .count() as u32;
        let mut full: Vec<(Terminal, EventCode)> = first_level
            .iter()
            .enumerate()
            .map(|(i, t)| (*t, EventCode::new(&[i as u32])))
            .collect();
        let mut k = 0u32;
        let second = |full: &mut Vec<(Terminal, EventCode)>,
                      k: &mut u32,
                      t: Terminal| {
            full.push((t, EventCode::new(&[n, *k])));
            *k += 1;
        };
        if !has_ee {
            second(&mut full, &mut k, Terminal::EE);
        }
        let first = fresh;
        if first {
            second(&mut full, &mut k, Terminal::AT(XSI_TYPE));
            second(&mut full, &mut k, Terminal::AT(XSI_NIL));
        }
        if first || start {
            second(&mut full, &mut k, Terminal::AtAny);
            let mut j = 0u32;
            for t in &first_level {
                if matches!(t, Terminal::AT(_)) {
                    full.push((*t, EventCode::new(&[n, k, j])));
                    j += 1;
                }
            }
            full.push((Terminal::AtAny, EventCode::new(&[n, k, declared_count])));
            k += 1;
        }
        if first {
            second(&mut full, &mut k, Terminal::NS);
            second(&mut full, &mut k, Terminal::SC);
        }
        second(&mut full, &mut k, Terminal::SeAny);
        second(&mut full, &mut k, Terminal::CH);
        second(&mut full, &mut k, Terminal::ER);
        full.push((Terminal::CM, EventCode::new(&[n, k, 0])));
        full.push((Terminal::PI, EventCode::new(&[n, k, 1])));
        pruned(&full, self.keep)
    }

    /// Whether `code` is one of the productions a non-strict informed
    /// state adds, rather than one of its own.
    pub fn deviates(&self, code: EventCode) -> bool {
        code.len() > 1
            && matches!(
                self.stack.last().map(|f| f.kind),
                Some(FrameKind::Informed { .. })
            )
    }

    /// The schema production of the current informed state with
    /// terminal `t`.
    pub fn iprod(&self, t: Terminal) -> Option<&'g IProd> {
        let f = self.stack.last()?;
        let FrameKind::Informed { ty, state, .. } = f.kind else {
            return None;
        };
        let td = self.schema?.ty(ty);
        td.states.get(state as usize)?.iter().find(|p| p.term == t)
    }

    /// True when the current state is an informed state 0 offering
    /// xsi:type or xsi:nil through the extra productions strict adds.
    pub fn xsi_slot(&self, t: Terminal) -> Option<Slot> {
        let f = self.stack.last()?;
        let FrameKind::Informed {
            ty,
            fresh: true,
            empty: false,
            nillable,
            ..
        } = f.kind
        else {
            return None;
        };
        let td = self.schema?.ty(ty);
        match t {
            Terminal::AT(XSI_TYPE) if td.castable => Some(Slot::TypeName),
            Terminal::AT(XSI_NIL) if nillable => {
                Some(if self.lexical() { Slot::NilText } else { Slot::Nil })
            }
            _ => None,
        }
    }

    /// How the value of an AT event with terminal `t`, code `code` and
    /// name `id` is coded.
    pub fn at_slot(&self, t: Terminal, code: EventCode, id: NameId) -> Slot {
        if self.opts.strict() {
            if let Some(s) = self.xsi_slot(t) {
                return s;
            }
        } else if self.deviates(code) {
            // Added productions: xsi:type and xsi:nil typed as EXI
            // says; the rest untyped.
            return match (t, code.len()) {
                (Terminal::AT(XSI_TYPE), 2) if !self.lexical() => Slot::QName,
                (Terminal::AT(XSI_NIL), 2) if !self.lexical() => Slot::Nil,
                _ => Slot::Str,
            };
        }
        if let Some(p) = self.iprod(t) {
            if let Some(ty) = p.ty {
                return if self.lexical() {
                    Slot::Lexical(ty)
                } else {
                    Slot::Typed(ty)
                };
            }
        }
        if id == XSI_TYPE
            && !self.lexical()
            && matches!(
                self.stack.last().map(|f| f.kind),
                Some(FrameKind::Builtin { .. })
            )
        {
            return Slot::QName;
        }
        Slot::Str
    }

    /// How the value of a CH event with code `code` is coded.
    pub fn ch_slot(&self, code: EventCode) -> Slot {
        if self.deviates(code) {
            return Slot::Str;
        }
        match self.iprod(Terminal::CH).and_then(|p| p.ty) {
            Some(ty) if self.lexical() => Slot::Lexical(ty),
            Some(ty) => Slot::Typed(ty),
            None => Slot::Str,
        }
    }

    /// Whether a declared CH leads to a state with an EE production.
    pub fn can_end_after_characters(&self) -> bool {
        let (Some(schema), Some(prod), Some(FrameKind::Informed { ty, .. })) = (
            self.schema,
            self.iprod(Terminal::CH),
            self.stack.last().map(|f| f.kind),
        ) else {
            return false;
        };
        schema
            .ty(ty)
            .states
            .get(prod.next as usize)
            .is_some_and(|state| state.iter().any(|p| p.term == Terminal::EE))
    }

    /// The declaration an SE with terminal `t`, code `code` and name
    /// `id` enters, or `None` for a built-in element grammar.
    pub fn child_for(
        &self,
        t: Terminal,
        code: EventCode,
        id: NameId,
    ) -> Option<ElementId> {
        if self.deviates(code) {
            return self.schema?.global_named(id);
        }
        let schema = self.schema?;
        match self.stack.last() {
            None if self.in_sc() => None,
            None => {
                let (table, elems, _) = self.informed_top()?;
                let i = table.entries().iter().position(|(x, _)| *x == t)?;
                elems[i].or_else(|| schema.global_named(id))
            }
            Some(Frame { kind: FrameKind::Informed { .. }, .. }) => {
                match self.iprod(t) {
                    Some(IProd { elem: Some(e), .. }) => Some(*e),
                    _ => schema.global_named(id),
                }
            }
            // In a built-in grammar of a schema-informed stream, an
            // element with a global declaration uses it (XHTML inside
            // SVG's foreignObject, say).
            Some(_) => schema.global_named(id),
        }
    }

    /// Checks the depth limit for one more element.
    pub fn check_depth(&self) -> Result<()> {
        if self.depth() >= self.limits.get_max_depth() {
            return Err(err(ErrorKind::Limit, "max_depth reached"));
        }
        Ok(())
    }

    /// The state after an added production (`deviates`) that leaves the
    /// start tag (SE, CH, ER, CM, PI): from the start tag, the first
    /// content state; in content, the same.
    fn after_deviation(
        state: &mut u32,
        tag: &mut bool,
        fresh: &mut bool,
        content: u32,
    ) {
        if *tag {
            *state = content;
        }
        *tag = false;
        *fresh = false;
    }

    /// A move along a production of the state. An attribute keeps the
    /// start tag, and the first start tag while it loops to state 0;
    /// anything else ends the start tag.
    fn follow(
        state: &mut u32,
        tag: &mut bool,
        fresh: &mut bool,
        next: u32,
        attribute: bool,
    ) {
        *fresh = *fresh && attribute && *state == 0 && next == 0;
        *tag = *tag && attribute;
        *state = next;
    }

    fn content_of(&self, ty: TypeId) -> Result<u32> {
        Ok(self.schema()?.ty(ty).content)
    }

    /// SE with terminal `t`, code `code` and name `id`: learns, moves
    /// the parent, pushes the child.
    pub fn on_se(
        &mut self,
        t: Terminal,
        code: EventCode,
        id: NameId,
        prefix: Option<u32>,
    ) -> Result<()> {
        self.check_depth()?;
        let dev = self.deviates(code);
        let child = self.child_for(t, code, id);
        // Everything that can fail is checked before anything changes:
        // a new element grammar and a learned production may each need
        // one unit of budget.
        let new_grammar = child.is_none() && !self.elems.contains_key(&id);
        let learns =
            t == Terminal::SeAny
                && (self.stack.last().is_some_and(|f| {
                    matches!(f.kind, FrameKind::Builtin { .. })
                }) || (self.stack.is_empty()
                    && self.fragment()
                    && (self.informed_top().is_none() || self.in_sc())));
        if (new_grammar as u64 + learns as u64) > self.budget {
            return Err(err(
                ErrorKind::Limit,
                "max_learned_productions reached",
            ));
        }
        let next = self.iprod(t).map(|p| p.next);
        let content = match self.stack.last().map(|f| f.kind) {
            Some(FrameKind::Informed { ty, .. }) => self.content_of(ty)?,
            _ => 0,
        };
        match self.stack.last_mut() {
            None => {
                if self.top != Top::Content {
                    return Err(internal("SE after the document element"));
                }
                if learns {
                    #[cfg(feature = "std")]
                    let table = self.builtin_top.get_or_insert_with(|| {
                        self.top_tables.builtin.clone()
                    });
                    #[cfg(not(feature = "std"))]
                    let table =
                        self.builtin_top.as_mut().expect("owned tables");
                    table.learn_fragment_se(id, &mut self.budget)?;
                }
                if !self.fragment() {
                    self.top = Top::End;
                }
            }
            Some(f) => match &mut f.kind {
                FrameKind::Builtin { content } => {
                    let g = self.elems.get_mut(&f.name).ok_or_else(|| {
                        internal("open element without a grammar")
                    })?;
                    if !*content {
                        if t == Terminal::SeAny {
                            g.learn_start_tag(
                                Learn::SE(id),
                                &mut self.budget,
                            )?;
                        }
                        *content = true;
                    } else if t == Terminal::SeAny {
                        g.learn_content(Learn::SE(id), &mut self.budget)?;
                    }
                }
                FrameKind::Informed { state, tag, fresh, .. } => {
                    if dev {
                        Self::after_deviation(state, tag, fresh, content);
                    } else {
                        Self::follow(
                            state,
                            tag,
                            fresh,
                            next.ok_or_else(|| {
                                internal("SE without a production")
                            })?,
                            false,
                        );
                    }
                }
            },
        }
        let kind = match child {
            Some(e) => {
                let ed = self.schema()?.elem(e);
                FrameKind::Informed {
                    ty: ed.ty,
                    state: 0,
                    tag: true,
                    fresh: true,
                    empty: false,
                    nillable: ed.nillable,
                }
            }
            None => {
                if new_grammar {
                    self.budget -= 1;
                    self.elems.insert(id, ElementGrammar::new(self.keep));
                }
                FrameKind::Builtin { content: false }
            }
        };
        self.stack.push(Frame { name: id, prefix, kind });
        Ok(())
    }

    /// AT with terminal `t`, code `code` and name `id` (not xsi:type
    /// nor xsi:nil in an informed state).
    pub fn on_at(
        &mut self,
        t: Terminal,
        code: EventCode,
        id: NameId,
    ) -> Result<()> {
        let dev = self.deviates(code);
        let next = self.iprod(t).map(|p| p.next);
        let f = self
            .stack
            .last_mut()
            .ok_or_else(|| internal("AT outside an element"))?;
        match &mut f.kind {
            FrameKind::Builtin { content: false } => {
                if t == Terminal::AtAny {
                    let g = self.elems.get_mut(&f.name).ok_or_else(|| {
                        internal("open element without a grammar")
                    })?;
                    let l = if id == XSI_TYPE {
                        Learn::XsiType(id)
                    } else {
                        Learn::AT(id)
                    };
                    g.learn_start_tag(l, &mut self.budget)?;
                }
                Ok(())
            }
            FrameKind::Builtin { content: true } => {
                Err(internal("AT in element content"))
            }
            FrameKind::Informed { state, tag, fresh, .. } => {
                // An untyped value of a declared attribute (three
                // parts, AT(qname)) moves as the declared production
                // does; AT(*), declared or not, stays.
                if dev && !(code.len() == 3 && matches!(t, Terminal::AT(_))) {
                    return Ok(());
                }
                Self::follow(
                    state,
                    tag,
                    fresh,
                    next.ok_or_else(|| internal("AT without a production"))?,
                    true,
                );
                Ok(())
            }
        }
    }

    /// Checks the strict xsi:type cast. Readers report a grammar error
    /// and writers report an event they cannot encode.
    pub fn strict_xsi_type(
        &self,
        uri: &str,
        local: &str,
        kind: ErrorKind,
    ) -> Result<TypeId> {
        let schema = self.schema.ok_or_else(|| internal("no schema"))?;
        let ty = schema.type_named(uri, local).ok_or_else(|| {
            err(kind, "xsi:type names no type of the schema")
        })?;
        let Some(FrameKind::Informed { ty: current, .. }) =
            self.stack.last().map(|f| f.kind)
        else {
            return Err(internal("xsi:type outside an informed element"));
        };
        if !schema.ty(current).casts.contains(&ty) {
            return Err(err(
                kind,
                "xsi:type names a type that does not derive from the element's type",
            ));
        }
        Ok(ty)
    }

    /// xsi:type naming type `ty`: its grammar starts, in its first
    /// start tag. `None`: a name with no type in the schema (only
    /// without strict), and nothing changes.
    pub fn on_xsi_type(&mut self, ty: Option<TypeId>) -> Result<()> {
        match self.stack.last_mut().map(|f| &mut f.kind) {
            Some(FrameKind::Informed { ty: t, state, tag, fresh, .. }) => {
                if let Some(ty) = ty {
                    *t = ty;
                    *state = 0;
                    *tag = true;
                    *fresh = true;
                }
                Ok(())
            }
            _ => Err(internal("xsi:type outside an informed element")),
        }
    }

    /// Strict xsi:nil.
    pub fn on_xsi_nil(&mut self, nil: bool) -> Result<()> {
        match self.stack.last_mut().map(|f| &mut f.kind) {
            Some(FrameKind::Informed { empty, .. }) => {
                // TypeEmpty is a first start tag too (EXIficient), so
                // `fresh` stays.
                *empty |= nil;
                Ok(())
            }
            _ => Err(internal("xsi:nil outside an informed element")),
        }
    }

    /// CH with code `code`.
    pub fn on_ch(&mut self, code: EventCode) -> Result<()> {
        let dev = self.deviates(code);
        let next = self.iprod(Terminal::CH).map(|p| p.next);
        let content = match self.stack.last().map(|f| f.kind) {
            Some(FrameKind::Informed { ty, .. }) => self.content_of(ty)?,
            _ => 0,
        };
        let f = self
            .stack
            .last_mut()
            .ok_or_else(|| internal("CH outside an element"))?;
        match &mut f.kind {
            FrameKind::Builtin { content } => {
                let g = self.elems.get_mut(&f.name).ok_or_else(|| {
                    internal("open element without a grammar")
                })?;
                if *content {
                    g.learn_content(Learn::CH, &mut self.budget)?;
                } else {
                    g.learn_start_tag(Learn::CH, &mut self.budget)?;
                    *content = true;
                }
            }
            FrameKind::Informed { state, tag, fresh, .. } => {
                if dev {
                    Self::after_deviation(state, tag, fresh, content);
                } else {
                    Self::follow(
                        state,
                        tag,
                        fresh,
                        next.ok_or_else(|| {
                            internal("CH without a production")
                        })?,
                        false,
                    );
                }
            }
        }
        Ok(())
    }

    /// EE: learns, pops, and returns the element that ended.
    pub fn on_ee(&mut self) -> Result<Frame> {
        let f = *self
            .stack
            .last()
            .ok_or_else(|| internal("EE outside an element"))?;
        if let FrameKind::Builtin { content: false } = f.kind {
            let g = self
                .elems
                .get_mut(&f.name)
                .ok_or_else(|| internal("open element without a grammar"))?;
            g.learn_start_tag(Learn::EE, &mut self.budget)?;
        }
        self.stack.pop();
        Ok(f)
    }

    /// NS, CM, PI, DT, ER, with code `code`.
    pub fn on_misc(&mut self, t: Terminal, code: EventCode) -> Result<()> {
        let dev = self.deviates(code);
        let content = match self.stack.last().map(|f| f.kind) {
            Some(FrameKind::Informed { ty, .. }) => self.content_of(ty)?,
            _ => 0,
        };
        match self.stack.last_mut().map(|f| &mut f.kind) {
            Some(FrameKind::Builtin { content }) => {
                if !*content && t != Terminal::NS {
                    *content = true;
                }
            }
            // NS stays in the start tag; ER, CM and PI leave it.
            Some(FrameKind::Informed { state, tag, fresh, .. })
                if dev && t != Terminal::NS =>
            {
                Self::after_deviation(state, tag, fresh, content)
            }
            _ => {}
        }
        Ok(())
    }

    pub fn on_ed(&mut self) {
        self.top = Top::Done;
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::grammar::GrammarSet;
    use crate::strings::{GlobalConfig, GlobalStrings};

    #[test]
    fn shared_fragment_tables_keep_learning_local() -> Result<()> {
        let set = GrammarSet::schemaless();
        let strings = GlobalStrings::new(GlobalConfig::new())?;
        let grammars = set.attach(&strings)?;
        let opts = Options::builder().fragment(true).build()?;
        let mut first = State::new(opts.clone(), &grammars, Limits::new());
        let second = State::new(opts, &grammars, Limits::new());
        assert!(alloc::sync::Arc::ptr_eq(
            &first.top_tables,
            &second.top_tables
        ));

        let name = NameId { uri: 0, local: 0 };
        let code = first.table()?.code(Terminal::SeAny).unwrap();
        first.on_se(Terminal::SeAny, code, name, None)?;
        first.on_ee()?;
        assert_eq!(
            first.table()?.code(Terminal::SE(name)),
            Some(EventCode::new(&[0]))
        );
        assert_eq!(second.table()?.code(Terminal::SE(name)), None);
        assert_eq!(second.table()?.code(Terminal::SeAny), Some(code));

        let opts = Options::builder()
            .fragment(true)
            .preserve(Preserve::COMMENTS)
            .build()?;
        let with_comments = State::new(opts, &grammars, Limits::new());
        assert!(with_comments.table()?.code(Terminal::CM).is_some());
        assert!(second.table()?.code(Terminal::CM).is_none());
        Ok(())
    }
}
