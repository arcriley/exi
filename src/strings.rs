//! Global string tables: every distinct string stored once, shared by every session.
//!
//! # What this module is for
//!
//! An EXI stream gives each string a small number so a repeated string is sent as that
//! number. Each stream keeps its own numbering ([`crate::tables`]). A server may hold
//! thousands of streams at once, and most of their strings are the same: the same
//! namespaces, element names and addresses. This module stores each distinct string once
//! for all of them, and gives it an integer id. Two sessions that hold the same id hold the
//! same string, so a server compares strings from different connections as integers.
//!
//! The global tables only share storage. They never change what any stream writes: a stream
//! encoded with shared tables is byte for byte the same as one encoded with private tables.
//!
//! # Four tables, one per kind (ST1)
//!
//! [`GlobalStrings`] holds four [`GlobalTable`]s: URIs, prefixes, local names and values.
//! Strings of different kinds are never compared with each other, so they never share a
//! table. The same text, such as `"id"`, can be in two tables with unrelated ids. Each kind
//! has its own id type ([`UriId`], [`PrefixId`], [`LocalId`], [`ValueId`]), so the compiler
//! rejects an id used with the wrong table (ST7).
//!
//! # References (ST5)
//!
//! Every string counts its references. A string with at least one reference keeps its id
//! and its bytes. When the count reaches zero the string is freed: its id and its bytes may
//! be reused by a later string. References are held by:
//!
//! - a [`Held`] handle, returned by [`GlobalTable::intern`] and [`PendingStr::finish`]. It
//!   releases its reference when dropped. Code that keeps an id for later, such as a router
//!   keeping a JID, keeps a `Held`;
//! - session tables, for every entry they hold (crate-internal `retain`/`release`);
//! - [`Initial`](crate::tables::Initial), for every initial entry.
//!
//! An id obtained with [`GlobalTable::get`] carries no reference. It names the string only
//! while something else holds one. Kept beyond that, it may name a different string later.
//!
//! # Soundness rule for the implementation (ST8)
//!
//! Tables are shared by `&` reference and add strings through interior mutability
//! (`UnsafeCell`). This is sound because of one rule: **the bytes of a string with at least
//! one reference are never written.** Every `&str` this module hands out is borrowed from
//! something that holds a reference (a [`Held`], or a session table), so no `&str` can point
//! at bytes being reused. Consequences for the code:
//!
//! - chunk bytes are written only through raw pointers into free or reserved space; never
//!   form a `&mut [u8]` over a chunk that other strings live in;
//! - chunks are separate heap allocations that never move or shrink; the `Vec` of chunk
//!   pointers may reallocate, which does not move the chunks;
//! - no method keeps a reference into `entries`, the index or the free lists across a call
//!   that can modify them. No method calls back into user code;
//! - `GlobalTable` and `GlobalStrings` are `Send` but not `Sync`: one thread at a time. A
//!   shared form for many threads is an open question (blueprint §8, question 1).
//!
//! Run the tests of this module under Miri.
//!
//! # Representation (§5.1 of the blueprint)
//!
//! | Part | Representation |
//! |---|---|
//! | Arena | A `Vec` of chunks, each a `Box<[u8]>` of `chunk_bytes`. Each string is stored contiguously, followed by one NUL byte that its length does not count, so C callers can use it directly. A string longer than `chunk_bytes − 1` bytes gets a chunk of its own, exactly `len + 1` bytes. A string that does not fit in the rest of the current chunk starts a new chunk; the unused tail is counted in [`TableStats::free_bytes`] |
//! | Entry, per id | `{ chunk: u32, start: u32, len: u32, cap: u32, hash: u32, refs: u32 }`. `cap` is the slot size in bytes (at least `len + 1`). `refs == 0` marks a free entry |
//! | Index | A Swiss table (`hashbrown::HashMap`, hasher `rustc_hash::FxBuildHasher`) from the key of a string's hash to every id with that key. The key is [`hash::key16`] of [`hash::hash32`] with `hash_bits(16)`, or the full `hash32` with `hash_bits(32)`. The value is one id inline (the usual case) or a short list when keys collide |
//! | Free lists | Freed entries by size class: class `k` holds slots with `cap` in `[2^k, 2^(k+1))`, smallest class 16 bytes. A new string of `n + 1` bytes takes a slot from the smallest non-empty class `k` with `2^k ≥ n + 1`. If no slot fits but any freed id exists, that id is reused with fresh bytes, and its old slot stays in its free list without an id. New ids are made only when no freed id exists |
//! | Chunk release | Each chunk counts its live strings. When the count reaches 0, the chunk's slots leave the free lists and the chunk is returned to the allocator; if it is the chunk being appended to, its write position goes back to 0 instead. So memory follows the live strings without a compaction step, which could never run anyway: compacting needs `&mut`, and every holder of a reference borrows the tables |
//!
//! Lookup: hash the string, find its key, and for each id listed compare the length, then
//! the bytes. Insertion: store the bytes and NUL, fill an entry, add the id under its key.
//! Freeing: remove the id from its key's list, move the entry to its free list.
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

use core::cell::Cell;
use core::marker::PhantomData;

use crate::Result;

pub mod hash;

mod sealed {
    pub trait Sealed {}
}

/// The kind of string a table holds (ST1). Sealed: the four kinds below are all there are.
pub trait Kind: sealed::Sealed + Copy + Eq + Ord + core::hash::Hash + core::fmt::Debug + 'static {}

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
/// Ids are dense integers from 0, so callers can index arrays with them. An id is the same
/// string for as long as the string has a reference; after that it may be reused (ST5).
/// Ids from different [`GlobalStrings`] are unrelated, and the type does not tell them
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
        todo!()
    }
}

pub type UriId = StrId<Uri>;
pub type PrefixId = StrId<Prefix>;
pub type LocalId = StrId<Local>;
pub type ValueId = StrId<Value>;

/// Configuration of a global table. Every table of a [`GlobalStrings`] uses the same one.
///
/// | Setting | `new()` | `device()` |
/// |---|---|---|
/// | `hash_bits` | 16 | 16 |
/// | `chunk_bytes` | 16 384 | 1 024 |
/// | `max_bytes` | `u64::MAX` (no limit) | 65 536 |
/// | `max_strings` | `u32::MAX` (no limit) | 4 096 |
///
/// The `device()` values are placeholders until the benchmark suite tunes them. Settings
/// are checked by [`GlobalStrings::new`], not here.
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
        todo!()
    }
    /// Small bounds for microcontrollers.
    pub const fn device() -> Self {
        todo!()
    }
    /// Width of the index key: 16 (default) or 32. Beyond about 30 000 strings in one table,
    /// 16-bit keys give collision lists of more than one id on average; 32-bit keys keep
    /// them short for millions of strings, at the cost of a larger index.
    pub const fn hash_bits(self, bits: u8) -> Self {
        todo!()
    }
    /// Size of each arena chunk in bytes: a power of two from 64 to 2^30.
    pub const fn chunk_bytes(self, n: u32) -> Self {
        todo!()
    }
    /// Most live bytes in one table, counting `len + 1` per string (the NUL included).
    pub const fn max_bytes(self, n: u64) -> Self {
        todo!()
    }
    /// Most live strings in one table.
    pub const fn max_strings(self, n: u32) -> Self {
        todo!()
    }
    pub const fn get_hash_bits(&self) -> u8 {
        todo!()
    }
    pub const fn get_chunk_bytes(&self) -> u32 {
        todo!()
    }
    pub const fn get_max_bytes(&self) -> u64 {
        todo!()
    }
    pub const fn get_max_strings(&self) -> u32 {
        todo!()
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
    /// Live bytes: the sum of `len + 1` over live strings. This is what `max_bytes` bounds.
    pub bytes: u64,
    /// Arena bytes allocated but not holding a live string: freed slots, unused chunk tails
    /// and abandoned reservations. Released with the chunk when its last string is freed.
    pub free_bytes: u64,
    /// Arena chunks allocated, oversized ones included.
    pub chunks: u32,
    /// The longest list of ids sharing one index key. 1 when no keys collide; 0 when empty.
    pub longest_list: u32,
}

/// One global table of strings of kind `K` (see the module documentation).
///
/// `Debug` shows [`TableStats`], not the strings.
pub struct GlobalTable<K: Kind> {
    // Representation: module documentation, "Representation". All of it inside UnsafeCell.
    _kind: PhantomData<K>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<K: Kind> core::fmt::Debug for GlobalTable<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl<K: Kind> GlobalTable<K> {
    /// The id of `s`, if the table holds it. Adds no reference (see "References").
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
        todo!()
    }

    /// The string `s`, added if absent, with one more reference held by the returned
    /// handle. The empty string is a string like any other: it is stored as a single NUL.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when adding `s` would exceed `max_strings` or `max_bytes`, or no
    /// id is left. A string already present never fails: it only gains a reference.
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
    /// Each kind is its own table (ST1):
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let _id = g.locals().intern("id")?;
    /// assert_eq!(g.values().get("id"), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// A value id cannot be used with the URI table (ST7):
    ///
    /// ```compile_fail
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// let g = GlobalStrings::new(GlobalConfig::new()).unwrap();
    /// let v = g.values().intern("x").unwrap();
    /// g.uris().refs(v.id());
    /// ```
    ///
    /// The limit on live strings: at the limit a new string fails and an existing one
    /// succeeds; freeing one makes room again.
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
    /// The limit on live bytes counts `len + 1` per string and allows exactly the limit:
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
    /// A string that fills a chunk exactly (with its NUL), and one larger than a chunk,
    /// which gets a chunk of its own:
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
        todo!()
    }

    /// Starts a string whose bytes arrive in pieces, such as a literal decoded across input
    /// refills. Reserves `max_len + 1` contiguous bytes at once, so several pending strings,
    /// from different sessions on one thread, can be built at the same time.
    ///
    /// `max_len` is an upper bound in bytes. A decoder that knows a literal has `n`
    /// characters passes `4 × n`, the most UTF-8 bytes `n` characters can take.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when `max_len + 1` more live bytes would exceed `max_bytes`, or
    /// `max_len` exceeds `u32::MAX − 1`. The reservation is counted as live bytes until
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
        todo!()
    }

    /// The number of references `id` has: 0 for a freed id or one never made.
    pub fn refs(&self, id: StrId<K>) -> u32 {
        todo!()
    }

    /// Counts describing this table.
    ///
    /// Keys that collide are listed together and both strings are found. The first pair
    /// of the strings "s0", "s1", … whose 16-bit keys collide is found by search here, so
    /// the test holds for either hash function (see [`hash`]).
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig, hash};
    /// let mut seen = std::collections::HashMap::new();
    /// let (x, y) = (0..)
    ///     .map(|i| format!("s{i}"))
    ///     .find_map(|s| seen.insert(hash::key16(hash::hash32(s.as_bytes())), s.clone()).map(|o| (o, s)))
    ///     .unwrap();
    /// let g = GlobalStrings::new(GlobalConfig::new())?;   // 16-bit keys
    /// let (hx, hy) = (g.values().intern(&x)?, g.values().intern(&y)?);
    /// assert_ne!(hx.id(), hy.id());
    /// assert_eq!((g.values().get(&x), g.values().get(&y)), (Some(hx.id()), Some(hy.id())));
    /// assert_eq!(g.values().stats().longest_list, 2);
    /// # Ok::<(), exi::Error>(())
    /// ```
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
        todo!()
    }
}

/// A reference to one string, released when dropped (ST5). Cloning adds a reference.
///
/// The string's bytes cannot change while a `Held` exists, so [`Held::as_str`] is valid for
/// as long as the handle.
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
        todo!()
    }
    pub fn as_str(&self) -> &str {
        todo!()
    }
    /// The table this handle belongs to, so a session can check that a handle comes from
    /// its own global tables.
    pub fn table(&self) -> &'g GlobalTable<K> {
        todo!()
    }
}

impl<K: Kind> core::fmt::Debug for Held<'_, K> {
    /// Shows the id and the string.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl<K: Kind> Clone for Held<'_, K> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<K: Kind> Drop for Held<'_, K> {
    fn drop(&mut self) {
        todo!()
    }
}

/// A string being built in pieces (see [`GlobalTable::begin`]). Dropped without
/// [`finish`](PendingStr::finish), it leaves the table as it was: the reservation is given
/// back if it is the last allocation in its chunk, and is otherwise counted in
/// [`TableStats::free_bytes`].
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
}

impl<'t, K: Kind> PendingStr<'t, K> {
    /// Appends `s`.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when the string would exceed the `max_len` given to `begin`.
    /// Nothing is appended.
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
        todo!()
    }
    /// Appends one character.
    pub fn push_char(&mut self, c: char) -> Result<()> {
        todo!()
    }
    /// Bytes appended so far.
    pub fn len(&self) -> u32 {
        todo!()
    }
    pub fn is_empty(&self) -> bool {
        todo!()
    }
    /// Ends the string. If the table already holds an equal string, that string gains a
    /// reference and this copy is abandoned; otherwise this copy is kept. Either way the
    /// unused part of the reservation is given back as in the type's documentation.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when keeping the string would exceed `max_strings`, or no id is
    /// left. The copy is abandoned.
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
        todo!()
    }
}

impl<K: Kind> core::fmt::Debug for PendingStr<'_, K> {
    /// Shows the bytes appended so far and the reservation.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl<K: Kind> Drop for PendingStr<'_, K> {
    fn drop(&mut self) {
        todo!()
    }
}

/// The four global tables (ST1). Sessions borrow one; sessions that should not share
/// memory or timing with each other, such as different tenants, use separate ones.
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
        todo!()
    }
}

impl GlobalStrings {
    /// Four empty tables, each with `cfg`.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when `hash_bits` is not 16 or 32, or `chunk_bytes` is not a power
    /// of two from 64 to 2^30.
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
        todo!()
    }
    pub fn uris(&self) -> &GlobalTable<Uri> {
        todo!()
    }
    pub fn prefixes(&self) -> &GlobalTable<Prefix> {
        todo!()
    }
    pub fn locals(&self) -> &GlobalTable<Local> {
        todo!()
    }
    pub fn values(&self) -> &GlobalTable<Value> {
        todo!()
    }
}
