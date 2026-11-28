// src/tables.rs
//
// Stream string tables
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! This module is written so the tables can be implemented from it
//! alone. It states every rule EXI sets for string tables. The EXI
//! section each rule comes from is named in brackets for reviewers; the
//! implementation does not need it.
//!
//! # What a session's tables are
//!
//! An EXI encoder and decoder each keep string tables for one stream.
//! Both add the same strings in the same order, so a number means the
//! same string on both sides. Every rule below exists for that: if this
//! code adds, skips or removes one entry differently from
//! another implementation, every later number differs and the stream
//! decodes as wrong data.
//!
//! [`SessionTables`] are those tables. Each entry is the id of a string
//! in the [`GlobalStrings`], so a string is stored once however
//! many sessions use it. The numbers are exactly EXI's; sharing changes
//! nothing a stream writes.
//!
//! The tables do not read or write EXI. The reader and writer call them
//! in the order the stream needs.
//!
//! # Partitions (R1) [EXI §7.3.1]
//!
//! | Partition | How many | Holds |
//! |---|---|---|
//! | URI | one | namespace URIs |
//! | Prefix | one per URI | prefixes declared for that URI |
//! | Local name | one per URI | local names of elements and attributes in that namespace |
//! | Global value | one | every value kept, in one numbering |
//! | Local value | one per name ([`NameId`]) | the values of that name, in a second numbering |
//!
//! Every partition numbers its entries from 0. The local value
//! partition of a name holds the values of attributes with that name
//! and the character content of elements with that name: an element and
//! an attribute with the same URI and local name share one partition.
//! (To be confirmed against a fixture from a second implementation
//! before this module is marked done.)
//!
//! # Initial entries (R2) [EXI App. D]
//!
//! Every stream starts with these entries, in this order. Value
//! partitions start empty.
//!
//! | URI number | URI | Prefixes | Local names |
//! |---|---|---|---|
//! | 0 | `""` (no namespace) | `""` | none |
//! | 1 | `http://www.w3.org/XML/1998/namespace` | `xml` | `base`, `id`, `lang`, `space` |
//! | 2 | `http://www.w3.org/2001/XMLSchema-instance` | `xsi` | `nil`, `type` |
//! | 3, only when a schema is used | `http://www.w3.org/2001/XMLSchema` | none | the 46 built-in type names, sorted: `ENTITIES`, `ENTITY`, `ID`, `IDREF`, `IDREFS`, `NCName`, `NMTOKEN`, `NMTOKENS`, `NOTATION`, `Name`, `QName`, `anySimpleType`, `anyType`, `anyURI`, `base64Binary`, `boolean`, `byte`, `date`, `dateTime`, `decimal`, `double`, `duration`, `float`, `gDay`, `gMonth`, `gMonthDay`, `gYear`, `gYearMonth`, `hexBinary`, `int`, `integer`, `language`, `long`, `negativeInteger`, `nonNegativeInteger`, `nonPositiveInteger`, `normalizedString`, `positiveInteger`, `short`, `string`, `time`, `token`, `unsignedByte`, `unsignedInt`, `unsignedLong`, `unsignedShort` |
//!
//! With a schema, its target namespaces that are not already listed
//! follow URI 3, sorted. Each URI's local-name partition then holds its
//! entries above plus every name the schema declares in that namespace,
//! the whole partition sorted, duplicates removed. A schema adds
//! no prefixes. "Sorted" means by Unicode code point, which for UTF-8
//! is byte order: Rust's `str` ordering. So `"Z"` sorts before `"a"`,
//! and `"é"` after `"z"`.
//!
//! # Names (R3) [EXI §7.3.2]
//!
//! A URI, prefix or local name sent as a literal is added after it,
//! with the next number in its partition. Names are never removed
//! during a stream.
//!
//! # Values (R4, R5, R6, R11) [EXI §7.3.3, Profile §4]
//!
//! **R4: which values are added.** A value sent as a literal is added
//! only when all three hold:
//!
//! 1. `valuePartitionCapacity` is not 0;
//! 2. the value has at least one character, so the empty value is never
//!    added;
//! 3. the value has at most `valueMaxLength` characters, when that
//!    option is set.
//!
//! Characters are Unicode code points, what `str::chars` yields, not
//! bytes.
//!
//! **R5: where.** An added value goes into the global value partition
//! and into the local value partition of its name, each with that
//! partition's next number.
//!
//! **R6: a full global partition.** When `valuePartitionCapacity` is
//! set and the global partition holds that many values, the next value
//! added takes the slot of the oldest. A slot counter goes 0, 1, …,
//! capacity − 1, then back to 0, and the slot it points at is the
//! global number of the value added. The value that was in that slot is
//! removed from the global partition and from its local partition. Its
//! local number is permanently unassigned: the local partition keeps
//! counting from where it was, and the number is never given to another
//! value. Names are never removed this way; only values.
//!
//! **R11: no local partitions.** With the EXI Profile parameter
//! `localValuePartitions` set to 0, values go only into the global
//! partition, and only global numbers exist.
//!
//! # Self-contained elements (R7) [EXI §8.4.3]
//!
//! Inside a self-contained element the tables are saved, reset to the
//! initial entries (R2), and restored when the element ends.
//! Self-contained elements can nest.
//! [`SessionTables::begin_self_contained`] and
//! [`SessionTables::end_self_contained`] do this, keeping the saved
//! partitions on a stack inside the session.
//!
//! # One stream (R8) [EXI §7.3]
//!
//! Tables last for one stream. The next stream starts again from R2
//! with a new `SessionTables`.
//!
//! # Order of additions (R9) [EXI §9.3]
//!
//! The tables add entries in the order they are called. With
//! compression or pre-compression the stream holds values in channel
//! order, not document order, and the reader and writer must call the
//! tables in channel order.
//!
//! # Finding values (R10) [C14N §4.5.6]
//!
//! A value may be sent by its local number or its global number.
//! [`SessionTables::find_value`] returns the local number when there is
//! one, then the global number. Canonical EXI requires that order, and
//! other writers lose nothing by it.
//!
//! # Duplicates
//!
//! The reader adds what the stream says. An encoder may send a literal
//! for a string the table already holds, and then the table holds it
//! twice, under two numbers, as the encoder does. Lookups find the most
//! recent number. A libexi writer always looks first, so it never
//! adds a duplicate.
//!
//! # Writer policy
//!
//! libexi writers leave `valuePartitionCapacity` unset, so their value
//! partitions only grow. When a session reaches
//! `Limits::max_string_table_bytes` it returns `ErrorKind::Limit`, and
//! the application starts a new stream. The tables do not decide this:
//! they apply the [`ValueRules`] they are given. R6 exists for streams
//! from other encoders.
//!
//! # References
//!
//! Every entry a session adds holds one reference to its global string
//! (one per value addition, for its global and local entry together).
//! Initial entries are held by [`Initial`], once for all its sessions.
//! Dropping a `SessionTables`, or ending a self-contained element,
//! releases the references of what it drops; strings no one references
//! are freed.
//!
//! # Counts, not widths
//!
//! The number of bits a number takes on the wire depends on how many
//! entries its partition has. The tables report the counts
//! ([`SessionTables::uri_count`] and the others) and the codec computes
//! the widths. [`SessionTables::local_value_count`] counts every number
//! ever assigned in that partition, permanently unassigned ones
//! included.
//!
//! # Representation
//!
//! | Part | Representation |
//! |---|---|
//! | Initial entries | Held by [`Initial`]: id arrays per partition and their reverse maps, built once. A session refers to them and stores only what it adds, so creating a session copies nothing |
//! | URIs, prefixes, local names | Per partition: the initial part from `Initial`, then a `Vec` of added ids, and a reverse map (`hashbrown::HashMap`, `FxBuildHasher`) from id to its most recent number |
//! | Global values | `Vec<Slot>` with `Slot { id: ValueId, name: NameId }`, grown up to the capacity, then reused by the slot counter; a reverse map from id to its most recent slot |
//! | Local values | `HashMap<NameId, LocalPartition>`, with `LocalPartition { base: u32, live: VecDeque<ValueId>, reverse: HashMap<ValueId, u32> }`. Live numbers are `base .. base + live.len()`; numbers below `base` are permanently unassigned |
//! | Self-contained stack | `Vec` of saved partition sets, innermost last |
//! | Counts | names added, and bytes referenced (see Limits), over the current and saved partitions together |
//!
//! Why a queue is enough for local partitions: values enter the global
//! partition in time order and R6 removes them in the same order,
//! oldest first. Each local partition receives its values in time order
//! too. So the value R6 removes is always the oldest live value of
//! its own local partition: its front. Removing it is
//! `live.pop_front()` and `base += 1`. Memory is bounded by the live
//! values, not by every value ever added.
//!
//! On removal, drop a reverse-map entry only when it points at the
//! number or slot being removed; with duplicates it may point at a
//! newer one.
//!
//! # Limits
//!
//! | Limit | Counts | When exceeded |
//! |---|---|---|
//! | [`Limits::max_names`] | URIs, prefixes and local names this session added, all kinds together, saved partitions included. Initial entries do not count | `ErrorKind::Limit` |
//! | [`Limits::max_string_table_bytes`] | UTF-8 bytes of every name this session added, plus every value now in its global value partition, saved partitions included. Initial entries do not count. A removed value (R6) stops counting. The check is `bytes − removed + added ≤ limit`, so exactly the limit is allowed | `ErrorKind::Limit` |
//! | Numbers per partition | `0 ..= u32::MAX − 1` | `ErrorKind::Limit` |
//! | Global table limits | [`GlobalConfig`](crate::strings::GlobalConfig) | `ErrorKind::Limit` |
//!
//! On any error, nothing is changed, and the session stays usable.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use hashbrown::HashMap;
use rustc_hash::FxBuildHasher;

use crate::strings::{
    GlobalStrings, GlobalTable, Kind, LocalId, PrefixId, StrId, UriId, ValueId,
};
use crate::{Error, ErrorKind, Limits, NameId, Result};

type Map<K, V> = HashMap<K, V, FxBuildHasher>;

fn map<K, V>() -> Map<K, V> {
    HashMap::with_hasher(FxBuildHasher)
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";
const XSD_TYPES: [&str; 46] = [
    "ENTITIES",
    "ENTITY",
    "ID",
    "IDREF",
    "IDREFS",
    "NCName",
    "NMTOKEN",
    "NMTOKENS",
    "NOTATION",
    "Name",
    "QName",
    "anySimpleType",
    "anyType",
    "anyURI",
    "base64Binary",
    "boolean",
    "byte",
    "date",
    "dateTime",
    "decimal",
    "double",
    "duration",
    "float",
    "gDay",
    "gMonth",
    "gMonthDay",
    "gYear",
    "gYearMonth",
    "hexBinary",
    "int",
    "integer",
    "language",
    "long",
    "negativeInteger",
    "nonNegativeInteger",
    "nonPositiveInteger",
    "normalizedString",
    "positiveInteger",
    "short",
    "string",
    "time",
    "token",
    "unsignedByte",
    "unsignedInt",
    "unsignedLong",
    "unsignedShort",
];

/// The entries a stream starts with (R2), as strings. Attach them to a
/// [`GlobalStrings`] with [`attach`](InitialEntries::attach) to make sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitialEntries {
    // Per URI, in number order: the URI, its prefixes, its local names.
    uris: Vec<UriEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UriEntry {
    uri: Box<str>,
    prefixes: Vec<Box<str>>,
    locals: Vec<Box<str>>,
}

impl InitialEntries {
    /// Without a schema: URIs 0 to 2.
    pub fn schemaless() -> InitialEntries {
        InitialEntries {
            uris: alloc::vec![
                uri_entry("", &[""], &[]),
                uri_entry(XML_NS, &["xml"], &["base", "id", "lang", "space"]),
                uri_entry(XSI_NS, &["xsi"], &["nil", "type"]),
            ],
        }
    }

    /// With the XML Schema built-in types only: URIs 0 to 3.
    pub fn builtin_types() -> InitialEntries {
        let mut e = Self::schemaless();
        e.uris.push(uri_entry(XSD_NS, &[], &XSD_TYPES));
        e
    }

    /// Starts from [`builtin_types`](InitialEntries::builtin_types) and adds a schema's
    /// names.
    pub fn schema() -> InitialBuilder {
        InitialBuilder {
            base: Self::builtin_types(),
            uris: Vec::new(),
            locals: Vec::new(),
        }
    }

    pub fn uris(&self) -> alloc::vec::Vec<&str> {
        self.uris.iter().map(|u| &*u.uri).collect()
    }
    /// The local names of URI number `uri`; empty when out of range.
    pub fn locals(&self, uri: u32) -> alloc::vec::Vec<&str> {
        self.uris
            .get(uri as usize)
            .map_or_else(Vec::new, |u| u.locals.iter().map(|l| &**l).collect())
    }

    /// Index wire names without making another copy of their strings.
    #[cfg(feature = "std")]
    pub(crate) fn name_index(&self) -> hashbrown::HashTable<NameId> {
        use core::hash::BuildHasher;
        let mut index = hashbrown::HashTable::new();
        for (uri, entry) in self.uris.iter().enumerate() {
            for (local, name) in entry.locals.iter().enumerate() {
                let hash = FxBuildHasher.hash_one((&*entry.uri, &**name));
                index.insert_unique(
                    hash,
                    NameId { uri: uri as u32, local: local as u32 },
                    |&id| FxBuildHasher.hash_one(self.name_strings(id)),
                );
            }
        }
        index
    }

    #[cfg(feature = "std")]
    pub(crate) fn name_strings(&self, id: NameId) -> (&str, &str) {
        let entry = &self.uris[id.uri as usize];
        (&entry.uri, &entry.locals[id.local as usize])
    }

    pub(crate) fn name_id(&self, uri: &str, local: &str) -> Option<NameId> {
        let u = self.uris.iter().position(|e| &*e.uri == uri)?;
        let l = self.uris[u].locals.iter().position(|s| &**s == local)?;
        Some(NameId { uri: u as u32, local: l as u32 })
    }
    /// Interns every entry into `g` and builds the id arrays sessions
    /// share. The returned [`Initial`] holds one reference to each
    /// string until it is dropped.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when `g` cannot take the strings. Nothing is
    /// kept.
    pub fn attach<'g>(&self, g: &'g GlobalStrings) -> Result<Initial<'g>> {
        let mut init = Initial {
            global: g,
            uris: Vec::new(),
            uri_rev: map(),
            prefixes: Vec::new(),
            prefix_rev: Vec::new(),
            locals: Vec::new(),
            local_rev: Vec::new(),
        };
        // On error `init` is dropped, which releases every string
        // interned so far.
        for (n, u) in self.uris.iter().enumerate() {
            let id = g.uris().intern_id(&u.uri)?;
            init.uris.push(id);
            init.uri_rev.insert(id, n as u32);
            init.prefixes.push(Vec::with_capacity(u.prefixes.len()));
            init.prefix_rev.push(Map::with_capacity_and_hasher(u.prefixes.len(), FxBuildHasher));
            init.locals.push(Vec::with_capacity(u.locals.len()));
            init.local_rev.push(Map::with_capacity_and_hasher(u.locals.len(), FxBuildHasher));
            for (i, p) in u.prefixes.iter().enumerate() {
                let id = g.prefixes().intern_id(p)?;
                init.prefixes[n].push(id);
                init.prefix_rev[n].insert(id, i as u32);
            }
            for (i, l) in u.locals.iter().enumerate() {
                let id = g.locals().intern_id(l)?;
                init.locals[n].push(id);
                init.local_rev[n].insert(id, i as u32);
            }
        }
        Ok(init)
    }
}

/// Builds the initial entries of a schema (R2). Names given more than
/// once, or already present, are kept once.
#[derive(Clone, Debug)]
pub struct InitialBuilder {
    base: InitialEntries,
    uris: Vec<Box<str>>,
    locals: Vec<(Box<str>, Box<str>)>,
}

impl InitialBuilder {
    /// Adds a target namespace.
    pub fn uri(self, uri: &str) -> Self {
        let mut me = self;
        me.uris.push(uri.into());
        me
    }
    /// Adds a declared name, and its namespace if new.
    pub fn local(self, uri: &str, local: &str) -> Self {
        let mut me = self;
        me.uris.push(uri.into());
        me.locals.push((uri.into(), local.into()));
        me
    }
    /// Sorts and numbers the entries as R2 says.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when a partition would have more than
    /// `u32::MAX − 1` entries.
    pub fn build(self) -> Result<InitialEntries> {
        let mut e = self.base;
        let mut extra: Vec<Box<str>> = self
            .uris
            .into_iter()
            .filter(|u| !e.uris.iter().any(|x| &x.uri == u))
            .collect();
        extra.sort();
        extra.dedup();
        for u in extra {
            e.uris.push(UriEntry {
                uri: u,
                prefixes: Vec::new(),
                locals: Vec::new(),
            });
        }
        for (u, l) in self.locals {
            let entry = e
                .uris
                .iter_mut()
                .find(|x| x.uri == u)
                .expect("every URI was added above");
            entry.locals.push(l);
        }
        for entry in &mut e.uris {
            entry.locals.sort();
            entry.locals.dedup();
            if entry.locals.len() as u64 >= u32::MAX as u64 - 1 {
                return Err(limit("partition has u32::MAX - 1 entries"));
            }
        }
        if e.uris.len() as u64 >= u32::MAX as u64 - 1 {
            return Err(limit("partition has u32::MAX - 1 entries"));
        }
        Ok(e)
    }
}

/// Initial entries attached to one [`GlobalStrings`]: what every
/// session of that grammar set starts from. Built once, shared by any
/// number of sessions.
pub struct Initial<'g> {
    global: &'g GlobalStrings,
    uris: Vec<UriId>,
    uri_rev: Map<UriId, u32>,
    prefixes: Vec<Vec<PrefixId>>,
    prefix_rev: Vec<Map<PrefixId, u32>>,
    locals: Vec<Vec<LocalId>>,
    local_rev: Vec<Map<LocalId, u32>>,
}

impl<'g> Initial<'g> {
    pub fn global(&self) -> &'g GlobalStrings {
        self.global
    }
}

impl core::fmt::Debug for Initial<'_> {
    /// Shows the partition counts.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Initial")
            .field("uris", &self.uris.len())
            .field("locals", &self.locals.iter().map(Vec::len).sum::<usize>())
            .finish()
    }
}

impl Drop for Initial<'_> {
    fn drop(&mut self) {
        let g = self.global;
        for id in self.uris.drain(..) {
            g.uris().release(id);
        }
        for v in self.prefixes.drain(..) {
            for id in v {
                g.prefixes().release(id);
            }
        }
        for v in self.locals.drain(..) {
            for id in v {
                g.locals().release(id);
            }
        }
    }
}

/// The stream options that govern value partitions (R4, R6, R11).
///
/// [`unbounded`](ValueRules::unbounded) is EXI's default: no capacity, no maximum length,
/// local partitions on. A later `from_options(&Options)` fills these
/// from a stream's options, once `Options` exists.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ValueRules {
    capacity: Option<u32>,
    max_length: Option<u32>,
    local_partitions: bool,
}

impl ValueRules {
    pub const fn unbounded() -> Self {
        ValueRules { capacity: None, max_length: None, local_partitions: true }
    }
    /// `valuePartitionCapacity`: `None` for no limit.
    pub const fn capacity(self, n: Option<u32>) -> Self {
        ValueRules { capacity: n, ..self }
    }
    /// `valueMaxLength`, in characters: `None` for no limit.
    pub const fn max_length(self, n: Option<u32>) -> Self {
        ValueRules { max_length: n, ..self }
    }
    pub const fn get_local_partitions(&self) -> bool {
        self.local_partitions
    }
    pub const fn get_capacity(&self) -> Option<u32> {
        self.capacity
    }

    /// R4: is a literal of `chars` characters added to the tables?
    pub const fn keeps(&self, chars: u32) -> bool {
        let room = !matches!(self.capacity, Some(0));
        let short = match self.max_length {
            Some(m) => chars <= m,
            None => true,
        };
        room && chars >= 1 && short
    }
}

/// Where a value was found (R10).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ValueHit {
    /// Its number in the name's local value partition.
    Local(u32),
    /// Its number in the global value partition.
    Global(u32),
    /// Not in this session's tables: send it as a literal.
    Miss,
}

/// EXI's string tables for one stream (R1), as arrays of global ids.
///
/// Setup used by the examples:
///
/// Dropping a session releases its references.
pub struct SessionTables<'a> {
    initial: &'a Initial<'a>,
    rules: ValueRules,
    max_names: u32,
    max_bytes: u64,
    cur: Partitions,
    stack: Vec<Partitions>,
    names_total: u32,
    bytes_total: u64,
    /// Writers look up values by `ValueId` (`find_value`); readers only index
    /// forward by partition number and can skip maintaining `global_rev` /
    /// `lp.rev`.
    track_value_rev: bool,
}

impl<'a> SessionTables<'a> {
    /// Tables holding only the initial entries.
    pub fn new(
        initial: &'a Initial<'a>,
        rules: ValueRules,
        limits: &Limits,
    ) -> Self {
        SessionTables {
            initial,
            rules,
            max_names: limits.get_max_names(),
            max_bytes: limits.get_max_string_table_bytes(),
            cur: Partitions::new(),
            stack: Vec::new(),
            names_total: 0,
            bytes_total: 0,
            track_value_rev: true,
        }
    }

    /// Tables for a reader: forward value lookup only, without reverse value
    /// hash maps.
    pub(crate) fn new_for_reader(
        initial: &'a Initial<'a>,
        rules: ValueRules,
        limits: &Limits,
    ) -> Self {
        let mut t = Self::new(initial, rules, limits);
        t.track_value_rev = false;
        t
    }
    pub fn rules(&self) -> ValueRules {
        self.rules
    }

    // ----- URIs, prefixes, local names (R2, R3) -----

    fn active(&self, outer: bool) -> &Partitions {
        if outer { self.stack.last().unwrap_or(&self.cur) } else { &self.cur }
    }

    pub fn uri_count(&self) -> u32 {
        (self.initial_uris() + self.cur.uris.added.len()) as u32
    }
    pub(crate) fn uri_at(&self, outer: bool, n: u32) -> Option<UriId> {
        lookup(&self.initial.uris, Some(&self.active(outer).uris), n)
    }
    pub(crate) fn uri_str_at(&self, outer: bool, n: u32) -> Option<&str> {
        self.uri_at(outer, n).map(|id| self.global().uris().resolve(id))
    }
    pub fn uri_str(&self, n: u32) -> Option<&str> {
        self.uri_str_at(false, n)
    }
    /// The most recent number of the URI with global id `id`.
    pub fn find_uri(&self, id: UriId) -> Option<u32> {
        find(Some(&self.initial.uri_rev), Some(&self.cur.uris), id)
    }
    pub fn find_uri_str(&self, s: &str) -> Option<u32> {
        if s.is_empty() && !self.initial.uris.is_empty() {
            if self.cur.uris.added.is_empty() {
                return Some(0);
            }
            let empty_id = self.initial.uris[0];
            return Some(
                self.cur.uris.rev.get(&empty_id).copied().unwrap_or(0),
            );
        }
        self.global().uris().get(s).and_then(|id| self.find_uri(id))
    }

    /// Adds a URI sent as a literal, with the next number, even when
    /// the URI is already present (see "Duplicates").
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` for [`Limits::max_names`],
    /// [`Limits::max_string_table_bytes`], a full partition or a full
    /// global table. Nothing is changed.
    ///
    /// [`Limits::max_names`] counts the names this session added;
    /// initial entries are free.
    pub fn add_uri(&mut self, s: &str) -> Result<u32> {
        self.check_name(s.len(), self.uri_count() as usize)?;
        let id = self.global().uris().intern_id(s)?;
        let n = self.uri_count();
        self.cur.uris.added.push(id);
        self.cur.uris.rev.insert(id, n);
        self.count_name(s.len());
        Ok(n)
    }

    /// Prefixes in the partition of URI number `uri`; 0 when out of
    /// range.
    pub fn prefix_count(&self, uri: u32) -> u32 {
        let init = self.initial.prefixes.get(uri as usize).map_or(0, Vec::len);
        let added = self
            .cur
            .uri_part(uri)
            .map_or(0, |p| p.prefixes.added.len());
        (init + added) as u32
    }
    pub(crate) fn prefix_at(
        &self,
        outer: bool,
        uri: u32,
        n: u32,
    ) -> Option<PrefixId> {
        let part = self.active(outer).uri_part(uri).map(|p| &p.prefixes);
        lookup(
            self.initial.prefixes.get(uri as usize).map_or(&[][..], |v| v),
            part,
            n,
        )
    }
    #[cfg(test)]
    fn prefix(&self, uri: u32, n: u32) -> Option<PrefixId> {
        self.prefix_at(false, uri, n)
    }
    pub(crate) fn prefix_str_at(
        &self,
        outer: bool,
        uri: u32,
        n: u32,
    ) -> Option<&str> {
        self.prefix_at(outer, uri, n)
            .map(|id| self.global().prefixes().resolve(id))
    }
    pub fn prefix_str(&self, uri: u32, n: u32) -> Option<&str> {
        self.prefix_str_at(false, uri, n)
    }
    pub fn find_prefix(&self, uri: u32, id: PrefixId) -> Option<u32> {
        find(
            self.initial.prefix_rev.get(uri as usize),
            self.cur.uri_part(uri).map(|p| &p.prefixes),
            id,
        )
    }
    /// Adds a prefix to the partition of URI number `uri`, as [`add_uri`](Self::add_uri).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when `uri` is not a number this session has;
    /// otherwise as `add_uri`.
    pub fn add_prefix(&mut self, uri: u32, s: &str) -> Result<u32> {
        self.check_uri_number(uri)?;
        self.check_name(s.len(), self.prefix_count(uri) as usize)?;
        let id = self.global().prefixes().intern_id(s)?;
        let n = self.prefix_count(uri);
        let part = &mut self.cur.uri_part_mut(uri).prefixes;
        part.added.push(id);
        part.rev.insert(id, n);
        self.count_name(s.len());
        Ok(n)
    }

    /// Local names in the partition of URI number `uri`; 0 when out of
    /// range.
    pub fn local_count(&self, uri: u32) -> u32 {
        let init = self.initial.locals.get(uri as usize).map_or(0, Vec::len);
        let added = self
            .cur
            .uri_part(uri)
            .map_or(0, |p| p.locals.added.len());
        (init + added) as u32
    }
    pub(crate) fn local_at(
        &self,
        outer: bool,
        uri: u32,
        n: u32,
    ) -> Option<LocalId> {
        let part = self.active(outer).uri_part(uri).map(|p| &p.locals);
        lookup(
            self.initial.locals.get(uri as usize).map_or(&[][..], |v| v),
            part,
            n,
        )
    }
    #[cfg(any(feature = "xml", test))]
    pub fn local(&self, uri: u32, n: u32) -> Option<LocalId> {
        self.local_at(false, uri, n)
    }
    pub(crate) fn local_str_at(
        &self,
        outer: bool,
        uri: u32,
        n: u32,
    ) -> Option<&str> {
        self.local_at(outer, uri, n)
            .map(|id| self.global().locals().resolve(id))
    }
    pub fn local_str(&self, uri: u32, n: u32) -> Option<&str> {
        self.local_str_at(false, uri, n)
    }
    #[cfg(feature = "xml")]
    pub(crate) fn local_str_by_id(&self, id: LocalId) -> &str {
        self.global().locals().resolve(id)
    }
    pub(crate) fn resolve_name_at(
        &self,
        outer: bool,
        id: NameId,
    ) -> (Option<UriId>, &str, Option<LocalId>, &str) {
        let g = self.global();
        let uid = self.uri_at(outer, id.uri);
        let u = uid.map_or("", |id| g.uris().resolve(id));
        let lid = self.local_at(outer, id.uri, id.local);
        let l = lid.map_or("", |id| g.locals().resolve(id));
        (uid, u, lid, l)
    }
    #[cfg(feature = "xml")]
    pub(crate) fn resolve_name(
        &self,
        id: NameId,
    ) -> (Option<UriId>, &str, Option<LocalId>, &str) {
        self.resolve_name_at(false, id)
    }
    pub(crate) fn global_ref(&self) -> &GlobalStrings {
        self.initial.global
    }
    pub fn find_local(&self, uri: u32, id: LocalId) -> Option<u32> {
        find(
            self.initial.local_rev.get(uri as usize),
            self.cur.uri_part(uri).map(|p| &p.locals),
            id,
        )
    }
    /// Adds a local name to the partition of URI number `uri`, as [`add_uri`](Self::add_uri).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when `uri` is not a number this session has;
    /// otherwise as `add_uri`.
    pub fn add_local(&mut self, uri: u32, s: &str) -> Result<u32> {
        self.check_uri_number(uri)?;
        self.check_name(s.len(), self.local_count(uri) as usize)?;
        let id = self.global().locals().intern_id(s)?;
        let n = self.local_count(uri);
        let part = &mut self.cur.uri_part_mut(uri).locals;
        part.added.push(id);
        part.rev.insert(id, n);
        self.count_name(s.len());
        Ok(n)
    }

    /// The numbers of a name, if this session has its URI and local
    /// name.
    pub fn find_name(&self, uri: &str, local: &str) -> Option<NameId> {
        let u = self.find_uri_str(uri)?;
        let l = self
            .global()
            .locals()
            .get(local)
            .and_then(|id| self.find_local(u, id))?;
        Some(NameId { uri: u, local: l })
    }
    /// The numbers of a name, adding its URI and local name when
    /// absent: what a writer does. A reader adds with `add_uri` and
    /// `add_local`, as the stream says.
    #[cfg(test)]
    pub fn add_name(&mut self, uri: &str, local: &str) -> Result<NameId> {
        let u = match self.find_uri_str(uri) {
            Some(u) => u,
            None => self.add_uri(uri)?,
        };
        let l = match self
            .global()
            .locals()
            .get(local)
            .and_then(|id| self.find_local(u, id))
        {
            Some(l) => l,
            None => self.add_local(u, local)?,
        };
        Ok(NameId { uri: u, local: l })
    }

    // ----- Values (R4, R5, R6, R10, R11) -----

    /// Values now in the global value partition: at most the capacity.
    pub fn global_value_count(&self) -> u32 {
        self.cur.slots.len() as u32
    }
    /// Numbers ever assigned in the local value partition of `name`,
    /// permanently unassigned ones included (R6). 0 without local
    /// partitions (R11).
    pub fn local_value_count(&self, name: NameId) -> u32 {
        self.cur
            .local
            .get(&name)
            .map_or(0, |lp| lp.base + lp.live.len() as u32)
    }
    pub fn global_value(&self, n: u32) -> Option<ValueId> {
        self.cur.slots.get(n as usize).map(|s| s.id)
    }
    /// `None` for a number never assigned or permanently unassigned.
    pub fn local_value(&self, name: NameId, n: u32) -> Option<ValueId> {
        let lp = self.cur.local.get(&name)?;
        if n < lp.base {
            return None;
        }
        lp.live.get((n - lp.base) as usize).copied()
    }
    /// The value's local number if it has one, else its global number
    /// (R10).
    pub fn find_value(&self, name: NameId, id: ValueId) -> ValueHit {
        if self.rules.local_partitions {
            if let Some(&n) =
                self.cur.local.get(&name).and_then(|lp| lp.rev.get(&id))
            {
                return ValueHit::Local(n);
            }
        }
        match self.cur.global_rev.get(&id) {
            Some(&slot) => ValueHit::Global(slot),
            None => ValueHit::Miss,
        }
    }
    pub fn find_value_str(&self, name: NameId, s: &str) -> ValueHit {
        match self.global().values().get(s) {
            Some(id) => self.find_value(name, id),
            None => ValueHit::Miss,
        }
    }

    /// Adds a value sent as a literal for `name`. Returns its global
    /// id, or `None` when R4 does not keep it, in which case nothing
    /// changes. When the global partition is full, the oldest value is
    /// removed first (R6).
    ///
    /// # Errors
    ///
    /// - `ErrorKind::Arg` when `name` is not a name this session has.
    /// - `ErrorKind::Limit` for [`Limits::max_string_table_bytes`], a
    ///   full partition or a full global table.
    ///
    /// Nothing is changed on error.
    ///
    /// R4 and R5: the empty value is skipped; a kept value has both
    /// numbers.
    ///
    /// `valueMaxLength` counts characters, not bytes.
    ///
    /// Capacity 0 keeps nothing; without local partitions only global
    /// numbers exist (R11).
    ///
    /// Duplicates: lookups find the most recent; removing the older
    /// copy leaves the newer.
    ///
    /// The byte limit: names and values count, exactly the limit is
    /// allowed, and a removed value makes room (R6).
    pub fn add_value(
        &mut self,
        name: NameId,
        s: &str,
    ) -> Result<Option<ValueId>> {
        self.check_name_id(name)?;
        let chars = if s.is_empty() {
            0
        } else if let Some(max_len) = self.rules.max_length {
            if s.len() <= max_len as usize {
                s.len() as u32
            } else {
                s.chars().count().min(u32::MAX as usize) as u32
            }
        } else {
            1
        };
        if !self.rules.keeps(chars) {
            return Ok(None);
        }
        self.check_value(s.len())?;
        let id = self.global().values().intern_id(s)?;
        self.place_value(name, id, s.len() as u32);
        Ok(Some(id))
    }


    // ----- Self-contained elements (R7) -----

    /// Enters a self-contained element: moves the current partitions
    /// onto an internal stack and starts again from the initial
    /// entries, with empty value partitions and the slot counter at 0
    /// (R7). The limits keep counting what was moved aside.
    pub fn begin_self_contained(&mut self) {
        let fresh = Partitions::new();
        let saved = core::mem::replace(&mut self.cur, fresh);
        self.stack.push(saved);
    }

    /// Leaves the innermost self-contained element: drops the
    /// partitions used inside it, releasing their references, and puts
    /// back the ones moved aside by the matching [`begin_self_contained`](Self::begin_self_contained).
    ///
    /// # Errors
    ///
    /// `ErrorKind::State` when no self-contained element is open.
    /// Nothing is changed.
    pub fn end_self_contained(&mut self) -> Result<()> {
        let saved = self.stack.pop().ok_or_else(|| {
            Error::new(ErrorKind::State, "no self-contained element is open")
        })?;
        let mut inner = core::mem::replace(&mut self.cur, saved);
        self.names_total -= inner.names;
        self.bytes_total -= inner.bytes;
        inner.release(self.initial.global);
        Ok(())
    }

    /// Self-contained elements now open.
    pub fn depth(&self) -> u32 {
        self.stack.len() as u32
    }

    // ----- Limits -----

    /// Names this session added, partitions moved aside included.
    #[cfg(test)]
    fn names_added(&self) -> u32 {
        self.names_total
    }
    /// Bytes this session references, as
    /// [`Limits::max_string_table_bytes`] counts them.
    #[cfg(test)]
    fn referenced_bytes(&self) -> u64 {
        self.bytes_total
    }
}

impl core::fmt::Debug for SessionTables<'_> {
    /// Shows the partition counts, the depth and the limit counters,
    /// not the strings.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SessionTables")
            .field("uris", &self.uri_count())
            .field("values", &self.global_value_count())
            .field("depth", &self.depth())
            .field("names_added", &self.names_total)
            .field("bytes", &self.bytes_total)
            .finish()
    }
}

impl Drop for SessionTables<'_> {
    fn drop(&mut self) {
        let g = self.initial.global;
        self.cur.release(g);
        for p in &mut self.stack {
            p.release(g);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------------------

fn uri_entry(uri: &str, prefixes: &[&str], locals: &[&str]) -> UriEntry {
    UriEntry {
        uri: uri.into(),
        prefixes: prefixes.iter().map(|&p| p.into()).collect(),
        locals: locals.iter().map(|&l| l.into()).collect(),
    }
}

fn limit(detail: &'static str) -> Error {
    Error::new(ErrorKind::Limit, detail)
}

/// One growing name partition: the numbers after the initial entries.
struct NamePart<K: Kind> {
    added: Vec<StrId<K>>,
    rev: Map<StrId<K>, u32>,
}

impl<K: Kind> NamePart<K> {
    fn new() -> Self {
        NamePart { added: Vec::new(), rev: map() }
    }
    fn release(&mut self, table: &GlobalTable<K>) {
        if !self.added.is_empty() {
            table.release_all(self.added.drain(..));
        }
        self.rev.clear();
    }
}

struct UriPart {
    prefixes: NamePart<crate::strings::Prefix>,
    locals: NamePart<crate::strings::Local>,
}

impl UriPart {
    fn new() -> Self {
        UriPart { prefixes: NamePart::new(), locals: NamePart::new() }
    }
}

#[derive(Copy, Clone)]
struct Slot {
    id: ValueId,
    name: NameId,
    len: u32,
}

struct LocalPart {
    base: u32,
    live: VecDeque<ValueId>,
    rev: Map<ValueId, u32>,
}

/// The partitions of one stream, or of one self-contained element.
struct Partitions {
    uris: NamePart<crate::strings::Uri>,
    /// Indexed by URI number; only populated for URIs with a
    /// session-added prefix or local name.
    per_uri: Vec<Option<Box<UriPart>>>,
    slots: Vec<Slot>,
    next_slot: u32,
    global_rev: Map<ValueId, u32>,
    local: Map<NameId, LocalPart>,
    names: u32,
    bytes: u64,
}

impl Partitions {
    fn new() -> Self {
        Partitions {
            uris: NamePart::new(),
            per_uri: Vec::new(),
            slots: Vec::new(),
            next_slot: 0,
            global_rev: map(),
            local: map(),
            names: 0,
            bytes: 0,
        }
    }

    #[inline]
    fn uri_part(&self, uri: u32) -> Option<&UriPart> {
        self.per_uri.get(uri as usize).and_then(|p| p.as_deref())
    }

    fn uri_part_mut(&mut self, uri: u32) -> &mut UriPart {
        let idx = uri as usize;
        if idx >= self.per_uri.len() {
            self.per_uri.resize_with(idx + 1, || None);
        }
        self.per_uri[idx].get_or_insert_with(|| Box::new(UriPart::new()))
    }

    fn release(&mut self, g: &GlobalStrings) {
        self.uris.release(g.uris());
        for p in self.per_uri.iter_mut().flatten() {
            p.prefixes.release(g.prefixes());
            p.locals.release(g.locals());
        }
        if !self.slots.is_empty() {
            g.values().release_all(self.slots.drain(..).map(|s| s.id));
        }
        self.global_rev.clear();
        self.local.clear();
    }
}

fn lookup<K: Kind>(
    initial: &[StrId<K>],
    part: Option<&NamePart<K>>,
    n: u32,
) -> Option<StrId<K>> {
    let n = n as usize;
    if n < initial.len() {
        Some(initial[n])
    } else {
        part?.added.get(n - initial.len()).copied()
    }
}

fn find<K: Kind>(
    initial: Option<&Map<StrId<K>, u32>>,
    part: Option<&NamePart<K>>,
    id: StrId<K>,
) -> Option<u32> {
    part.and_then(|p| p.rev.get(&id))
        .copied()
        .or_else(|| initial.and_then(|m| m.get(&id).copied()))
}

impl SessionTables<'_> {
    /// The text of a value this session's tables hold.
    pub(crate) fn global_value_text(&self, id: ValueId) -> &str {
        self.global().values().resolve(id)
    }

    /// The global id of a local name, if any session holds it.
    pub(crate) fn global_strings_local(&self, s: &str) -> Option<LocalId> {
        self.global().locals().get(s)
    }

    /// The global id of a prefix, if any session holds it.
    pub(crate) fn global_strings_prefix(&self, s: &str) -> Option<PrefixId> {
        self.global().prefixes().get(s)
    }

    fn global(&self) -> &GlobalStrings {
        self.initial.global
    }

    fn initial_uris(&self) -> usize {
        self.initial.uris.len()
    }

    /// Checks the name limits for one more name of `len` bytes.
    fn check_name(&self, len: usize, count: usize) -> Result<()> {
        if self.names_total >= self.max_names {
            return Err(limit("max_names"));
        }
        if self.bytes_total + len as u64 > self.max_bytes {
            return Err(limit("max_string_table_bytes"));
        }
        if count as u64 >= u32::MAX as u64 - 1 {
            return Err(limit("partition has u32::MAX - 1 entries"));
        }
        Ok(())
    }

    fn count_name(&mut self, len: usize) {
        self.names_total += 1;
        self.bytes_total += len as u64;
        self.cur.names += 1;
        self.cur.bytes += len as u64;
    }

    fn check_uri_number(&self, uri: u32) -> Result<()> {
        if uri >= self.uri_count() {
            return Err(Error::new(
                ErrorKind::Arg,
                "no URI with this number in the session",
            ));
        }
        Ok(())
    }

    fn check_name_id(&self, name: NameId) -> Result<()> {
        if name.uri >= self.uri_count()
            || name.local >= self.local_count(name.uri)
        {
            return Err(Error::new(
                ErrorKind::Arg,
                "name not in the session's tables",
            ));
        }
        Ok(())
    }

    /// Adds `id` (whose reference the session now owns) as a value of
    /// `name` with `len` bytes.
    fn place_value(&mut self, name: NameId, id: ValueId, len: u32) {
        let cap = self.rules.capacity;
        let slot = self.cur.next_slot;
        if (slot as usize) < self.cur.slots.len() {
            // Full: remove the oldest value (R6).
            let victim = self.cur.slots[slot as usize];
            if self.track_value_rev
                && self.cur.global_rev.get(&victim.id) == Some(&slot)
            {
                self.cur.global_rev.remove(&victim.id);
            }
            if let Some(lp) = self.cur.local.get_mut(&victim.name) {
                let front = lp
                    .live
                    .pop_front()
                    .expect("the evicted value is its partition's oldest");
                debug_assert_eq!(front, victim.id);
                if self.track_value_rev
                    && lp.rev.get(&victim.id) == Some(&lp.base)
                {
                    lp.rev.remove(&victim.id);
                }
                lp.base += 1;
            }
            self.global().values().release(victim.id);
            self.bytes_total -= victim.len as u64;
            self.cur.bytes -= victim.len as u64;
            self.cur.slots[slot as usize] = Slot { id, name, len };
        } else {
            self.cur.slots.push(Slot { id, name, len });
        }
        self.cur.next_slot = slot + 1;
        if cap == Some(self.cur.next_slot) {
            self.cur.next_slot = 0;
        }
        if self.track_value_rev {
            self.cur.global_rev.insert(id, slot);
        }
        if self.rules.local_partitions {
            let lp = self.cur.local.entry(name).or_insert_with(|| LocalPart {
                base: 0,
                live: VecDeque::new(),
                rev: map(),
            });
            let n = lp.base + lp.live.len() as u32;
            lp.live.push_back(id);
            if self.track_value_rev {
                lp.rev.insert(id, n);
            }
        }
        self.bytes_total += len as u64;
        self.cur.bytes += len as u64;
    }

    /// Checks the value limits for adding `len` bytes; the bytes of the
    /// value R6 would remove are taken off first.
    fn check_value(&self, len: usize) -> Result<()> {
        let full = (self.cur.next_slot as usize) < self.cur.slots.len();
        let removed = if full {
            self.cur.slots[self.cur.next_slot as usize].len as u64
        } else {
            0
        };
        if self.bytes_total - removed + len as u64 > self.max_bytes {
            return Err(limit("max_string_table_bytes"));
        }
        if !full && self.cur.slots.len() as u64 >= u32::MAX as u64 - 1 {
            return Err(limit(
                "global value partition has u32::MAX - 1 entries",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::GlobalConfig;

    #[test]
    fn uri_partitions_are_created_only_when_extended() -> Result<()> {
        let global = GlobalStrings::new(GlobalConfig::device())?;
        let entries = InitialEntries::schemaless();
        let initial = entries.attach(&global)?;
        let mut tables = SessionTables::new(
            &initial,
            ValueRules::unbounded(),
            &Limits::device(),
        );
        let xml = tables.prefix(1, 0).unwrap();
        let lang = tables.local(1, 2).unwrap();
        assert_eq!(tables.find_prefix(1, xml), Some(0));
        assert_eq!(tables.find_local(1, lang), Some(2));

        let uri = tables.add_uri(XML_NS)?;
        assert!(tables.cur.per_uri.is_empty());
        assert_eq!(tables.prefix_count(uri), 0);
        assert_eq!(tables.local_count(uri), 0);
        assert_eq!(tables.find_local(uri, lang), None);
        assert_eq!(tables.add_local(uri, "lang")?, 0);
        assert_eq!(tables.add_local(1, "lang")?, 4);
        assert_eq!(tables.add_prefix(1, "xml")?, 1);
        assert_eq!(tables.find_local(1, lang), Some(4));
        assert_eq!(tables.find_prefix(1, xml), Some(1));
        assert_eq!(tables.local(1, 2), Some(lang));
        assert_eq!(tables.prefix(1, 0), Some(xml));

        tables.begin_self_contained();
        assert!(tables.cur.per_uri.is_empty());
        assert_eq!(tables.find_local(1, lang), Some(2));
        assert_eq!(tables.find_prefix(1, xml), Some(0));
        assert_eq!(tables.add_local(1, "inner")?, 4);
        tables.end_self_contained()?;
        assert_eq!(global.locals().get("inner"), None);
        assert_eq!(tables.find_local(1, lang), Some(4));
        assert_eq!(tables.find_local(uri, lang), Some(0));
        assert_eq!(tables.find_prefix(1, xml), Some(1));
        Ok(())
    }

    #[test]
    fn max_length_counts_characters() -> Result<()> {
        let g = GlobalStrings::new(GlobalConfig::new())?;
        let init = InitialEntries::schemaless().attach(&g)?;
        let rules = ValueRules::unbounded().max_length(Some(3));
        let mut t = SessionTables::new(&init, rules, &Limits::new());
        let n = t.add_name("", "n")?;
        assert!(t.add_value(n, "ééé")?.is_some()); // 3 characters, 6 bytes
        assert!(t.add_value(n, "😀😀😀")?.is_some()); // 3 characters, 12 bytes
        assert!(t.add_value(n, "abcd")?.is_none()); // 4 characters
        Ok(())
    }

    #[test]
    fn value_admission_limits() {
        let r = ValueRules::unbounded();
        assert!(!r.keeps(0)); // the empty value: never
        assert!(r.keeps(1));
        assert!(r.keeps(u32::MAX));
        let r = ValueRules::unbounded().max_length(Some(8));
        assert!(r.keeps(8)); // exactly the maximum
        assert!(!r.keeps(9));
        assert!(!ValueRules::unbounded().max_length(Some(0)).keeps(1)); // nothing kept
        assert!(!ValueRules::unbounded().capacity(Some(0)).keeps(5)); // nothing kept
        assert!(ValueRules::unbounded().capacity(Some(1)).keeps(5));
    }

    #[test]
    fn eviction_wraparound() -> Result<()> {
        let g = GlobalStrings::new(GlobalConfig::new())?;
        let init = InitialEntries::schemaless().attach(&g)?;
        let mut t = SessionTables::new(
            &init,
            ValueRules::unbounded().capacity(Some(3)),
            &Limits::new(),
        );
        let n = t.add_name("", "n")?;
        for i in 0..10 {
            t.add_value(n, &alloc::format!("v{i}"))?;
        }
        assert_eq!(t.global_value_count(), 3);
        let slots: Vec<_> = (0..3)
            .map(|i| g.values().resolve(t.global_value(i).unwrap()))
            .collect();
        assert_eq!(slots, ["v9", "v7", "v8"]);
        assert_eq!(t.local_value_count(n), 10);
        assert_eq!(t.local_value(n, 6).map(|id| g.values().resolve(id)), None); // removed
        assert_eq!(
            t.local_value(n, 7).map(|id| g.values().resolve(id)),
            Some("v7")
        );
        assert_eq!(
            t.local_value(n, 9).map(|id| g.values().resolve(id)),
            Some("v9")
        );
        assert_eq!(
            t.local_value(n, 10).map(|id| g.values().resolve(id)),
            None
        ); // not yet assigned
        assert_eq!(t.find_value_str(n, "v8"), ValueHit::Local(8));
        Ok(())
    }

    #[test]
    fn saved_partitions_count_toward_limits() -> Result<()> {
        let g = GlobalStrings::new(GlobalConfig::new())?;
        let init = InitialEntries::schemaless().attach(&g)?;
        let mut t = SessionTables::new(
            &init,
            ValueRules::unbounded(),
            &Limits::new().max_names(1),
        );
        t.add_uri("urn:a")?;
        t.begin_self_contained();
        assert_eq!(t.add_uri("urn:b").unwrap_err().kind(), ErrorKind::Limit);
        Ok(())
    }

    #[test]
    fn self_contained_values_are_released() -> Result<()> {
        let g = GlobalStrings::new(GlobalConfig::new())?;
        let init = InitialEntries::schemaless().attach(&g)?;
        let mut t =
            SessionTables::new(&init, ValueRules::unbounded(), &Limits::new());
        assert_eq!(
            t.end_self_contained().unwrap_err().kind(),
            ErrorKind::State
        );
        t.begin_self_contained();
        let n = t.add_name("", "inner")?;
        t.add_value(n, "inner value")?;
        t.end_self_contained()?;
        assert_eq!(g.values().get("inner value"), None); // released
        assert_eq!((t.names_added(), t.referenced_bytes()), (0, 0));
        Ok(())
    }
}
