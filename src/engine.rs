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

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use hashbrown::HashMap;
use rustc_hash::FxBuildHasher;

use crate::coding::datatypes::*;
use crate::coding::names::{self, ReadTables, ValueRead};
use crate::coding::{Channel, Decoder, Encoder, width};
use crate::grammar::builtin::{
    CodeTable, ElementGrammar, Keep, Learn, TopLevel,
};
use crate::grammar::informed::{
    ElementId, IProd, IntegerRep, Representation, Schema, SimpleTypeId, TypeId,
};
use crate::grammar::{EventCode, Grammars, Terminal};
use crate::lexical;
use crate::options::{Coding, SchemaId};
use crate::strings::ValueId;
use crate::tables::{SessionTables, ValueRules};
use crate::value::{BinaryKind, OwnedValue, Value};
use crate::{
    Error, ErrorKind, Limits, NameId, Options, OwnedName, Preserve, Result,
};

pub(crate) const XSI_NIL: NameId = NameId { uri: 2, local: 0 };
pub(crate) const XSI_TYPE: NameId = NameId { uri: 2, local: 1 };

type Map<K, V> = HashMap<K, V, FxBuildHasher>;

/// A code table from full codes, keeping the productions the options
/// keep.
fn pruned(full: &[(Terminal, Vec<u32>)], keep: Keep) -> CodeTable {
    let v: Vec<(Terminal, &[u32])> =
        full.iter().map(|(t, p)| (*t, p.as_slice())).collect();
    CodeTable::pruned(&v, crate::grammar::builtin::keeps(keep))
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

/// The Boolean character set of EXI Table 7-2.
fn boolean_charset() -> Box<[char]> {
    let mut v: Vec<char> = "\t\n\r 01aeflrstu".chars().collect();
    v.sort_unstable();
    v.into_boxed_slice()
}

/// The restricted character set a lexical value of `ty` is coded with
/// (EXI 7.1.10 and Table 7-2): the representation's, the item type's
/// for a list, the base type's for an enumeration; for strings, the set
/// a pattern gives, if any. Sorted, as charsets are.
pub(crate) fn lexical_charset(
    schema: &Schema,
    ty: SimpleTypeId,
) -> Option<Box<[char]>> {
    const WS: &str = "\t\n\r ";
    let set = |chars: &str| -> Option<Box<[char]>> {
        let mut v: Vec<char> = WS.chars().chain(chars.chars()).collect();
        v.sort_unstable();
        Some(v.into_boxed_slice())
    };
    match schema.rep(ty) {
        Representation::String { charset } => charset.clone(),
        Representation::Boolean { .. } => set("01aeflrstu"),
        Representation::Integer(_) => set("+-0123456789"),
        Representation::Decimal => set("+-.0123456789"),
        Representation::Float => set("+-.0123456789EFINae"),
        Representation::Binary(BinaryKind::Hex) => {
            set("0123456789ABCDEFabcdef")
        }
        Representation::Binary(BinaryKind::Base64) => set(
            "+/0123456789=ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        ),
        Representation::DateTime(_) => set("+-.0123456789:TZ"),
        Representation::List(item) => lexical_charset(schema, *item),
        Representation::Enumeration { base, .. } => {
            lexical_charset(schema, *base)
        }
    }
}

/// The event codes of the current state. Ordinary strict states use
/// production indexes directly as their one-part codes.
pub(crate) enum Table<'a> {
    Flat(&'a [IProd]),
    Borrowed(&'a CodeTable),
    Shared(alloc::sync::Arc<CodeTable>),
}

impl Table<'_> {
    fn explicit(&self) -> Option<&CodeTable> {
        match self {
            Table::Flat(_) => None,
            Table::Borrowed(t) => Some(t),
            Table::Shared(t) => Some(t),
        }
    }

    pub fn code(&self, term: Terminal) -> Option<EventCode> {
        match self {
            Table::Flat(prods) => prods
                .iter()
                .position(|p| p.term == term)
                .map(|i| EventCode::new(&[i as u32])),
            Table::Borrowed(t) => t.code(term),
            Table::Shared(t) => t.code(term),
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
            Table::Shared(t) => t.count(prefix),
        }
    }

    fn lookup(&self, parts: &[u32]) -> Option<Terminal> {
        match self {
            Table::Flat(prods) => match parts {
                &[i] => prods.get(i as usize).map(|p| p.term),
                _ => None,
            },
            Table::Borrowed(t) => t.lookup(parts),
            Table::Shared(t) => t.lookup(parts),
        }
    }
}

pub(crate) struct State<'g> {
    pub schema: Option<&'g Schema>,
    #[cfg(feature = "std")]
    grammars: &'g Grammars<'g>,
    /// Without std the built tables are kept per stream.
    #[cfg(not(feature = "std"))]
    tables: core::cell::RefCell<crate::grammar::TableCache>,
    pub opts: Options,
    pub keep: Keep,
    pub limits: Limits,
    pub budget: u64,
    builtin_top: TopLevel,
    /// Schema-informed top level: the content table and the declaration
    /// of each of its entries, then the DocEnd table.
    informed_top: Option<(CodeTable, Vec<Option<ElementId>>, CodeTable)>,
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
    builtin_top: TopLevel,
    top: Top,
    elems: Map<NameId, ElementGrammar>,
    stack: Vec<Frame>,
}

impl<'g> State<'g> {
    pub fn new(
        opts: Options,
        g: &'g Grammars<'g>,
        limits: Limits,
    ) -> State<'g> {
        let keep = Keep::from_options(&opts);
        let schema = g.set().schema.as_ref();
        let builtin_top = if opts.fragment() {
            TopLevel::fragment(keep)
        } else {
            TopLevel::document(keep)
        };
        let informed_top = schema.map(|s| {
            let list: Vec<ElementId> = if opts.fragment() {
                s.fragment.clone().unwrap_or_default()
            } else {
                s.globals.clone()
            };
            let mut terms: Vec<Terminal> =
                list.iter().map(|e| Terminal::SE(s.elem(*e).id)).collect();
            let mut elems: Vec<Option<ElementId>> =
                list.into_iter().map(Some).collect();
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
            let mut full: Vec<(Terminal, Vec<u32>)> = terms
                .into_iter()
                .enumerate()
                .map(|(i, t)| (t, alloc::vec![i as u32]))
                .collect();
            let mut end: Vec<(Terminal, Vec<u32>)> =
                alloc::vec![(Terminal::ED, alloc::vec![0])];
            if opts.fragment() {
                full.push((Terminal::CM, alloc::vec![n, 0]));
                full.push((Terminal::PI, alloc::vec![n, 1]));
            } else {
                full.push((Terminal::DT, alloc::vec![n, 0]));
                full.push((Terminal::CM, alloc::vec![n, 1, 0]));
                full.push((Terminal::PI, alloc::vec![n, 1, 1]));
                end.push((Terminal::CM, alloc::vec![1, 0]));
                end.push((Terminal::PI, alloc::vec![1, 1]));
            }
            (pruned(&full, keep), elems, pruned(&end, keep))
        });
        State {
            schema,
            #[cfg(feature = "std")]
            grammars: g,
            #[cfg(not(feature = "std"))]
            tables: Default::default(),
            keep,
            budget: limits.get_max_learned_productions(),
            limits,
            builtin_top,
            informed_top,
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
        let fresh_top = TopLevel::fragment(self.keep);
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

    /// The code table of the current state.
    pub fn table(&self) -> Result<Table<'_>> {
        let informed_top =
            if self.in_sc() { &None } else { &self.informed_top };
        let Some(f) = self.stack.last() else {
            return Ok(match (self.top, informed_top) {
                (Top::Done, _) => Table::Flat(&[]),
                (Top::Content, Some((t, _, _))) => Table::Borrowed(t),
                (Top::End, Some((_, _, end))) => Table::Borrowed(end),
                (Top::Content, None) => {
                    Table::Borrowed(self.builtin_top.content())
                }
                (Top::End, None) => Table::Borrowed(self.builtin_top.end()),
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

    /// The table for `key`, built by `make` once per grammars (with
    /// std) or per stream.
    fn cached(
        &self,
        key: crate::grammar::TableKey,
        make: impl FnOnce() -> CodeTable,
    ) -> Table<'_> {
        #[cfg(feature = "std")]
        let cache = self.grammars.table_cache();
        #[cfg(feature = "std")]
        let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(not(feature = "std"))]
        let mut map = self.tables.borrow_mut();
        if let Some(t) = map.get(&key) {
            return Table::Shared(t.clone());
        }
        let t = alloc::sync::Arc::new(make());
        map.insert(key, t.clone());
        Table::Shared(t)
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
        let declared: Vec<Terminal> = first_level
            .iter()
            .copied()
            .filter(|t| matches!(t, Terminal::AT(_)))
            .collect();
        let mut full: Vec<(Terminal, Vec<u32>)> = first_level
            .iter()
            .enumerate()
            .map(|(i, t)| (*t, alloc::vec![i as u32]))
            .collect();
        let mut k = 0u32;
        let second = |full: &mut Vec<(Terminal, Vec<u32>)>,
                      k: &mut u32,
                      t: Terminal| {
            full.push((t, alloc::vec![n, *k]));
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
            for (j, t) in declared.iter().enumerate() {
                full.push((*t, alloc::vec![n, k, j as u32]));
            }
            full.push((
                Terminal::AtAny,
                alloc::vec![n, k, declared.len() as u32],
            ));
            k += 1;
        }
        if first {
            second(&mut full, &mut k, Terminal::NS);
            second(&mut full, &mut k, Terminal::SC);
        }
        second(&mut full, &mut k, Terminal::SeAny);
        second(&mut full, &mut k, Terminal::CH);
        second(&mut full, &mut k, Terminal::ER);
        full.push((Terminal::CM, alloc::vec![n, k, 0]));
        full.push((Terminal::PI, alloc::vec![n, k, 1]));
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
                let (table, elems, _) = self.informed_top.as_ref()?;
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
                    && (self.informed_top.is_none() || self.in_sc())));
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
                    self.builtin_top
                        .learn_fragment_se(id, &mut self.budget)?;
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

// ---------------------------------------------------------------------------------------
// A counting view of the tables, for trying an event on input that may
// be incomplete.
// ---------------------------------------------------------------------------------------

/// [`ReadTables`] over session tables that changes nothing: additions
/// are only counted, so numbers come out as the real additions would
/// give them. Lookups of entries added in the same event return `None`;
/// an event never refers to an entry it adds.
pub(crate) struct Probe<'p, 'a> {
    t: &'p SessionTables<'a>,
    uris: u32,
    locals: Vec<(u32, u32)>,
    prefixes: Vec<(u32, u32)>,
}

impl<'p, 'a> Probe<'p, 'a> {
    pub fn new(t: &'p SessionTables<'a>) -> Self {
        Probe { t, uris: 0, locals: Vec::new(), prefixes: Vec::new() }
    }
}

fn bump(v: &mut Vec<(u32, u32)>, uri: u32) {
    match v.iter_mut().find(|(u, _)| *u == uri) {
        Some((_, n)) => *n += 1,
        None => v.push((uri, 1)),
    }
}

fn added(v: &[(u32, u32)], uri: u32) -> u32 {
    v.iter().find(|(u, _)| *u == uri).map_or(0, |(_, n)| *n)
}

impl ReadTables for Probe<'_, '_> {
    fn uri_count(&self) -> u32 {
        self.t.uri_count() + self.uris
    }
    fn add_uri(&mut self, _: &str) -> Result<u32> {
        let n = self.uri_count();
        self.uris += 1;
        Ok(n)
    }
    fn local_count(&self, uri: u32) -> u32 {
        let base =
            if uri < self.t.uri_count() { self.t.local_count(uri) } else { 0 };
        base + added(&self.locals, uri)
    }
    fn add_local(&mut self, uri: u32, _: &str) -> Result<u32> {
        let n = self.local_count(uri);
        bump(&mut self.locals, uri);
        Ok(n)
    }
    fn prefix_count(&self, uri: u32) -> u32 {
        let base = if uri < self.t.uri_count() {
            self.t.prefix_count(uri)
        } else {
            0
        };
        base + added(&self.prefixes, uri)
    }
    fn add_prefix(&mut self, uri: u32, _: &str) -> Result<u32> {
        let n = self.prefix_count(uri);
        bump(&mut self.prefixes, uri);
        Ok(n)
    }
    fn rules(&self) -> ValueRules {
        self.t.rules()
    }
    fn local_value_count(&self, name: NameId) -> u32 {
        self.t.local_value_count(name)
    }
    fn global_value_count(&self) -> u32 {
        self.t.global_value_count()
    }
    fn local_value(&self, name: NameId, n: u32) -> Option<ValueId> {
        self.t.local_value(name, n)
    }
    fn global_value(&self, n: u32) -> Option<ValueId> {
        self.t.global_value(n)
    }
    fn add_value(&mut self, _: NameId, _: &str) -> Result<Option<ValueId>> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------------------
// Decoding (reader)
// ---------------------------------------------------------------------------------------

/// A decoded value, before names are turned into strings.
#[derive(Clone, Debug, Default)]
pub(crate) enum DVal {
    #[default]
    None,
    /// A string with the value tables: a hit, or a literal in
    /// `Decoded::text`.
    Str(ValueRead),
    /// A name: URI number, local-name number, prefix number.
    QName(NameId, Option<u32>),
    Nil(bool),
    Typed(OwnedValue),
}

/// One decoded event, in numbers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Decoded {
    pub term: Option<Terminal>,
    pub code: Option<EventCode>,
    pub name: Option<(NameId, Option<u32>)>,
    pub value: DVal,
    pub slot: Option<Slot>,
    pub ns: Option<(u32, u32, bool)>,
    /// Literal text: a string value, or the strings of CM, PI, DT, ER
    /// in order.
    pub text: String,
    pub texts: Vec<String>,
    /// The value goes in a value channel and was not decoded
    /// (pre-compression and compression decode a block's values after
    /// its structure).
    pub deferred: bool,
}

impl Decoded {
    pub fn clear(&mut self) {
        self.term = None;
        self.code = None;
        self.name = None;
        self.value = DVal::None;
        self.slot = None;
        self.ns = None;
        self.text.clear();
        self.texts.clear();
        self.deferred = false;
    }
}

fn read_string<D: Decoder>(d: &mut D, limits: &Limits) -> Result<String> {
    let len =
        read_uint(d, Channel::Structure, limits.get_max_integer_octets())?;
    if len > limits.get_max_value_chars() {
        return Err(err(
            ErrorKind::Limit,
            "string longer than max_value_chars",
        ));
    }
    let mut s = String::new();
    read_chars(d, Channel::Structure, len, &mut s)?;
    Ok(s)
}

fn read_prefix<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &T,
    uri: u32,
) -> Result<Option<u32>> {
    let m = t.prefix_count(uri);
    if m == 0 {
        return Ok(None);
    }
    let n = d.nbit(Channel::Structure, width(m))?;
    if n >= m {
        return Err(err(
            ErrorKind::Grammar,
            "prefix number beyond the partition",
        ));
    }
    Ok(Some(n))
}

/// True when a value of this slot and name travels in a value channel:
/// everything but `xsi:type` and a typed `xsi:nil`, which choose the
/// grammar (EXI 9.2.1).
pub(crate) fn in_value_channel(slot: Slot, name: NameId) -> bool {
    matches!(slot, Slot::Str | Slot::Typed(_) | Slot::Lexical(_))
        && name != XSI_TYPE
}

/// Decodes one event at the decoder's position. With `defer`, a value
/// that belongs in a value channel is left for later
/// ([`Decoded::deferred`]).
pub(crate) fn decode<D: Decoder, T: ReadTables + ?Sized>(
    st: &State<'_>,
    d: &mut D,
    t: &mut T,
    out: &mut Decoded,
    defer: bool,
) -> Result<()> {
    let limits = st.limits;
    let table = st.table()?;
    let mut parts = [0; 3];
    let mut len = 0;
    let term = loop {
        let c = table.count(&parts[..len]);
        if c == 0 {
            return Err(err(ErrorKind::Grammar, "no event may follow here"));
        }
        parts[len] = read_code_part(d, c)?;
        len += 1;
        if let Some(term) = table.lookup(&parts[..len]) {
            break term;
        }
        if len == parts.len() {
            return Err(internal("event code longer than three parts"));
        }
    };
    out.term = Some(term);
    let code = EventCode::new(&parts[..len]);
    out.code = Some(code);
    let prefixes = st.prefixes();
    let qname = |d: &mut D,
                 t: &mut T,
                 term: Terminal|
     -> Result<(NameId, Option<u32>)> {
        match term {
            Terminal::SE(id) | Terminal::AT(id) => {
                let p =
                    if prefixes { read_prefix(d, t, id.uri)? } else { None };
                Ok((id, p))
            }
            Terminal::SeUri(u) | Terminal::AtUri(u) => {
                let local = names::read_local(d, t, u, &limits)?;
                let p = if prefixes { read_prefix(d, t, u)? } else { None };
                Ok((NameId { uri: u, local }, p))
            }
            _ => names::read_name(d, t, prefixes, &limits),
        }
    };
    match term {
        Terminal::SE(_) | Terminal::SeUri(_) | Terminal::SeAny => {
            out.name = Some(qname(d, t, term)?);
        }
        Terminal::AT(_) | Terminal::AtUri(_) | Terminal::AtAny => {
            let (id, p) = qname(d, t, term)?;
            out.name = Some((id, p));
            let slot = st.at_slot(term, code, id);
            out.slot = Some(slot);
            if defer && in_value_channel(slot, id) {
                out.deferred = true;
            } else {
                out.value = decode_value(st, d, t, slot, id, &mut out.text)?;
            }
        }
        Terminal::CH => {
            let f = st
                .stack
                .last()
                .ok_or_else(|| internal("CH outside an element"))?;
            let slot = st.ch_slot(code);
            out.slot = Some(slot);
            if defer && in_value_channel(slot, f.name) {
                out.deferred = true;
            } else {
                out.value =
                    decode_value(st, d, t, slot, f.name, &mut out.text)?;
            }
        }
        Terminal::NS => out.ns = Some(names::read_ns(d, t, &limits)?),
        Terminal::CM | Terminal::ER => {
            out.texts.push(read_string(d, &limits)?)
        }
        Terminal::PI => {
            out.texts.push(read_string(d, &limits)?);
            out.texts.push(read_string(d, &limits)?);
        }
        Terminal::DT => {
            for _ in 0..4 {
                out.texts.push(read_string(d, &limits)?);
            }
        }
        Terminal::SC | Terminal::EE | Terminal::ED | Terminal::SD => {}
    }
    Ok(())
}

pub(crate) fn decode_value<D: Decoder, T: ReadTables + ?Sized>(
    st: &State<'_>,
    d: &mut D,
    t: &mut T,
    slot: Slot,
    name: NameId,
    text: &mut String,
) -> Result<DVal> {
    let limits = st.limits;
    let ch = Channel::Value(name);
    match slot {
        Slot::Str => {
            Ok(DVal::Str(names::read_value(d, t, name, &limits, text)?))
        }
        Slot::QName | Slot::TypeName => {
            let uri = names::read_uri(d, t, &limits)?;
            let local = names::read_local(d, t, uri, &limits)?;
            let p = if st.prefixes() { read_prefix(d, t, uri)? } else { None };
            Ok(DVal::QName(NameId { uri, local }, p))
        }
        Slot::NilText => Ok(DVal::Str(read_value_charset(
            d,
            t,
            name,
            &limits,
            Some(&boolean_charset()),
            text,
        )?)),
        Slot::Lexical(ty) => {
            let schema = st
                .schema
                .ok_or_else(|| internal("typed slot without a schema"))?;
            let set = lexical_charset(schema, ty);
            Ok(DVal::Str(read_value_charset(
                d,
                t,
                name,
                &limits,
                set.as_deref(),
                text,
            )?))
        }
        Slot::Nil => Ok(DVal::Nil(read_bool(d, Channel::Structure)?)),
        Slot::Typed(ty) => {
            let schema = st
                .schema
                .ok_or_else(|| internal("typed value without a schema"))?;
            if let Representation::String { charset } = schema.rep(ty) {
                return Ok(DVal::Str(read_value_charset(
                    d,
                    t,
                    name,
                    &limits,
                    charset.as_deref(),
                    text,
                )?));
            }
            Ok(DVal::Typed(decode_typed(schema, d, ch, ty, &limits)?))
        }
    }
}

/// [`names::read_value`] with a restricted character set for literals.
fn read_value_charset<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &mut T,
    name: NameId,
    limits: &Limits,
    set: Option<&[char]>,
    out: &mut String,
) -> Result<ValueRead> {
    let Some(set) = set else {
        return names::read_value(d, t, name, limits, out);
    };
    let ch = Channel::Value(name);
    let v = read_uint(d, ch, limits.get_max_integer_octets())?;
    if v < 2 {
        // Hits are coded the same with or without a character set.
        return match v {
            0 => {
                let n = d.nbit(ch, width(t.local_value_count(name)))?;
                t.local_value(name, n).map(ValueRead::Hit).ok_or_else(|| {
                    err(ErrorKind::Grammar, "local value number not assigned")
                })
            }
            _ => {
                let n = d.nbit(ch, width(t.global_value_count()))?;
                t.global_value(n).map(ValueRead::Hit).ok_or_else(|| {
                    err(
                        ErrorKind::Grammar,
                        "global value number beyond the partition",
                    )
                })
            }
        };
    }
    let len = v - 2;
    if len > limits.get_max_value_chars() {
        return Err(err(
            ErrorKind::Limit,
            "literal longer than max_value_chars",
        ));
    }
    let mut s = String::new();
    read_restricted_chars(d, ch, len, set, &mut s)?;
    let kept = t.add_value(name, &s)?;
    out.push_str(&s);
    Ok(ValueRead::Literal(kept))
}

fn decode_typed<D: Decoder>(
    schema: &Schema,
    d: &mut D,
    ch: Channel,
    ty: SimpleTypeId,
    limits: &Limits,
) -> Result<OwnedValue> {
    let oct = limits.get_max_integer_octets();
    Ok(match schema.rep(ty) {
        Representation::String { charset } => {
            // Inside a list: a plain literal.
            let len = read_uint(d, ch, oct)?;
            if len > limits.get_max_value_chars() {
                return Err(err(
                    ErrorKind::Limit,
                    "string longer than max_value_chars",
                ));
            }
            let mut s = String::new();
            match charset {
                Some(set) => read_restricted_chars(d, ch, len, set, &mut s)?,
                None => read_chars(d, ch, len, &mut s)?,
            }
            OwnedValue::Str(s.into())
        }
        Representation::Boolean { pattern: false } => {
            OwnedValue::Bool(read_bool(d, ch)?)
        }
        Representation::Boolean { pattern: true } => {
            let (v, digit) = read_bool_pattern(d, ch)?;
            if digit { OwnedValue::BoolDigit(v) } else { OwnedValue::Bool(v) }
        }
        Representation::Integer(IntegerRep::Signed) => {
            let mut s = String::new();
            read_int_big(d, ch, oct, &mut s)?;
            match s.parse::<i64>() {
                Ok(v) => OwnedValue::Int(v),
                Err(_) => OwnedValue::BigInt(s.into()),
            }
        }
        Representation::Integer(IntegerRep::Unsigned) => {
            let mut s = String::new();
            read_uint_big(d, ch, oct, &mut s)?;
            match s.parse::<i64>() {
                Ok(v) => OwnedValue::Int(v),
                Err(_) => OwnedValue::BigInt(s.into()),
            }
        }
        Representation::Integer(IntegerRep::Bounded { min, max }) => {
            OwnedValue::Int(read_bounded(d, ch, *min, *max)?)
        }
        Representation::Decimal => {
            OwnedValue::Decimal(read_decimal(d, ch, oct)?)
        }
        Representation::Float => OwnedValue::Float(read_float(d, ch, oct)?),
        Representation::Binary(kind) => {
            let n =
                read_binary_len(d, ch, oct, limits.get_max_binary_bytes())?;
            let mut buf = alloc::vec![0u8; n as usize];
            d.octets(ch, &mut buf)?;
            OwnedValue::Binary(buf.into(), *kind)
        }
        Representation::DateTime(kind) => {
            OwnedValue::DateTime(read_datetime(d, ch, *kind, oct)?)
        }
        Representation::List(item) => {
            let n = read_uint(d, ch, oct)?;
            if n > limits.get_max_list_items() as u64 {
                return Err(err(
                    ErrorKind::Limit,
                    "list longer than max_list_items",
                ));
            }
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                items.push(decode_typed(schema, d, ch, *item, limits)?);
            }
            OwnedValue::List(items)
        }
        Representation::Enumeration { values, .. } => {
            let i = read_enum(d, ch, values.len() as u32)?;
            OwnedValue::Enum { ordinal: i, text: values[i as usize].clone() }
        }
    })
}

/// The string a [`DVal::Str`] stands for: a hit's table string, or the
/// literal.
pub(crate) fn value_str<'s>(
    t: &'s SessionTables<'_>,
    v: &ValueRead,
    literal: &'s str,
) -> &'s str {
    match v {
        ValueRead::Hit(id) => t.global_value_text(*id),
        ValueRead::Literal(_) => literal,
    }
}

// ---------------------------------------------------------------------------------------
// Encoding (writer)
// ---------------------------------------------------------------------------------------

/// A value checked against its slot and ready to write: nothing about
/// it can be refused any more.
#[derive(Clone, Debug)]
pub(crate) enum Prepared {
    Str(String, Option<Box<[char]>>),
    /// A name: URI, local name, prefix.
    QName(String, String, Option<String>),
    Bool(bool),
    BoolPattern(bool, bool),
    Int(i64),
    UInt(u64),
    BigUInt(String),
    Bounded(i64, i64, i64),
    Decimal(crate::Decimal),
    Float(crate::Float),
    Binary(Vec<u8>),
    DateTime(crate::DateTime),
    List(Vec<Prepared>),
    Enum(u32, u32),
}

fn unencodable(detail: &'static str) -> Error {
    err(ErrorKind::Unencodable, detail)
}

/// The text form of any value.
pub(crate) fn lexical_string(v: &Value<'_>) -> Result<String> {
    match v.as_str() {
        Some(s) => Ok(s.into()),
        None => {
            let mut size = 64;
            loop {
                let mut buf = alloc::vec![0u8; size];
                match v.write_lexical(&mut buf) {
                    Ok(s) => return Ok(s.into()),
                    Err(e)
                        if e.kind() == ErrorKind::BufferFull
                            && size < 1 << 26 =>
                    {
                        size *= 8
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

/// Checks `v` against `slot` and converts it (writer rule 6).
pub(crate) fn prepare(
    schema: Option<&Schema>,
    slot: Slot,
    v: &Value<'_>,
) -> Result<Prepared> {
    match slot {
        Slot::Str => Ok(Prepared::Str(lexical_string(v)?, None)),
        Slot::QName | Slot::TypeName => match v {
            Value::QName(n) => Ok(Prepared::QName(
                n.uri().into(),
                n.local_name().into(),
                n.prefix().map(Into::into),
            )),
            _ => Err(unencodable("xsi:type needs a QName value")),
        },
        Slot::Nil => match v {
            Value::Bool(b) | Value::BoolDigit(b) => Ok(Prepared::Bool(*b)),
            _ => lexical::parse_bool(&lexical_string(v)?)
                .map(|(b, _)| Prepared::Bool(b))
                .ok_or_else(|| unencodable("xsi:nil needs a Boolean")),
        },
        Slot::Typed(ty) => {
            let schema = schema
                .ok_or_else(|| internal("typed slot without a schema"))?;
            prepare_typed(schema, ty, v)
        }
        Slot::Lexical(ty) => {
            let schema = schema
                .ok_or_else(|| internal("typed slot without a schema"))?;
            Ok(Prepared::Str(lexical_string(v)?, lexical_charset(schema, ty)))
        }
        Slot::NilText => {
            let s = lexical_string(v)?;
            lexical::parse_bool(&s)
                .ok_or_else(|| unencodable("xsi:nil needs a Boolean"))?;
            Ok(Prepared::Str(s, Some(boolean_charset())))
        }
    }
}

fn prepare_typed(
    schema: &Schema,
    ty: SimpleTypeId,
    v: &Value<'_>,
) -> Result<Prepared> {
    let bad = |d| Err(unencodable(d));
    let text = || lexical_string(v);
    match schema.rep(ty) {
        Representation::String { charset } => {
            Ok(Prepared::Str(text()?, charset.clone()))
        }
        Representation::Boolean { pattern } => {
            let (b, digit) = match v {
                Value::Bool(b) => (*b, false),
                Value::BoolDigit(b) => (*b, true),
                _ => match lexical::parse_bool(&text()?) {
                    Some(x) => x,
                    None => return bad("not a Boolean"),
                },
            };
            Ok(if *pattern {
                Prepared::BoolPattern(b, digit)
            } else {
                Prepared::Bool(b)
            })
        }
        Representation::Integer(rep) => {
            let parsed = match v {
                Value::Int(i) => Ok(*i),
                _ => match lexical::parse_int(&text()?) {
                    Some(x) => x,
                    None => return bad("not an integer"),
                },
            };
            match (rep, parsed) {
                (IntegerRep::Signed, Ok(i)) => Ok(Prepared::Int(i)),
                (IntegerRep::Unsigned, Ok(i)) if i >= 0 => {
                    Ok(Prepared::UInt(i as u64))
                }
                (IntegerRep::Unsigned, Err((false, digits))) => {
                    Ok(Prepared::BigUInt(digits))
                }
                (IntegerRep::Bounded { min, max }, Ok(i))
                    if (*min..=*max).contains(&i) =>
                {
                    Ok(Prepared::Bounded(i, *min, *max))
                }
                (IntegerRep::Signed, Err(_)) => bad(
                    "integer outside i64 (not supported for signed types yet)",
                ),
                _ => bad("integer outside the type's range"),
            }
        }
        Representation::Decimal => match v {
            Value::Decimal(d) => Ok(Prepared::Decimal(*d)),
            Value::Int(i) => {
                Ok(Prepared::Decimal(crate::Decimal::from_scaled(*i, 0)))
            }
            _ => lexical::parse_decimal(&text()?)
                .map(Prepared::Decimal)
                .map_or_else(|| bad("not a decimal"), Ok),
        },
        Representation::Float => match v {
            Value::Float(f) => Ok(Prepared::Float(*f)),
            Value::Int(i) => {
                Ok(Prepared::Float(crate::Float { mantissa: *i, exponent: 0 }))
            }
            _ => lexical::parse_float(&text()?)
                .map(Prepared::Float)
                .map_or_else(|| bad("not a float"), Ok),
        },
        Representation::Binary(kind) => match v {
            Value::Binary(b, _) => Ok(Prepared::Binary(b.to_vec())),
            _ => {
                let s = text()?;
                let r = match kind {
                    BinaryKind::Hex => lexical::parse_hex(&s),
                    BinaryKind::Base64 => lexical::parse_base64(&s),
                };
                r.map(Prepared::Binary)
                    .map_or_else(|| bad("not binary in the type's form"), Ok)
            }
        },
        Representation::DateTime(kind) => match v {
            Value::DateTime(d) if d.kind == *kind => {
                Ok(Prepared::DateTime(*d))
            }
            Value::DateTime(_) => bad("a date-time of another kind"),
            _ => lexical::parse_datetime(&text()?, *kind)
                .map(Prepared::DateTime)
                .map_or_else(|| bad("not a date-time of the type's kind"), Ok),
        },
        Representation::List(item) => {
            let mut out = Vec::new();
            match v {
                Value::List(items) => {
                    for x in items.iter() {
                        out.push(prepare_typed(schema, *item, &x)?);
                    }
                }
                _ => {
                    let s = text()?;
                    for x in lexical::list_items(&s) {
                        out.push(prepare_typed(
                            schema,
                            *item,
                            &Value::Str(x),
                        )?);
                    }
                }
            }
            Ok(Prepared::List(out))
        }
        Representation::Enumeration { values, .. } => {
            let n = values.len() as u32;
            match v {
                Value::Enum { ordinal, .. } if *ordinal < n => {
                    Ok(Prepared::Enum(*ordinal, n))
                }
                _ => {
                    let s = text()?;
                    let s = s.trim_matches(|c| {
                        matches!(c, ' ' | '\t' | '\n' | '\r')
                    });
                    match values.iter().position(|x| &**x == s) {
                        Some(i) => Ok(Prepared::Enum(i as u32, n)),
                        None => bad("not one of the enumerated values"),
                    }
                }
            }
        }
    }
}

/// Writes a prepared value of `name` (its channel and value partition).
pub(crate) fn emit_value<E: Encoder>(
    e: &mut E,
    t: &mut SessionTables<'_>,
    name: NameId,
    v: &Prepared,
    prefixes: bool,
) -> Result<()> {
    let ch = Channel::Value(name);
    match v {
        Prepared::Str(s, None) => names::write_value(e, t, name, s),
        Prepared::Str(s, Some(set)) => match t.find_value_str(name, s) {
            crate::tables::ValueHit::Miss => {
                t.add_value(name, s)?;
                write_uint(e, ch, s.chars().count() as u64 + 2)?;
                write_restricted_chars(e, ch, s, set)
            }
            _ => names::write_value(e, t, name, s),
        },
        Prepared::QName(uri, local, prefix) => {
            let u = names::write_uri(e, t, uri)?;
            names::write_local(e, t, u, local)?;
            if prefixes {
                write_prefix(e, t, u, prefix.as_deref())?;
            }
            Ok(())
        }
        other => emit_typed(e, ch, other),
    }
}

fn emit_typed<E: Encoder>(e: &mut E, ch: Channel, v: &Prepared) -> Result<()> {
    match v {
        Prepared::Str(s, set) => {
            write_uint(e, ch, s.chars().count() as u64)?;
            match set {
                Some(set) => write_restricted_chars(e, ch, s, set),
                None => write_chars(e, ch, s),
            }
        }
        Prepared::QName(..) => Err(internal("QName inside a typed value")),
        Prepared::Bool(b) => write_bool(e, ch, *b),
        Prepared::BoolPattern(b, digit) => {
            write_bool_pattern(e, ch, *b, *digit)
        }
        Prepared::Int(i) => write_int(e, ch, *i),
        Prepared::UInt(u) => write_uint(e, ch, *u),
        Prepared::BigUInt(digits) => write_uint_digits(e, ch, digits),
        Prepared::Bounded(i, min, max) => write_bounded(e, ch, *i, *min, *max),
        Prepared::Decimal(d) => write_decimal(e, ch, d),
        Prepared::Float(f) => write_float(e, ch, *f),
        Prepared::Binary(b) => write_binary(e, ch, b),
        Prepared::DateTime(d) => write_datetime(e, ch, d),
        Prepared::List(items) => {
            write_uint(e, ch, items.len() as u64)?;
            for x in items {
                emit_typed(e, ch, x)?;
            }
            Ok(())
        }
        Prepared::Enum(i, n) => write_enum(e, ch, *i, *n),
    }
}

/// The prefix of a name with a known URI: n-bit, `width(m)` bits, when
/// the URI has `m > 0` prefixes; 0 bits otherwise. An unknown prefix is
/// sent as number 0.
pub(crate) fn write_prefix<E: Encoder>(
    e: &mut E,
    t: &SessionTables<'_>,
    uri: u32,
    prefix: Option<&str>,
) -> Result<()> {
    let m = t.prefix_count(uri);
    if m == 0 {
        return Ok(());
    }
    let n = prefix
        .and_then(|p| (0..m).find(|i| t.prefix_str(uri, *i) == Some(p)))
        .unwrap_or(0);
    e.nbit(Channel::Structure, n, width(m))
}

/// The owned name of numbers in the tables.
pub(crate) fn owned_name(
    t: &SessionTables<'_>,
    id: NameId,
    prefix: Option<u32>,
) -> OwnedName {
    let n = OwnedName::new(
        t.uri_str(id.uri).unwrap_or(""),
        t.local_str(id.uri, id.local).unwrap_or(""),
    );
    match prefix.and_then(|p| t.prefix_str(id.uri, p)) {
        Some(p) => n.with_prefix(p),
        None => n,
    }
}
