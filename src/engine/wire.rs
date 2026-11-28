// src/engine/wire.rs
//
// EXI event and value coding
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Coding of events and values after the grammar supplies their event
//! codes and types.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Slot, State, XSI_TYPE, err, internal};
use crate::coding::datatypes::*;
use crate::coding::names::{self, ReadTables, ValueRead};
use crate::coding::{Channel, Decoder, Encoder, width};
use crate::grammar::informed::{
    IntegerRep, Representation, Schema, SimpleTypeId,
};
use crate::grammar::{EventCode, Terminal};
use crate::lexical;
use crate::strings::ValueId;
use crate::tables::{SessionTables, ValueRules};
use crate::value::{BinaryKind, OwnedValue, Value};
use crate::{Error, ErrorKind, Limits, NameId, OwnedName, Result};

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
fn lexical_charset(schema: &Schema, ty: SimpleTypeId) -> Option<Box<[char]>> {
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
        self.t.local_count(uri) + added(&self.locals, uri)
    }
    fn add_local(&mut self, uri: u32, _: &str) -> Result<u32> {
        let n = self.local_count(uri);
        bump(&mut self.locals, uri);
        Ok(n)
    }
    fn prefix_count(&self, uri: u32) -> u32 {
        self.t.prefix_count(uri) + added(&self.prefixes, uri)
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

/// Single-pass [`ReadTables`] wrapper used on streaming input when
/// `!final_input`: delegates hits and value additions directly to
/// `SessionTables`, and signals `need_probe` only if an event tries to
/// add a new URI, local name, or prefix (so a subsequent `Truncated`
/// can never leave partial name-table mutations behind).
pub(crate) struct TryTables<'p, 'a> {
    t: &'p mut SessionTables<'a>,
    pub need_probe: bool,
}

impl<'p, 'a> TryTables<'p, 'a> {
    pub fn new(t: &'p mut SessionTables<'a>) -> Self {
        TryTables { t, need_probe: false }
    }
}

impl ReadTables for TryTables<'_, '_> {
    fn uri_count(&self) -> u32 {
        self.t.uri_count()
    }
    fn add_uri(&mut self, _: &str) -> Result<u32> {
        self.need_probe = true;
        Err(err(ErrorKind::Truncated, "probe needed for new URI"))
    }
    fn local_count(&self, uri: u32) -> u32 {
        self.t.local_count(uri)
    }
    fn add_local(&mut self, _: u32, _: &str) -> Result<u32> {
        self.need_probe = true;
        Err(err(ErrorKind::Truncated, "probe needed for new local name"))
    }
    fn prefix_count(&self, uri: u32) -> u32 {
        self.t.prefix_count(uri)
    }
    fn add_prefix(&mut self, _: u32, _: &str) -> Result<u32> {
        self.need_probe = true;
        Err(err(ErrorKind::Truncated, "probe needed for new prefix"))
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
    fn add_value(&mut self, name: NameId, s: &str) -> Result<Option<ValueId>> {
        self.t.add_value(name, s)
    }
}

// Reading events and values

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

/// True when a value of this slot and name travels in a value channel:
/// everything but `xsi:type` and a typed `xsi:nil`, which choose the
/// grammar (EXI 9.2.1).
fn in_value_channel(slot: Slot, name: NameId) -> bool {
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
    let mut parts = [0; 3];
    let mut len = 0;
    let term = {
        let table = st.table()?;
        loop {
            let c = table.count(&parts[..len]);
            if c == 0 {
                return Err(err(
                    ErrorKind::Grammar,
                    "no event may follow here",
                ));
            }
            parts[len] = read_code_part(d, c)?;
            len += 1;
            if let Some(term) = table.lookup(&parts[..len]) {
                break term;
            }
            if len == parts.len() {
                return Err(internal("event code longer than three parts"));
            }
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
                let p = if prefixes {
                    names::read_prefix(d, t, id.uri)?
                } else {
                    None
                };
                Ok((id, p))
            }
            Terminal::SeUri(u) | Terminal::AtUri(u) => {
                let local = names::read_local(d, t, u, &limits)?;
                let p =
                    if prefixes { names::read_prefix(d, t, u)? } else { None };
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
        Terminal::SC | Terminal::EE | Terminal::ED => {}
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
            let p = if st.prefixes() {
                names::read_prefix(d, t, uri)?
            } else {
                None
            };
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
    let start = out.len();
    read_restricted_chars(d, ch, len, set, out)?;
    let kept = t.add_value(name, &out[start..])?;
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
            match read_int_or_big(d, ch, oct)? {
                Ok(v) => OwnedValue::Int(v),
                Err(s) => OwnedValue::BigInt(s.into()),
            }
        }
        Representation::Integer(IntegerRep::Unsigned) => {
            match read_uint_or_big(d, ch, oct)? {
                Ok(v) => OwnedValue::Int(v),
                Err(s) => OwnedValue::BigInt(s.into()),
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

// Preparing and writing values

/// A value checked against its slot and ready to write: nothing about
/// it can be refused any more.
#[derive(Clone, Debug)]
pub(crate) enum Prepared<'a> {
    Str(Cow<'a, str>, Option<Box<[char]>>),
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
    List(Vec<Prepared<'static>>),
    Enum(u32, u32),
}

impl Prepared<'_> {
    pub(crate) fn into_owned(self) -> Prepared<'static> {
        match self {
            Self::Str(s, cs) => Prepared::Str(Cow::Owned(s.into_owned()), cs),
            Self::QName(u, l, p) => Prepared::QName(u, l, p),
            Self::Bool(b) => Prepared::Bool(b),
            Self::BoolPattern(b, d) => Prepared::BoolPattern(b, d),
            Self::Int(i) => Prepared::Int(i),
            Self::UInt(i) => Prepared::UInt(i),
            Self::BigUInt(s) => Prepared::BigUInt(s),
            Self::Bounded(i, lo, hi) => Prepared::Bounded(i, lo, hi),
            Self::Decimal(d) => Prepared::Decimal(d),
            Self::Float(f) => Prepared::Float(f),
            Self::Binary(b) => Prepared::Binary(b),
            Self::DateTime(d) => Prepared::DateTime(d),
            Self::List(v) => Prepared::List(v),
            Self::Enum(i, n) => Prepared::Enum(i, n),
        }
    }
}

fn lexical_text<'a>(v: &Value<'a>) -> Result<Cow<'a, str>> {
    match v.as_str() {
        Some(s) => Ok(Cow::Borrowed(s)),
        None => lexical_string(v).map(Cow::Owned),
    }
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
pub(crate) fn prepare<'a>(
    schema: Option<&Schema>,
    slot: Slot,
    v: &Value<'a>,
) -> Result<Prepared<'a>> {
    match slot {
        Slot::Str => Ok(Prepared::Str(lexical_text(v)?, None)),
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
            _ => lexical::parse_bool(&lexical_text(v)?)
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
            Ok(Prepared::Str(lexical_text(v)?, lexical_charset(schema, ty)))
        }
        Slot::NilText => {
            let s = lexical_text(v)?;
            lexical::parse_bool(&s)
                .ok_or_else(|| unencodable("xsi:nil needs a Boolean"))?;
            Ok(Prepared::Str(s, Some(boolean_charset())))
        }
    }
}

fn prepare_typed<'a>(
    schema: &Schema,
    ty: SimpleTypeId,
    v: &Value<'a>,
) -> Result<Prepared<'a>> {
    let bad = |d| Err(unencodable(d));
    let text = || lexical_text(v);
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
                        out.push(prepare_typed(schema, *item, &x)?.into_owned());
                    }
                }
                _ => {
                    let s = text()?;
                    for x in lexical::list_items(&s) {
                        out.push(prepare_typed(
                            schema,
                            *item,
                            &Value::Str(x),
                        )?.into_owned());
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
                names::write_prefix(e, t, u, prefix.as_deref())?;
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
