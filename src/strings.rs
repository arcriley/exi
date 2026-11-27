// src/strings.rs
//
// Global string tables
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # What this module is for
//!
//! An EXI stream gives each string a small number so a repeated string
//! is sent as that number. Each stream keeps its own numbering.
//! A server may hold thousands of streams at once,
//! and most of their strings are the same: the same namespaces, element
//! names and addresses. This module stores each distinct string once
//! for all of them, and gives it an integer id. Two sessions that hold
//! the same id hold the same string, so a server compares strings from
//! different connections as integers.
//!
//! The global tables only share storage. They never change what any
//! stream writes: a stream encoded with shared tables is byte for byte
//! the same as one encoded with private tables.
//!
//! # Four tables, one per kind
//!
//! [`GlobalStrings`] holds four [`GlobalTable`]s: URIs, prefixes, local
//! names and values. Strings of different kinds are never compared with
//! each other, so they never share a table. The same text, such as
//! `"id"`, can be in two tables with unrelated ids. Each kind
//! has its own id type ([`UriId`], [`PrefixId`], [`LocalId`],
//! [`ValueId`]), so the compiler rejects an id used with the wrong
//! table.
//!
//! # References
//!
//! Every string counts its references. A string with at least one
//! reference keeps its id and its bytes. When the count reaches zero
//! the string is freed: its id and its bytes may be reused by a later
//! string. References are held by:
//!
//! - a [`Held`] handle, returned by [`GlobalTable::intern`] and
//!   [`PendingStr::finish`]. It releases its reference when dropped.
//!   Code that keeps an id for later, such as a router keeping a JID,
//!   keeps a `Held`;
//! - session tables, for every entry they hold (crate-internal
//!   `retain`/`release`);
//! - The attached grammar, for every initial entry.
//!
//! An id obtained with [`GlobalTable::get`] carries no reference. It
//! names the string only while something else holds one. Kept beyond
//! that, it may name a different string later.
//!
//! # Soundness rule for the implementation
//!
//! Tables are shared by `&` reference and add strings through interior
//! mutability (`UnsafeCell`). This is sound because of one rule: **the
//! bytes of a string with at least one reference are never written.**
//! Every `&str` this module hands out is borrowed from something that
//! holds a reference (a [`Held`], or a session table), so no `&str` can
//! point at bytes being reused. Consequences for the code:
//!
//! - chunk bytes are written only through raw pointers into free or
//!   reserved space; never form a `&mut [u8]` over a chunk that other
//!   strings live in;
//! - chunks are separate heap allocations that never move or shrink;
//!   the `Vec` of chunk pointers may reallocate, which does not move
//!   the chunks;
//! - no method keeps a reference into `entries`, the index or the free
//!   lists across a call that can modify them. No method calls back
//!   into user code;
//! - `GlobalTable` and `GlobalStrings` are `Send` but not `Sync`: one
//!   thread at a time. A shared form for many threads is an open
//!   question.
//!
//! Run the tests of this module under Miri.
//!
//! # Representation
//!
//! | Part | Representation |
//! |---|---|
//! | Arena | A `Vec` of chunks, each a `Box<[u8]>` of `chunk_bytes`. Each string is stored contiguously, followed by one NUL byte that its length does not count, so C callers can use it directly. A string longer than `chunk_bytes − 1` bytes gets a chunk of its own, exactly `len + 1` bytes. A string that does not fit in the rest of the current chunk starts a new chunk; the unused tail is counted in [`TableStats::free_bytes`] |
//! | Entry, per id | `{ chunk: u32, start: u32, len: u32, cap: u32, hash: u32, refs: u32 }`. `cap` is the slot size in bytes (at least `len + 1`). `refs == 0` marks a free entry |
//! | Index | A Swiss table (`hashbrown::HashMap`, hasher `rustc_hash::FxBuildHasher`) from the key of a string's hash to every id with that key. The key is `hash::key16` of `hash::hash32` with `hash_bits(16)`, or the full `hash32` with `hash_bits(32)`. The value is one id inline (the usual case) or a short list when keys collide |
//! | Free lists | Freed entries by size class: class `k` holds slots with `cap` in `[2^k, 2^(k+1))`. A new string of `n + 1` bytes takes, in this order: a freed slot from the smallest non-empty class `k` with `2^k ≥ n + 1`, with its id; a freed id whose slot went with its chunk, with fresh bytes; a new id with fresh bytes |
//! | Chunk release | Each chunk counts its live strings. When the count reaches 0, the chunk's slots leave the free lists and the chunk is returned to the allocator; if it is the chunk being appended to, its write position goes back to 0 instead. So memory follows the live strings without a compaction step, which could never run anyway: compacting needs `&mut`, and every holder of a reference borrows the tables |
//!
//! Lookup: hash the string, find its key, and for each id listed
//! compare the length, then the bytes. Insertion: store the bytes and
//! NUL, fill an entry, add the id under its key. Freeing: remove the id
//! from its key's list, move the entry to its free list.
//!
//! # Limits
//!
//! | Limit | Value | When exceeded |
//! |---|---|---|
//! | Ids per table | `0 ..= u32::MAX − 1` | `ErrorKind::Limit` |
//! | Live strings per table | [`GlobalConfig::max_strings`], default `u32::MAX` | `ErrorKind::Limit`; interning a string already present still succeeds |
//! | Live bytes per table: the sum of `len + 1` over live strings | [`GlobalConfig::max_bytes`], default `u64::MAX` | `ErrorKind::Limit` |
//! | One string | at most `u32::MAX − 1` bytes, and within `max_bytes` | `ErrorKind::Limit` |
//! | References per string | `u32::MAX − 1` | Panic, as `Arc` aborts on overflow: it means a leak, not input |
//! | Chunk size | a power of two, `64 ..= 2^30` | [`GlobalStrings::new`] returns `ErrorKind::Arg` |
//! | Key width | 16 or 32 bits | [`GlobalStrings::new`] returns `ErrorKind::Arg` |
//!
//! When a limit is exceeded, nothing is changed.

use core::cell::{Cell, UnsafeCell};
use core::ptr::NonNull;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::marker::PhantomData;
use hashbrown::HashMap;
use rustc_hash::FxBuildHasher;

use crate::{Error, ErrorKind, Result};

pub(crate) mod hash;

mod sealed {
    pub trait Sealed {}
}

/// The kind of string a table holds.
/// The four kinds below are all there are.
pub trait Kind:
    sealed::Sealed
    + Copy
    + Eq
    + Ord
    + core::hash::Hash
    + core::fmt::Debug
    + 'static
{
}

/// Namespace URIs. Holds `""`, which is "no namespace".
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Uri {}
/// Namespace prefixes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Prefix {}
/// Local names of elements and attributes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Local {}
/// Attribute values and character content.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {}

impl sealed::Sealed for Uri {}
impl sealed::Sealed for Prefix {}
impl sealed::Sealed for Local {}
impl sealed::Sealed for Value {}
impl Kind for Uri {}
impl Kind for Prefix {}
impl Kind for Local {}
impl Kind for Value {}

/// The id of one string in a global table of kind `K`.
///
/// Ids are dense integers from 0, so callers can index arrays with
/// them. An id is the same string for as long as the string has a
/// reference; after that it may be reused. Ids from different
/// [`GlobalStrings`] are unrelated, and the type does not tell them
/// apart: mixing them is a logic error the tables cannot detect.
///
/// ```
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// let g = GlobalStrings::new(GlobalConfig::new())?;
/// let first = g.values().intern("first")?;
/// assert_eq!(first.id().index(), 0);
/// assert_eq!(g.values().intern("second")?.id().index(), 1);
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StrId<K: Kind>(u32, PhantomData<K>);

impl<K: Kind> StrId<K> {
    /// The id as an integer.
    pub const fn index(self) -> u32 {
        self.0
    }
}

pub type UriId = StrId<Uri>;
pub type PrefixId = StrId<Prefix>;
pub type LocalId = StrId<Local>;
pub type ValueId = StrId<Value>;

/// Configuration of a global table. Every table of a [`GlobalStrings`]
/// uses the same one.
///
/// | Setting | `new()` | `device()` |
/// |---|---|---|
/// | `hash_bits` | 16 | 16 |
/// | `chunk_bytes` | 16 384 | 1 024 |
/// | `max_bytes` | `u64::MAX` (no limit) | 65 536 |
/// | `max_strings` | `u32::MAX` (no limit) | 4 096 |
///
/// The `device()` values are placeholders until the benchmark suite
/// tunes them. Settings are checked by [`GlobalStrings::new`], not
/// here.
///
/// ```
/// use exi::strings::GlobalConfig;
/// let c = GlobalConfig::new().hash_bits(32).chunk_bytes(4096).max_bytes(1 << 20).max_strings(100);
/// assert_eq!((c.get_hash_bits(), c.get_chunk_bytes()), (32, 4096));
/// assert_eq!((c.get_max_bytes(), c.get_max_strings()), (1 << 20, 100));
/// let d = GlobalConfig::device();
/// assert_eq!((d.get_chunk_bytes(), d.get_max_bytes(), d.get_max_strings()), (1024, 65_536, 4096));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GlobalConfig {
    hash_bits: u8,
    chunk_bytes: u32,
    max_bytes: u64,
    max_strings: u32,
}

impl GlobalConfig {
    /// Defaults for hosts and servers.
    pub const fn new() -> Self {
        GlobalConfig {
            hash_bits: 16,
            chunk_bytes: 16_384,
            max_bytes: u64::MAX,
            max_strings: u32::MAX,
        }
    }
    /// Small bounds for microcontrollers.
    pub const fn device() -> Self {
        GlobalConfig {
            hash_bits: 16,
            chunk_bytes: 1_024,
            max_bytes: 65_536,
            max_strings: 4_096,
        }
    }
    /// Width of the index key: 16 (default) or 32. Beyond about 30 000
    /// strings in one table, 16-bit keys give collision lists of more
    /// than one id on average; 32-bit keys keep them short for millions
    /// of strings, at the cost of a larger index.
    pub const fn hash_bits(self, bits: u8) -> Self {
        GlobalConfig { hash_bits: bits, ..self }
    }
    /// Size of each arena chunk in bytes: a power of two from 64 to
    /// 2^30.
    pub const fn chunk_bytes(self, n: u32) -> Self {
        GlobalConfig { chunk_bytes: n, ..self }
    }
    /// Most live bytes in one table, counting `len + 1` per string (the
    /// NUL included).
    pub const fn max_bytes(self, n: u64) -> Self {
        GlobalConfig { max_bytes: n, ..self }
    }
    /// Most live strings in one table.
    pub const fn max_strings(self, n: u32) -> Self {
        GlobalConfig { max_strings: n, ..self }
    }
    pub const fn get_hash_bits(&self) -> u8 {
        self.hash_bits
    }
    pub const fn get_chunk_bytes(&self) -> u32 {
        self.chunk_bytes
    }
    pub const fn get_max_bytes(&self) -> u64 {
        self.max_bytes
    }
    pub const fn get_max_strings(&self) -> u32 {
        self.max_strings
    }
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Counts that describe a table, for tests, benchmarks and monitoring.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TableStats {
    /// Live strings: those with at least one reference.
    pub strings: u32,
    /// Live bytes: the sum of `len + 1` over live strings. This is what
    /// `max_bytes` bounds.
    pub bytes: u64,
    /// Arena bytes allocated but not holding a live string: freed
    /// slots, unused chunk tails and abandoned reservations. Released
    /// with the chunk when its last string is freed.
    pub free_bytes: u64,
    /// Arena chunks allocated, oversized ones included.
    pub chunks: u32,
    /// The longest list of ids sharing one index key. 1 when no keys
    /// collide; 0 when empty.
    pub longest_list: u32,
}

/// One global table of strings of kind `K` (see the module
/// documentation).
///
/// `Debug` shows [`TableStats`], not the strings.
pub struct GlobalTable<K: Kind> {
    inner: UnsafeCell<Inner>,
    cfg: GlobalConfig,
    _kind: PhantomData<K>,
    _not_sync: PhantomData<Cell<()>>,
}

// SAFETY: every byte and pointer the table holds is owned by it;
// nothing is shared with another table or thread. `Sync` stays off (the
// Cell marker): one thread at a time.
unsafe impl<K: Kind> Send for GlobalTable<K> {}

impl<K: Kind> core::fmt::Debug for GlobalTable<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GlobalTable").field("stats", &self.stats()).finish()
    }
}

impl<K: Kind> GlobalTable<K> {
    /// The id of `s`, if the table holds it. Adds no reference (see
    /// "References").
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// assert_eq!(g.values().get("absent"), None);
    /// let h = g.values().intern("present")?;
    /// assert_eq!(g.values().get("present"), Some(h.id()));
    /// assert_eq!(g.values().refs(h.id()), 1);  // get added nothing
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn get(&self, s: &str) -> Option<StrId<K>> {
        let cfg = self.cfg;
        let h = hash::hash32(s.as_bytes());
        self.inner_ref()
            .find(&cfg, s.as_bytes(), h)
            .map(|i| StrId(i, PhantomData))
    }

    /// The string `s`, added if absent, with one more reference held by
    /// the returned handle. The empty string is a string like any
    /// other: it is stored as a single NUL.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when adding `s` would exceed `max_strings` or
    /// `max_bytes`, or no id is left. A string already present never
    /// fails: it only gains a reference.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let a = g.values().intern("juliet@example.com")?;
    /// let b = g.values().intern("juliet@example.com")?;
    /// assert_eq!(a.id(), b.id());                // stored once
    /// assert_eq!(g.values().refs(a.id()), 2);
    /// assert_eq!(a.as_str(), "juliet@example.com");
    /// let empty = g.uris().intern("")?;          // "no namespace" is a real entry
    /// assert_eq!(empty.as_str(), "");
    /// assert_eq!(g.uris().stats().bytes, 1);     // the NUL only
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// Each kind is its own table:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let _id = g.locals().intern("id")?;
    /// assert_eq!(g.values().get("id"), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// A value id cannot be used with the URI table:
    ///
    /// ```compile_fail
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new()).unwrap();
    /// let v = g.values().intern("x").unwrap();
    /// g.uris().refs(v.id());
    /// ```
    ///
    /// The limit on live strings: at the limit a new string fails and
    /// an existing one succeeds; freeing one makes room again.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::ErrorKind;
    /// let g = GlobalStrings::new(GlobalConfig::new().max_strings(2))?;
    /// let a = g.values().intern("a")?;
    /// let _b = g.values().intern("b")?;
    /// assert_eq!(g.values().intern("c").unwrap_err().kind(), ErrorKind::Limit);
    /// assert_eq!(g.values().stats().strings, 2);  // nothing changed
    /// let _a2 = g.values().intern("a")?;          // already present: succeeds
    /// drop(a);
    /// drop(_a2);                                  // "a" freed
    /// let _c = g.values().intern("c")?;
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// The limit on live bytes counts `len + 1` per string and allows
    /// exactly the limit:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::ErrorKind;
    /// let g = GlobalStrings::new(GlobalConfig::new().max_bytes(10))?;
    /// let _a = g.values().intern("abcd")?;        // 5 bytes
    /// let _b = g.values().intern("efgh")?;        // 10 bytes: exactly the limit
    /// assert_eq!(g.values().intern("").unwrap_err().kind(), ErrorKind::Limit); // 11
    /// assert_eq!(g.values().stats().bytes, 10);
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// A string that fills a chunk exactly (with its NUL), and one
    /// larger than a chunk, which gets a chunk of its own:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new().chunk_bytes(64))?;
    /// let exact = "x".repeat(63);
    /// let big = "y".repeat(1000);
    /// let a = g.values().intern(&exact)?;
    /// let b = g.values().intern(&big)?;
    /// let c = g.values().intern("after")?;
    /// assert_eq!((a.as_str(), b.as_str(), c.as_str()), (exact.as_str(), big.as_str(), "after"));
    /// assert_eq!(g.values().stats().chunks, 3);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn intern(&self, s: &str) -> Result<Held<'_, K>> {
        self.intern_id(s).map(|id| Held { table: self, id })
    }

    /// Starts a string whose bytes arrive in pieces, such as a literal
    /// decoded across input refills. Reserves `max_len + 1` contiguous
    /// bytes at once, so several pending strings, from different
    /// sessions on one thread, can be built at the same time.
    ///
    /// `max_len` is an upper bound in bytes. A decoder that knows a
    /// literal has `n` characters passes `4 × n`, the most UTF-8 bytes
    /// `n` characters can take.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when `max_len + 1` more live bytes would
    /// exceed `max_bytes`, or `max_len` exceeds `u32::MAX − 1`. The
    /// reservation is counted as live bytes until
    /// [`PendingStr::finish`] or drop.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut p = g.values().begin(32)?;
    /// let mut q = g.values().begin(32)?;            // two at once
    /// p.push_str("juliet@")?;
    /// q.push_str("romeo@")?;
    /// p.push_str("example.com")?;
    /// q.push_char('x')?;
    /// assert_eq!(p.finish()?.as_str(), "juliet@example.com");
    /// assert_eq!(q.finish()?.as_str(), "romeo@x");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn begin(&self, max_len: u32) -> Result<PendingStr<'_, K>> {
        let cfg = self.cfg;
        let inner = self.inner();
        if max_len >= u32::MAX - 1 {
            return Err(Error::new(
                ErrorKind::Limit,
                "reservation longer than u32::MAX - 2 bytes",
            ));
        }
        let need = max_len + 1;
        if inner.live_bytes + need as u64 > cfg.max_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "global table: max_bytes",
            ));
        }
        let (chunk, start) = inner.alloc(&cfg, need);
        inner.chunks[chunk as usize].as_mut().expect("chunk").live += 1;
        inner.live_bytes += need as u64;
        Ok(PendingStr {
            table: self,
            chunk,
            start,
            cap: need,
            len: 0,
            done: false,
        })
    }

    /// The number of references `id` has: 0 for a freed id or one never
    /// made.
    pub fn refs(&self, id: StrId<K>) -> u32 {
        self.inner_ref().entries.get(id.0 as usize).map_or(0, |e| e.refs)
    }

    /// Counts describing this table.
    ///
    /// When the last string in a chunk is freed, the chunk is released:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new().chunk_bytes(64))?;
    /// let first = g.values().intern(&"a".repeat(63))?;      // fills chunk 1
    /// let second = g.values().intern("b")?;                 // chunk 2
    /// assert_eq!(g.values().stats().chunks, 2);
    /// drop(first);
    /// assert_eq!(g.values().stats().chunks, 1);             // chunk 1 released
    /// assert_eq!(g.values().stats().free_bytes, 62);        // chunk 2's unused tail
    /// drop(second);
    /// let s = g.values().stats();                           // the current chunk is kept, reset
    /// assert_eq!((s.strings, s.bytes, s.free_bytes, s.chunks), (0, 0, 64, 1));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn stats(&self) -> TableStats {
        self.inner_ref().stats()
    }
}

/// A reference to one string, released when dropped. Cloning adds
/// a reference.
///
/// The string's bytes cannot change while a `Held` exists, so
/// [`Held::as_str`] is valid for as long as the handle.
///
/// ```
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// let g = GlobalStrings::new(GlobalConfig::new())?;
/// let a = g.values().intern("temporary")?;
/// let a2 = a.clone();
/// assert_eq!(g.values().refs(a.id()), 2);
/// let id = a.id();
/// drop(a);
/// drop(a2);
/// assert_eq!(g.values().refs(id), 0);
/// assert_eq!(g.values().get("temporary"), None);  // freed
/// let b = g.values().intern("another")?;          // freed ids are reused first
/// assert_eq!(b.id().index(), id.index());
/// # Ok::<(), exi::Error>(())
/// ```
///
/// A `&str` cannot outlive its handle:
///
/// ```compile_fail
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// let g = GlobalStrings::new(GlobalConfig::new()).unwrap();
/// let h = g.values().intern("x").unwrap();
/// let s: &str = h.as_str();
/// drop(h);
/// println!("{s}");
/// ```
pub struct Held<'g, K: Kind> {
    table: &'g GlobalTable<K>,
    id: StrId<K>,
}

impl<'g, K: Kind> Held<'g, K> {
    pub fn id(&self) -> StrId<K> {
        self.id
    }
    pub fn as_str(&self) -> &str {
        self.table.resolve(self.id)
    }
    /// The table this handle belongs to, so a session can check that a
    /// handle comes from its own global tables.
    pub fn table(&self) -> &'g GlobalTable<K> {
        self.table
    }
}

impl<K: Kind> core::fmt::Debug for Held<'_, K> {
    /// Shows the id and the string.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Held")
            .field("id", &self.id.0)
            .field("str", &self.as_str())
            .finish()
    }
}

impl<K: Kind> Clone for Held<'_, K> {
    fn clone(&self) -> Self {
        self.table.retain(self.id);
        Held { table: self.table, id: self.id }
    }
}

impl<K: Kind> Drop for Held<'_, K> {
    fn drop(&mut self) {
        self.table.release(self.id);
    }
}

/// A string being built in pieces (see [`GlobalTable::begin`]). Dropped
/// without [`finish`](PendingStr::finish), it leaves the table as it was: the reservation is given
/// back if it is the last allocation in its chunk, and is otherwise
/// counted in [`TableStats::free_bytes`].
///
/// ```
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// let g = GlobalStrings::new(GlobalConfig::new())?;
/// let before = g.values().stats();
/// let mut p = g.values().begin(16)?;
/// p.push_str("abandoned")?;
/// # let _ = &p;
/// drop(p);
/// let after = g.values().stats();
/// assert_eq!((after.strings, after.bytes), (before.strings, before.bytes));
/// assert_eq!(g.values().get("abandoned"), None);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct PendingStr<'t, K: Kind> {
    table: &'t GlobalTable<K>,
    chunk: u32,
    start: u32,
    cap: u32,
    len: u32,
    done: bool,
}

impl<'t, K: Kind> PendingStr<'t, K> {
    /// Appends `s`.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when the string would exceed the `max_len`
    /// given to `begin`. Nothing is appended.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::ErrorKind;
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut p = g.values().begin(4)?;
    /// p.push_str("abcd")?;                                   // exactly max_len
    /// assert_eq!(p.push_str("e").unwrap_err().kind(), ErrorKind::Arg);
    /// assert_eq!(p.len(), 4);
    /// let mut q = g.values().begin(3)?;
    /// assert_eq!(q.push_char('😀').unwrap_err().kind(), ErrorKind::Arg); // 4 bytes > 3
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn push_str(&mut self, s: &str) -> Result<()> {
        if self.len as u64 + s.len() as u64 > self.cap as u64 - 1 {
            return Err(Error::new(
                ErrorKind::Arg,
                "pending string longer than its reservation",
            ));
        }
        self.table.inner().write_bytes(
            self.chunk,
            self.start + self.len,
            s.as_bytes(),
        );
        self.len += s.len() as u32;
        Ok(())
    }
    /// Appends one character.
    pub fn push_char(&mut self, c: char) -> Result<()> {
        let mut buf = [0u8; 4];
        self.push_str(c.encode_utf8(&mut buf))
    }
    /// Bytes appended so far.
    pub fn len(&self) -> u32 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Ends the string. If the table already holds an equal string,
    /// that string gains a reference and this copy is abandoned;
    /// otherwise this copy is kept. Either way the unused part of the
    /// reservation is given back as in the type's documentation.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when keeping the string would exceed
    /// `max_strings`, or no id is left. The copy is abandoned.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let a = g.values().intern("same")?;
    /// let mut p = g.values().begin(16)?;
    /// p.push_str("same")?;
    /// let b = p.finish()?;
    /// assert_eq!(a.id(), b.id());
    /// assert_eq!(g.values().refs(a.id()), 2);
    /// assert_eq!(g.values().stats().bytes, 5);   // one copy kept
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn finish(self) -> Result<Held<'t, K>> {
        let mut me = self;
        let table = me.table;
        let cfg = table.cfg;
        let inner = table.inner();
        let (chunk, start, len, cap) = (me.chunk, me.start, me.len, me.cap);
        // SAFETY: these bytes were written by push_str from &str
        // pieces, so they are UTF-8.
        let bytes = {
            let c = inner.chunks[chunk as usize].as_ref().expect("chunk");
            unsafe {
                core::slice::from_raw_parts(
                    c.ptr.as_ptr().add(start as usize),
                    len as usize,
                )
            }
        };
        let h = hash::hash32(bytes);
        me.done = true;
        if let Some(id) = inner.find(&cfg, bytes, h) {
            inner.retain(id);
            give_back(inner, chunk, start, cap, 0);
            return Ok(Held { table, id: StrId(id, PhantomData) });
        }
        // Keep this copy: the reservation becomes the string's slot.
        inner.live_bytes -= cap as u64;
        inner.chunks[chunk as usize].as_mut().expect("chunk").live -= 1;
        let id =
            inner.check_new(&cfg, len as usize).and_then(|()| inner.new_id());
        let id = match id {
            Ok(id) => id,
            Err(e) => {
                inner.live_bytes += cap as u64;
                inner.chunks[chunk as usize].as_mut().expect("chunk").live +=
                    1;
                give_back(inner, chunk, start, cap, 0);
                return Err(e);
            }
        };
        // Shrink the slot when it is the last allocation of the current
        // chunk.
        let slot_cap =
            if chunk == inner.current && inner.write_pos == start + cap {
                inner.write_pos = start + len + 1;
                len + 1
            } else {
                cap
            };
        inner.commit(&cfg, id, chunk, start, slot_cap, len, h);
        Ok(Held { table, id: StrId(id, PhantomData) })
    }
}

impl<K: Kind> core::fmt::Debug for PendingStr<'_, K> {
    /// Shows the bytes appended so far and the reservation.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PendingStr")
            .field("len", &self.len)
            .field("reserved", &self.cap)
            .finish()
    }
}

impl<K: Kind> Drop for PendingStr<'_, K> {
    fn drop(&mut self) {
        if !self.done {
            give_back(self.table.inner(), self.chunk, self.start, self.cap, 0);
        }
    }
}

/// The four global tables. Sessions borrow one; sessions that
/// should not share memory or timing with each other, such as different
/// tenants, use separate ones.
///
/// `Send`, not `Sync`: one thread at a time.
///
/// ```compile_fail
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// let g = GlobalStrings::new(GlobalConfig::new()).unwrap();
/// std::thread::scope(|s| { s.spawn(|| g.values().get("x")); });
/// ```
pub struct GlobalStrings {
    uris: GlobalTable<Uri>,
    prefixes: GlobalTable<Prefix>,
    locals: GlobalTable<Local>,
    values: GlobalTable<Value>,
}

impl core::fmt::Debug for GlobalStrings {
    /// Shows the four tables' [`TableStats`].
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GlobalStrings")
            .field("uris", &self.uris.stats())
            .field("prefixes", &self.prefixes.stats())
            .field("locals", &self.locals.stats())
            .field("values", &self.values.stats())
            .finish()
    }
}

impl GlobalStrings {
    /// Four empty tables, each with `cfg`.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when `hash_bits` is not 16 or 32, or
    /// `chunk_bytes` is not a power of two from 64 to 2^30.
    ///
    /// ```
    /// use exi::strings::{GlobalStrings, GlobalConfig};
    /// use exi::ErrorKind;
    /// assert!(GlobalStrings::new(GlobalConfig::new().chunk_bytes(64)).is_ok());
    /// assert!(GlobalStrings::new(GlobalConfig::new().chunk_bytes(1 << 30)).is_ok());
    /// assert!(GlobalStrings::new(GlobalConfig::new().hash_bits(32)).is_ok());
    /// let bad = |c| GlobalStrings::new(c).err().map(|e: exi::Error| e.kind());
    /// assert_eq!(bad(GlobalConfig::new().chunk_bytes(32)), Some(ErrorKind::Arg));
    /// assert_eq!(bad(GlobalConfig::new().chunk_bytes(100)), Some(ErrorKind::Arg));
    /// assert_eq!(bad(GlobalConfig::new().chunk_bytes(1 << 31)), Some(ErrorKind::Arg));
    /// assert_eq!(bad(GlobalConfig::new().hash_bits(8)), Some(ErrorKind::Arg));
    /// ```
    pub fn new(cfg: GlobalConfig) -> Result<Self> {
        check_config(&cfg)?;
        Ok(GlobalStrings {
            uris: GlobalTable::with_config(cfg),
            prefixes: GlobalTable::with_config(cfg),
            locals: GlobalTable::with_config(cfg),
            values: GlobalTable::with_config(cfg),
        })
    }
    pub fn uris(&self) -> &GlobalTable<Uri> {
        &self.uris
    }
    pub fn prefixes(&self) -> &GlobalTable<Prefix> {
        &self.prefixes
    }
    pub fn locals(&self) -> &GlobalTable<Local> {
        &self.locals
    }
    pub fn values(&self) -> &GlobalTable<Value> {
        &self.values
    }
}

// ---------------------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------------------

const NO_CHUNK: u32 = u32::MAX;

struct Chunk {
    ptr: NonNull<u8>,
    cap: u32,
    /// Live strings and pending reservations in this chunk.
    live: u32,
}

impl Chunk {
    fn new(cap: u32) -> Chunk {
        let b: Box<[u8]> = alloc::vec![0u8; cap as usize].into_boxed_slice();
        // SAFETY: Box::into_raw never returns null.
        let ptr =
            unsafe { NonNull::new_unchecked(Box::into_raw(b) as *mut u8) };
        Chunk { ptr, cap, live: 0 }
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `Box::<[u8]>::into_raw` with length
        // `cap`, and is freed once.
        unsafe {
            drop(Box::from_raw(core::ptr::slice_from_raw_parts_mut(
                self.ptr.as_ptr(),
                self.cap as usize,
            )))
        }
    }
}

#[derive(Copy, Clone)]
struct Entry {
    chunk: u32,
    start: u32,
    len: u32,
    cap: u32,
    hash: u32,
    refs: u32,
}

enum Ids {
    One(u32),
    Many(Vec<u32>),
}

impl Ids {
    fn as_slice(&self) -> &[u32] {
        match self {
            Ids::One(i) => core::slice::from_ref(i),
            Ids::Many(v) => v,
        }
    }
}

struct Inner {
    chunks: Vec<Option<Chunk>>,
    current: u32,
    write_pos: u32,
    entries: Vec<Entry>,
    index: HashMap<u32, Ids, FxBuildHasher>,
    free_slots: [Vec<u32>; 32],
    slotless: Vec<u32>,
    live_strings: u32,
    live_bytes: u64,
}

fn class_of(cap: u32) -> usize {
    31 - cap.leading_zeros() as usize
}

fn class_needed(n: u32) -> usize {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() as usize }
}

impl Inner {
    fn new() -> Inner {
        Inner {
            chunks: Vec::new(),
            current: NO_CHUNK,
            write_pos: 0,
            entries: Vec::new(),
            index: HashMap::with_hasher(FxBuildHasher),
            free_slots: Default::default(),
            slotless: Vec::new(),
            live_strings: 0,
            live_bytes: 0,
        }
    }

    fn key(cfg: &GlobalConfig, h: u32) -> u32 {
        if cfg.hash_bits == 16 { hash::key16(h) as u32 } else { h }
    }

    fn bytes(&self, e: &Entry) -> &[u8] {
        let c = self.chunks[e.chunk as usize]
            .as_ref()
            .expect("live entry has a chunk");
        // SAFETY: [start, start + len) lies inside the chunk and holds
        // this entry's bytes, which are never written while the entry
        // has a reference.
        unsafe {
            core::slice::from_raw_parts(
                c.ptr.as_ptr().add(e.start as usize),
                e.len as usize,
            )
        }
    }

    fn find(&self, cfg: &GlobalConfig, s: &[u8], h: u32) -> Option<u32> {
        let ids = self.index.get(&Self::key(cfg, h))?;
        ids.as_slice().iter().copied().find(|&id| {
            let e = &self.entries[id as usize];
            e.refs > 0 && e.len as usize == s.len() && self.bytes(e) == s
        })
    }

    fn put_chunk(&mut self, c: Chunk) -> u32 {
        if let Some(i) = self.chunks.iter().position(Option::is_none) {
            self.chunks[i] = Some(c);
            i as u32
        } else {
            self.chunks.push(Some(c));
            (self.chunks.len() - 1) as u32
        }
    }

    /// Fresh contiguous space of `need` bytes: (chunk, start).
    fn alloc(&mut self, cfg: &GlobalConfig, need: u32) -> (u32, u32) {
        if need > cfg.chunk_bytes {
            let c = self.put_chunk(Chunk::new(need));
            return (c, 0);
        }
        let fits = self.current != NO_CHUNK
            && self.chunks[self.current as usize]
                .as_ref()
                .is_some_and(|c| c.cap - self.write_pos >= need);
        if !fits {
            if self.current != NO_CHUNK
                && self.chunks[self.current as usize]
                    .as_ref()
                    .is_some_and(|c| c.live == 0)
            {
                // The current chunk holds nothing: release it rather
                // than keep an empty one.
                self.release_chunk(self.current);
            }
            self.current = self.put_chunk(Chunk::new(cfg.chunk_bytes));
            self.write_pos = 0;
        }
        let start = self.write_pos;
        self.write_pos += need;
        (self.current, start)
    }

    fn release_chunk(&mut self, chunk: u32) {
        self.chunks[chunk as usize] = None;
        for class in self.free_slots.iter_mut() {
            let entries = &mut self.entries;
            let slotless = &mut self.slotless;
            class.retain(|&id| {
                if entries[id as usize].chunk == chunk {
                    entries[id as usize].chunk = NO_CHUNK;
                    slotless.push(id);
                    false
                } else {
                    true
                }
            });
        }
        if self.current == chunk {
            self.current = NO_CHUNK;
            self.write_pos = 0;
        }
    }

    /// One fewer live item in `chunk`; releases or resets it at zero.
    fn chunk_dec(&mut self, chunk: u32) {
        let c = self.chunks[chunk as usize].as_mut().expect("chunk");
        c.live -= 1;
        if c.live == 0 {
            if chunk == self.current {
                // Keep the chunk being appended to, empty: forget its
                // free slots.
                for class in self.free_slots.iter_mut() {
                    let entries = &mut self.entries;
                    let slotless = &mut self.slotless;
                    class.retain(|&id| {
                        if entries[id as usize].chunk == chunk {
                            entries[id as usize].chunk = NO_CHUNK;
                            slotless.push(id);
                            false
                        } else {
                            true
                        }
                    });
                }
                self.write_pos = 0;
            } else {
                self.release_chunk(chunk);
            }
        }
    }

    fn index_insert(&mut self, key: u32, id: u32) {
        match self.index.get_mut(&key) {
            None => {
                self.index.insert(key, Ids::One(id));
            }
            Some(ids) => match ids {
                Ids::One(other) => *ids = Ids::Many(alloc::vec![*other, id]),
                Ids::Many(v) => v.push(id),
            },
        }
    }

    fn index_remove(&mut self, key: u32, id: u32) {
        let remove_key = match self.index.get_mut(&key) {
            None => false,
            Some(Ids::One(other)) => *other == id,
            Some(ids @ Ids::Many(_)) => {
                if let Ids::Many(v) = ids {
                    v.retain(|&x| x != id);
                    if v.len() == 1 {
                        *ids = Ids::One(v[0]);
                    }
                }
                false
            }
        };
        if remove_key {
            self.index.remove(&key);
        }
    }

    /// Checks the limits for one more string of `len` bytes.
    fn check_new(&self, cfg: &GlobalConfig, len: usize) -> Result<()> {
        if len as u64 >= u32::MAX as u64 - 1 {
            return Err(Error::new(
                ErrorKind::Limit,
                "string longer than u32::MAX - 2 bytes",
            ));
        }
        if self.live_strings >= cfg.max_strings {
            return Err(Error::new(
                ErrorKind::Limit,
                "global table: max_strings",
            ));
        }
        if self.live_bytes + len as u64 + 1 > cfg.max_bytes {
            return Err(Error::new(
                ErrorKind::Limit,
                "global table: max_bytes",
            ));
        }
        Ok(())
    }

    /// Takes an id and space for a new string of `len` bytes (its NUL
    /// included in `cap`).
    fn take_slot(
        &mut self,
        cfg: &GlobalConfig,
        len: u32,
    ) -> Result<(u32, u32, u32, u32)> {
        let need = len + 1;
        for class in class_needed(need)..32 {
            if let Some(pos) = self.free_slots[class]
                .iter()
                .position(|&id| self.entries[id as usize].cap >= need)
            {
                let id = self.free_slots[class].swap_remove(pos);
                let e = self.entries[id as usize];
                return Ok((id, e.chunk, e.start, e.cap));
            }
        }
        let id = self.new_id()?;
        let (chunk, start) = self.alloc(cfg, need);
        Ok((id, chunk, start, need))
    }

    /// An id with no slot: a freed one whose chunk is gone, or a new
    /// one. `ErrorKind::Limit` when ids up to `u32::MAX − 1` are all in
    /// use.
    fn new_id(&mut self) -> Result<u32> {
        if let Some(id) = self.slotless.pop() {
            return Ok(id);
        }
        if self.entries.len() as u64 >= u32::MAX as u64 {
            return Err(Error::new(
                ErrorKind::Limit,
                "global table: no id left",
            ));
        }
        self.entries.push(Entry {
            chunk: NO_CHUNK,
            start: 0,
            len: 0,
            cap: 0,
            hash: 0,
            refs: 0,
        });
        Ok((self.entries.len() - 1) as u32)
    }

    fn write_bytes(&mut self, chunk: u32, start: u32, data: &[u8]) {
        let c = self.chunks[chunk as usize].as_ref().expect("chunk");
        debug_assert!(start as usize + data.len() <= c.cap as usize);
        // SAFETY: the destination is free or reserved space inside the
        // chunk; no `&str` points at it (module documentation,
        // "Soundness rule").
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                c.ptr.as_ptr().add(start as usize),
                data.len(),
            )
        }
    }

    /// Records a new live entry in `id` and returns it.
    #[allow(clippy::too_many_arguments)] // the parts of one entry, kept flat for speed
    fn commit(
        &mut self,
        cfg: &GlobalConfig,
        id: u32,
        chunk: u32,
        start: u32,
        cap: u32,
        len: u32,
        h: u32,
    ) -> u32 {
        self.write_bytes(chunk, start + len, &[0]);
        self.entries[id as usize] =
            Entry { chunk, start, len, cap, hash: h, refs: 1 };
        self.chunks[chunk as usize].as_mut().expect("chunk").live += 1;
        self.live_strings += 1;
        self.live_bytes += len as u64 + 1;
        self.index_insert(Self::key(cfg, h), id);
        id
    }

    fn intern(&mut self, cfg: &GlobalConfig, s: &str) -> Result<u32> {
        let h = hash::hash32(s.as_bytes());
        if let Some(id) = self.find(cfg, s.as_bytes(), h) {
            self.retain(id);
            return Ok(id);
        }
        self.check_new(cfg, s.len())?;
        let len = s.len() as u32;
        let (id, chunk, start, cap) = self.take_slot(cfg, len)?;
        self.write_bytes(chunk, start, s.as_bytes());
        Ok(self.commit(cfg, id, chunk, start, cap, len, h))
    }

    fn retain(&mut self, id: u32) {
        let e = &mut self.entries[id as usize];
        assert!(e.refs > 0, "retain of a freed string id");
        assert!(e.refs < u32::MAX - 1, "string reference count overflow");
        e.refs += 1;
    }

    fn release(&mut self, cfg: &GlobalConfig, id: u32) {
        let e = &mut self.entries[id as usize];
        assert!(e.refs > 0, "release of a freed string id");
        e.refs -= 1;
        if e.refs > 0 {
            return;
        }
        let e = *e;
        self.index_remove(Self::key(cfg, e.hash), id);
        self.live_strings -= 1;
        self.live_bytes -= e.len as u64 + 1;
        self.free_slots[class_of(e.cap)].push(id);
        self.chunk_dec(e.chunk);
    }

    fn stats(&self) -> TableStats {
        let allocated: u64 =
            self.chunks.iter().flatten().map(|c| c.cap as u64).sum();
        TableStats {
            strings: self.live_strings,
            bytes: self.live_bytes,
            free_bytes: allocated - self.live_bytes,
            chunks: self.chunks.iter().flatten().count() as u32,
            longest_list: self
                .index
                .values()
                .map(|ids| ids.as_slice().len() as u32)
                .max()
                .unwrap_or(0),
        }
    }
}

impl<K: Kind> GlobalTable<K> {
    fn with_config(cfg: GlobalConfig) -> Self {
        GlobalTable {
            inner: UnsafeCell::new(Inner::new()),
            cfg,
            _kind: PhantomData,
            _not_sync: PhantomData,
        }
    }

    /// For methods that change the table.
    #[allow(clippy::mut_from_ref)]
    fn inner(&self) -> &mut Inner {
        // SAFETY: the table is not Sync, and no method keeps this
        // reference across a call that obtains it again; `&str`s point
        // into chunk memory, not into `Inner`.
        unsafe { &mut *self.inner.get() }
    }

    /// For methods that only read: no `&mut Inner` is made, so none can
    /// overlap a shared one.
    fn inner_ref(&self) -> &Inner {
        // SAFETY: as for `inner`; this reference also ends before any
        // call that obtains `inner` again.
        unsafe { &*self.inner.get() }
    }

    /// Adds a reference to a live id. Crate-internal: sessions and
    /// `Initial` hold ids.
    pub(crate) fn retain(&self, id: StrId<K>) {
        self.inner().retain(id.0)
    }

    /// Drops a reference; at zero the string is freed.
    pub(crate) fn release(&self, id: StrId<K>) {
        let cfg = self.cfg;
        self.inner().release(&cfg, id.0)
    }

    /// The string of a live id. The caller must hold a reference for as
    /// long as it uses the result.
    pub(crate) fn resolve(&self, id: StrId<K>) -> &str {
        let inner = self.inner_ref();
        let e = &inner.entries[id.0 as usize];
        debug_assert!(e.refs > 0, "resolve of a freed string id");
        // SAFETY: interned bytes always come from a &str or from
        // push_str/push_char.
        unsafe { core::str::from_utf8_unchecked(inner.bytes(e)) }
    }

    /// Interns `s` and returns the id with one reference that the
    /// caller owns (no handle).
    pub(crate) fn intern_id(&self, s: &str) -> Result<StrId<K>> {
        let cfg = self.cfg;
        self.inner().intern(&cfg, s).map(|i| StrId(i, PhantomData))
    }

    /// Wraps an id the caller holds a reference to in a handle that
    /// will release it.
    #[allow(dead_code)] // for readers that keep values as handles; not used yet
    pub(crate) fn hold(&self, id: StrId<K>) -> Held<'_, K> {
        Held { table: self, id }
    }
}

fn check_config(cfg: &GlobalConfig) -> Result<()> {
    if cfg.hash_bits != 16 && cfg.hash_bits != 32 {
        return Err(Error::new(ErrorKind::Arg, "hash_bits must be 16 or 32"));
    }
    if !cfg.chunk_bytes.is_power_of_two()
        || cfg.chunk_bytes < 64
        || cfg.chunk_bytes > 1 << 30
    {
        return Err(Error::new(
            ErrorKind::Arg,
            "chunk_bytes must be a power of two from 64 to 2^30",
        ));
    }
    Ok(())
}

/// Gives back a reservation of `cap` bytes at `start` (`kept` bytes of
/// it stay in use).
fn give_back(inner: &mut Inner, chunk: u32, start: u32, cap: u32, kept: u32) {
    inner.live_bytes -= cap as u64;
    if chunk == inner.current && inner.write_pos == start + cap {
        inner.write_pos = start + kept;
    }
    inner.chunk_dec(chunk);
}

#[cfg(test)]
mod collision_tests {
    #[test]
    fn colliding_strings_stay_distinct() -> crate::Result<()> {
        use crate::strings::{GlobalConfig, GlobalStrings, hash};
        let mut seen = alloc::collections::BTreeMap::new();
        let (x, y) = (0..)
            .map(|i| alloc::format!("s{i}"))
            .find_map(|s| {
                seen.insert(hash::key16(hash::hash32(s.as_bytes())), s.clone())
                    .map(|o| (o, s))
            })
            .unwrap();
        let g = GlobalStrings::new(GlobalConfig::new())?; // 16-bit keys
        let (hx, hy) = (g.values().intern(&x)?, g.values().intern(&y)?);
        assert_ne!(hx.id(), hy.id());
        assert_eq!(
            (g.values().get(&x), g.values().get(&y)),
            (Some(hx.id()), Some(hy.id()))
        );
        assert_eq!(g.values().stats().longest_list, 2);
        Ok(())
    }
}
