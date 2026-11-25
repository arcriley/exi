// src/coding/names.rs
//
// Name, namespace declaration and string value coding
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Every function here looks a string up in the [`SessionTables`] and
//! writes its number when found, or writes it as a literal and adds it,
//! exactly as the decoder will. Readers use the matching `read_`
//! function and add the same entries in the same order.
//!
//! # Rules
//!
//! `m` is the number of entries in the partition at the moment of the
//! call. All widths are `width(x) = ceil(log2 x)`, 0 for `x ≤ 1`.
//!
//! | Item | Found (hit) | Not found (miss) |
//! |---|---|---|
//! | URI | n-bit `number + 1`, `n = width(m + 1)` | n-bit 0, then the URI as a String literal; it is added |
//! | Local name | Unsigned Integer 0, then n-bit `number`, `n = width(m)` | Unsigned Integer `length + 1`, then the characters (no separate length); it is added |
//! | Prefix in an NS event | n-bit `number + 1`, `n = width(m + 1)` | n-bit 0, then the prefix as a String literal; it is added |
//! | Prefix of an SE or AT name (only with `Preserve.prefixes`) | n-bit `number`, `n = width(m)`, among the prefixes of the name's URI | — (see below) |
//! | String value | Unsigned Integer 0, then n-bit local number, `n = width(local_value_count(name))`; or Unsigned Integer 1, then n-bit global number, `n = width(global_value_count())` | Unsigned Integer `length + 2`, then the characters; added when the value rules keep it |
//!
//! A name is written as its URI, then its local name, then (with
//! `Preserve.prefixes`) its prefix. An NS event is its URI, its prefix,
//! then `local_element_ns` as a Boolean.
//!
//! **Prefixes of SE names.** A start element's own prefix is often
//! declared by an NS event that comes after the SE. So: when the URI
//! has no prefixes yet (`m = 0`), the SE writes no prefix; when it has
//! some, the SE writes the index of its prefix if that prefix is
//! already declared, otherwise 0. In both cases, when one of the
//! element's NS events has `local_element_ns = true`, that event's
//! prefix is the element's prefix. A writer sets `local_element_ns` on
//! the NS event declaring the element's prefix, when the element
//! declares it. Attribute names always come after the element's NS
//! events, so their prefixes are always declared; an attribute whose
//! URI has no prefix is `ErrorKind::Grammar` for readers.
//!
//! **Lengths count characters.** Literals are limited by
//! `Limits::max_value_chars` before any memory is reserved.
//!
//! The order of a string value's checks: local partition first, then
//! global (R10).

use alloc::string::String;

use crate::coding::datatypes::{
    read_bool, read_chars, read_uint, write_bool, write_chars, write_string,
    write_uint,
};
use crate::coding::width;
use crate::coding::{Channel, Decoder, Encoder};
use crate::event::NsDecl;
use crate::strings::ValueId;
use crate::tables::SessionTables;
use crate::tables::ValueHit;
use crate::{Error, ErrorKind, Limits, Name, NameId, Result};

const S: Channel = Channel::Structure;

fn grammar(detail: &'static str) -> Error {
    Error::new(ErrorKind::Grammar, detail)
}

/// Reads a literal of `len` characters into a new string, within
/// `max_value_chars`.
fn read_literal<D: Decoder>(
    d: &mut D,
    ch: Channel,
    len: u64,
    limits: &Limits,
) -> Result<String> {
    if len > limits.get_max_value_chars() {
        return Err(Error::new(
            ErrorKind::Limit,
            "literal longer than max_value_chars",
        ));
    }
    let mut s = String::new();
    read_chars(d, ch, len, &mut s)?;
    Ok(s)
}

/// Writes a URI and returns its number, adding it on a miss.
///
/// ```
/// use exi::coding::{byte_aligned::ByteEncoder, names::write_uri};
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// # let g = GlobalStrings::new(GlobalConfig::new())?;
/// # let init = InitialEntries::schemaless().attach(&g)?;
/// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let mut e = ByteEncoder::new(Vec::new());
/// assert_eq!(write_uri(&mut e, &mut t, "")?, 0);          // 3 URIs: width(4) = 2 bits, value 1
/// assert_eq!(write_uri(&mut e, &mut t, "urn:x")?, 3);     // miss: 0, then the literal
/// assert_eq!(write_uri(&mut e, &mut t, "urn:x")?, 3);     // 4 URIs: width(5) = 3 bits, value 4
/// assert_eq!(e.into_sink(), [0x01, 0x00, 0x05, b'u', b'r', b'n', b':', b'x', 0x04]);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_uri<E: Encoder>(
    e: &mut E,
    t: &mut SessionTables<'_>,
    uri: &str,
) -> Result<u32> {
    let w = width(t.uri_count() + 1);
    match t.find_uri_str(uri) {
        Some(n) => {
            e.nbit(S, n + 1, w)?;
            Ok(n)
        }
        None => {
            let n = t.add_uri(uri)?;
            e.nbit(S, 0, w)?;
            write_string(e, S, uri)?;
            Ok(n)
        }
    }
}

/// # Errors
///
/// `ErrorKind::Grammar` for a number the partition does not have;
/// `ErrorKind::Limit` for a literal beyond `max_value_chars` or a table
/// limit.
pub fn read_uri<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &mut T,
    limits: &Limits,
) -> Result<u32> {
    let m = t.uri_count();
    let v = d.nbit(S, width(m + 1))?;
    if v == 0 {
        let len = read_uint(d, S, limits.get_max_integer_octets())?;
        let s = read_literal(d, S, len, limits)?;
        t.add_uri(&s)
    } else if v - 1 < m {
        Ok(v - 1)
    } else {
        Err(grammar("URI number beyond the partition"))
    }
}

/// Writes a local name in the partition of URI number `uri` and returns
/// its number.
///
/// ```
/// use exi::coding::{byte_aligned::ByteEncoder, names::write_local};
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// # let g = GlobalStrings::new(GlobalConfig::new())?;
/// # let init = InitialEntries::schemaless().attach(&g)?;
/// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let mut e = ByteEncoder::new(Vec::new());
/// write_local(&mut e, &mut t, 0, "a")?;     // miss: length + 1 = 2, then 'a'
/// write_local(&mut e, &mut t, 0, "a")?;     // hit: 0, then width(1) = 0 bits
/// write_local(&mut e, &mut t, 0, "b")?;     // miss
/// write_local(&mut e, &mut t, 0, "a")?;     // hit: 0, then width(2) = 1 bit: number 0
/// assert_eq!(e.into_sink(), [0x02, b'a', 0x00, 0x02, b'b', 0x00, 0x00]);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_local<E: Encoder>(
    e: &mut E,
    t: &mut SessionTables<'_>,
    uri: u32,
    local: &str,
) -> Result<u32> {
    let m = t.local_count(uri);
    let hit =
        t.global_strings_local(local).and_then(|id| t.find_local(uri, id));
    match hit {
        Some(n) => {
            write_uint(e, S, 0)?;
            e.nbit(S, n, width(m))?;
            Ok(n)
        }
        None => {
            let n = t.add_local(uri, local)?;
            write_uint(e, S, local.chars().count() as u64 + 1)?;
            write_chars(e, S, local)?;
            Ok(n)
        }
    }
}

/// # Errors
///
/// As [`read_uri`].
pub fn read_local<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &mut T,
    uri: u32,
    limits: &Limits,
) -> Result<u32> {
    let m = t.local_count(uri);
    let v = read_uint(d, S, limits.get_max_integer_octets())?;
    if v == 0 {
        let n = d.nbit(S, width(m))?;
        if n >= m {
            return Err(grammar("local-name number beyond the partition"));
        }
        Ok(n)
    } else {
        let s = read_literal(d, S, v - 1, limits)?;
        t.add_local(uri, &s)
    }
}

/// Writes a name (URI, local name, and the prefix when `prefixes` is
/// true) and returns its numbers.
///
/// ```
/// use exi::coding::{byte_aligned::ByteEncoder, names::write_name};
/// use exi::{Name, NameId};
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// # let g = GlobalStrings::new(GlobalConfig::new())?;
/// # let init = InitialEntries::schemaless().attach(&g)?;
/// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let mut e = ByteEncoder::new(Vec::new());
/// let xml_lang = Name::new("http://www.w3.org/XML/1998/namespace", "lang");
/// assert_eq!(write_name(&mut e, &mut t, xml_lang, false)?, NameId { uri: 1, local: 2 });
/// assert_eq!(e.into_sink(), [0x02, 0x00, 0x02]);   // URI 1 → 2; hit; number 2 of 4 (2 bits)
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_name<E: Encoder>(
    e: &mut E,
    t: &mut SessionTables<'_>,
    name: Name<'_>,
    prefixes: bool,
) -> Result<NameId> {
    let uri = write_uri(e, t, name.uri())?;
    let local = write_local(e, t, uri, name.local_name())?;
    if prefixes {
        let m = t.prefix_count(uri);
        if m > 0 {
            let idx = name
                .prefix()
                .and_then(|p| t.global_strings_prefix(p))
                .and_then(|id| t.find_prefix(uri, id))
                .unwrap_or(0);
            e.nbit(S, idx, width(m))?;
        }
    }
    Ok(NameId { uri, local })
}

/// Reads a name. Returns its numbers and the prefix number when
/// prefixes are written and the URI has prefixes (module
/// documentation).
pub fn read_name<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &mut T,
    prefixes: bool,
    limits: &Limits,
) -> Result<(NameId, Option<u32>)> {
    let uri = read_uri(d, t, limits)?;
    let local = read_local(d, t, uri, limits)?;
    let mut prefix = None;
    if prefixes {
        let m = t.prefix_count(uri);
        if m > 0 {
            let n = d.nbit(S, width(m))?;
            if n >= m {
                return Err(grammar("prefix number beyond the partition"));
            }
            prefix = Some(n);
        }
    }
    Ok((NameId { uri, local }, prefix))
}

/// Writes an NS event's content: URI, prefix, `local_element_ns`.
///
/// ```
/// use exi::coding::{byte_aligned::ByteEncoder, names::write_ns};
/// use exi::NsDecl;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// # let g = GlobalStrings::new(GlobalConfig::new())?;
/// # let init = InitialEntries::schemaless().attach(&g)?;
/// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let mut e = ByteEncoder::new(Vec::new());
/// write_ns(&mut e, &mut t, NsDecl { prefix: "p", uri: "urn:x", local_element_ns: true })?;
/// // URI miss and literal; the new URI has no prefixes: width(0 + 1) = 0 bits for the miss,
/// // then the prefix literal; then the Boolean.
/// assert_eq!(e.into_sink(), [0x00, 0x05, b'u', b'r', b'n', b':', b'x', 0x01, b'p', 0x01]);
/// assert_eq!(t.prefix_str(3, 0), Some("p"));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_ns<E: Encoder>(
    e: &mut E,
    t: &mut SessionTables<'_>,
    ns: NsDecl<'_>,
) -> Result<()> {
    let uri = write_uri(e, t, ns.uri)?;
    let w = width(t.prefix_count(uri) + 1);
    match t
        .global_strings_prefix(ns.prefix)
        .and_then(|id| t.find_prefix(uri, id))
    {
        Some(n) => e.nbit(S, n + 1, w)?,
        None => {
            t.add_prefix(uri, ns.prefix)?;
            e.nbit(S, 0, w)?;
            write_string(e, S, ns.prefix)?;
        }
    }
    write_bool(e, S, ns.local_element_ns)
}

/// Reads an NS event's content. Returns `(uri number, prefix number,
/// local_element_ns)`.
pub fn read_ns<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &mut T,
    limits: &Limits,
) -> Result<(u32, u32, bool)> {
    let uri = read_uri(d, t, limits)?;
    let m = t.prefix_count(uri);
    let v = d.nbit(S, width(m + 1))?;
    let prefix = if v == 0 {
        let len = read_uint(d, S, limits.get_max_integer_octets())?;
        let s = read_literal(d, S, len, limits)?;
        t.add_prefix(uri, &s)?
    } else if v - 1 < m {
        v - 1
    } else {
        return Err(grammar("prefix number beyond the partition"));
    };
    Ok((uri, prefix, read_bool(d, S)?))
}

/// Writes a string value of `name` in [`Channel::Value`]`(name)`: a
/// local hit, a global hit, or a literal, which is added when the value
/// rules keep it.
///
/// ```
/// use exi::coding::{byte_aligned::ByteEncoder, names::write_value};
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// # let g = GlobalStrings::new(GlobalConfig::new())?;
/// # let init = InitialEntries::schemaless().attach(&g)?;
/// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let n = t.add_name("", "n")?;
/// let m = t.add_name("", "m")?;
/// let mut e = ByteEncoder::new(Vec::new());
/// write_value(&mut e, &mut t, n, "hi")?;    // literal: 2 + 2 = 4
/// write_value(&mut e, &mut t, n, "hi")?;    // local hit: 0, width(1) = 0 bits
/// write_value(&mut e, &mut t, m, "hi")?;    // global hit: 1, width(1) = 0 bits
/// write_value(&mut e, &mut t, m, "")?;      // literal of length 0: 2; never added
/// assert_eq!(e.into_sink(), [0x04, b'h', b'i', 0x00, 0x01, 0x02]);
/// assert_eq!(t.global_value_count(), 1);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_value<E: Encoder>(
    e: &mut E,
    t: &mut SessionTables<'_>,
    name: NameId,
    s: &str,
) -> Result<()> {
    let ch = Channel::Value(name);
    match t.find_value_str(name, s) {
        ValueHit::Local(n) => {
            write_uint(e, ch, 0)?;
            e.nbit(ch, n, width(t.local_value_count(name)))
        }
        ValueHit::Global(n) => {
            write_uint(e, ch, 1)?;
            e.nbit(ch, n, width(t.global_value_count()))
        }
        ValueHit::Miss => {
            t.add_value(name, s)?;
            write_uint(e, ch, s.chars().count() as u64 + 2)?;
            write_chars(e, ch, s)
        }
    }
}

/// How a string value arrived.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ValueRead {
    /// A hit: the value is this global string.
    Hit(ValueId),
    /// A literal, now in the caller's buffer. `Some` when the tables
    /// kept it.
    Literal(Option<ValueId>),
}

/// Reads a string value of `name`. A literal is appended to `out`.
///
/// # Errors
///
/// `ErrorKind::Grammar` for a local or global number the partition does
/// not have (with local partitions off, any local hit);
/// `ErrorKind::Limit` for a literal beyond `max_value_chars`.
pub fn read_value<D: Decoder, T: ReadTables + ?Sized>(
    d: &mut D,
    t: &mut T,
    name: NameId,
    limits: &Limits,
    out: &mut String,
) -> Result<ValueRead> {
    let ch = Channel::Value(name);
    let v = read_uint(d, ch, limits.get_max_integer_octets())?;
    match v {
        0 => {
            if !t.rules().get_local_partitions() {
                return Err(grammar(
                    "local value hit without local value partitions",
                ));
            }
            let n = d.nbit(ch, width(t.local_value_count(name)))?;
            t.local_value(name, n)
                .map(ValueRead::Hit)
                .ok_or_else(|| grammar("local value number not assigned"))
        }
        1 => {
            let n = d.nbit(ch, width(t.global_value_count()))?;
            t.global_value(n).map(ValueRead::Hit).ok_or_else(|| {
                grammar("global value number beyond the partition")
            })
        }
        _ => {
            let s = read_literal(d, ch, v - 2, limits)?;
            let kept = t.add_value(name, &s)?;
            out.push_str(&s);
            Ok(ValueRead::Literal(kept))
        }
    }
}

/// The channel for values of `name`.
pub const fn value_channel(name: NameId) -> Channel {
    Channel::Value(name)
}

/// What the read functions need from session tables. Implemented by
/// [`SessionTables`]; readers also implement it with a view that only
/// counts additions, to try an event on input that may be incomplete
/// without changing the tables.
pub trait ReadTables {
    fn uri_count(&self) -> u32;
    fn add_uri(&mut self, s: &str) -> Result<u32>;
    fn local_count(&self, uri: u32) -> u32;
    fn add_local(&mut self, uri: u32, s: &str) -> Result<u32>;
    fn prefix_count(&self, uri: u32) -> u32;
    fn add_prefix(&mut self, uri: u32, s: &str) -> Result<u32>;
    fn rules(&self) -> crate::tables::ValueRules;
    fn local_value_count(&self, name: NameId) -> u32;
    fn global_value_count(&self) -> u32;
    fn local_value(
        &self,
        name: NameId,
        n: u32,
    ) -> Option<crate::strings::ValueId>;
    fn global_value(&self, n: u32) -> Option<crate::strings::ValueId>;
    fn add_value(
        &mut self,
        name: NameId,
        s: &str,
    ) -> Result<Option<crate::strings::ValueId>>;
}

impl ReadTables for SessionTables<'_> {
    fn uri_count(&self) -> u32 {
        SessionTables::uri_count(self)
    }
    fn add_uri(&mut self, s: &str) -> Result<u32> {
        SessionTables::add_uri(self, s)
    }
    fn local_count(&self, uri: u32) -> u32 {
        SessionTables::local_count(self, uri)
    }
    fn add_local(&mut self, uri: u32, s: &str) -> Result<u32> {
        SessionTables::add_local(self, uri, s)
    }
    fn prefix_count(&self, uri: u32) -> u32 {
        SessionTables::prefix_count(self, uri)
    }
    fn add_prefix(&mut self, uri: u32, s: &str) -> Result<u32> {
        SessionTables::add_prefix(self, uri, s)
    }
    fn rules(&self) -> crate::tables::ValueRules {
        SessionTables::rules(self)
    }
    fn local_value_count(&self, name: NameId) -> u32 {
        SessionTables::local_value_count(self, name)
    }
    fn global_value_count(&self) -> u32 {
        SessionTables::global_value_count(self)
    }
    fn local_value(
        &self,
        name: NameId,
        n: u32,
    ) -> Option<crate::strings::ValueId> {
        SessionTables::local_value(self, name, n)
    }
    fn global_value(&self, n: u32) -> Option<crate::strings::ValueId> {
        SessionTables::global_value(self, n)
    }
    fn add_value(
        &mut self,
        name: NameId,
        s: &str,
    ) -> Result<Option<crate::strings::ValueId>> {
        SessionTables::add_value(self, name, s)
    }
}
