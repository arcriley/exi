// src/value.rs
//
// Values of attributes and character content
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Schema-less streams carry every value as a string. Schema-informed
//! streams carry values typed by their schema datatype, and the reader
//! delivers them typed: a program that reads numbers gets
//! numbers. [`Value::write_lexical`] gives the text form to callers
//! that want it, written into a caller buffer without allocating.
//!
//! # Datatypes and their values
//!
//! | EXI datatype | `Value` | Wire form (written by [`crate::coding::datatypes`]) |
//! |---|---|---|
//! | String | `Str` | length in characters, then each character as a code point (or a table hit) |
//! | Boolean | `Bool`; `BoolDigit` when the type has a pattern facet and the form was `0`/`1` | 1 bit; 2 bits with a pattern facet: 0 `false`, 1 `0`, 2 `true`, 3 `1` |
//! | Integer, Unsigned Integer | `Int(i64)`, or `BigInt(&str)` of decimal digits when outside `i64` | see `coding::datatypes` |
//! | Decimal | `Decimal`, or `BigDecimal(&str)` when a part exceeds `u64` | sign, integral part, fraction with its digits reversed |
//! | Float | `Float` | mantissa and base-10 exponent |
//! | Binary | `Binary(&[u8], BinaryKind)` | length, then the octets |
//! | Date-Time | `DateTime` | components by kind |
//! | QName | `QName(Name)` | the value of `xsi:type` only |
//! | List | `List(ListRef)` | item count, then each item |
//! | Enumeration | `Enum { ordinal, text }` | the ordinal in the type's value order |
//!
//! # Lexical forms written by `write_lexical`
//!
//! Each is the canonical form XML Schema 1.0 defines for the datatype,
//! so equal values always print equally.
//!
//! | Value | Form | Examples |
//! |---|---|---|
//! | `Str`, `BigInt`, `BigDecimal`, `Enum` | the text as is | |
//! | `Bool` | `true`, `false` | |
//! | `BoolDigit` | `1`, `0` | |
//! | `Int` | decimal digits, `-` before negatives, no `+`, no leading zeros | `0`, `-12` |
//! | `Decimal` | integral digits, `.`, fraction digits; at least one digit on each side; trailing fraction zeros removed except one; no sign on zero | `3.05`, `3.0`, `-0.5`, `0.0` |
//! | `Float` | one non-zero digit, `.`, at least one more digit, `E`, exponent without `+` or leading zeros; zero is `0.0E0`; special values `INF`, `-INF`, `NaN` | `2.153E1`, `-1.0E-3` |
//! | `Binary` Base64 | RFC 4648 Base64 with `=` padding and no line breaks | `aGk=` |
//! | `Binary` Hex | two upper-case hex digits per octet | `0AFF` |
//! | `DateTime` | by kind, below | |
//! | `QName` | `prefix:local`, or `local` without a prefix | |
//! | `List` | the items' forms separated by one space | `1 2 3` |
//!
//! Date-time forms. `YYYY` is the year with at least four digits and
//! `-` when negative; the other fields are two digits. `.fff` is the
//! fraction with its leading zeros, trailing zeros removed, omitted
//! when 0. The time zone is `Z` for 0 minutes, otherwise `+hh:mm`
//! or `-hh:mm`, omitted when absent.
//!
//! | Kind | Form |
//! |---|---|
//! | `GYear` | `YYYY` |
//! | `GYearMonth` | `YYYY-MM` |
//! | `Date` | `YYYY-MM-DD` |
//! | `DateTime` | `YYYY-MM-DDThh:mm:ss.fff` |
//! | `Time` | `hh:mm:ss.fff` |
//! | `GMonth` | `--MM` |
//! | `GMonthDay` | `--MM-DD` |
//! | `GDay` | `---DD` |

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::name::{Name, OwnedName};
use crate::{Error, ErrorKind, Result};

/// A value, borrowing its strings from the reader or the caller.
///
/// ```
/// use exi::{Value, Decimal, Float, ListRef};
/// let mut buf = [0u8; 32];
/// assert_eq!(Value::Int(-12).write_lexical(&mut buf)?, "-12");
/// assert_eq!(Value::Int(i64::MIN).write_lexical(&mut buf)?, "-9223372036854775808");
/// assert_eq!(Value::Decimal(Decimal::new(false, 3, 5, 2)).write_lexical(&mut buf)?, "3.05");
/// assert_eq!(Value::Decimal(Decimal::new(false, 3, 0, 0)).write_lexical(&mut buf)?, "3.0");
/// assert_eq!(Value::Decimal(Decimal::new(false, 3, 50, 3)).write_lexical(&mut buf)?, "3.05");
/// assert_eq!(Value::Decimal(Decimal::new(true, 0, 0, 0)).write_lexical(&mut buf)?, "0.0");
/// assert_eq!(Value::Float(Float { mantissa: 2153, exponent: -2 }).write_lexical(&mut buf)?, "2.153E1");
/// assert_eq!(Value::Float(Float { mantissa: -1, exponent: -3 }).write_lexical(&mut buf)?, "-1.0E-3");
/// assert_eq!(Value::Float(Float { mantissa: 0, exponent: 5 }).write_lexical(&mut buf)?, "0.0E0");
/// assert_eq!(Value::Float(Float::NEG_INFINITY).write_lexical(&mut buf)?, "-INF");
/// assert_eq!(Value::Float(Float::NAN).write_lexical(&mut buf)?, "NaN");
/// assert_eq!(Value::BoolDigit(true).write_lexical(&mut buf)?, "1");
/// assert_eq!(Value::Binary(b"hi", exi::BinaryKind::Base64).write_lexical(&mut buf)?, "aGk=");
/// assert_eq!(Value::Binary(&[0x0a, 0xff], exi::BinaryKind::Hex).write_lexical(&mut buf)?, "0AFF");
/// assert_eq!(Value::Binary(&[], exi::BinaryKind::Base64).write_lexical(&mut buf)?, "");
/// let items = [Value::Int(1), Value::Int(2), Value::Int(3)];
/// assert_eq!(Value::List(ListRef::Values(&items)).write_lexical(&mut buf)?, "1 2 3");
/// assert_eq!(Value::List(ListRef::Values(&[])).write_lexical(&mut buf)?, "");
/// # Ok::<(), exi::Error>(())
/// ```
///
/// A buffer that is too small is `ErrorKind::BufferFull`; exactly large
/// enough succeeds.
///
/// ```
/// use exi::{Value, ErrorKind};
/// let mut two = [0u8; 2];
/// assert_eq!(Value::Str("hi").write_lexical(&mut two)?, "hi");
/// assert_eq!(Value::Str("hey").write_lexical(&mut two).unwrap_err().kind(), ErrorKind::BufferFull);
/// assert_eq!(Value::Str("").write_lexical(&mut [])?, "");
/// # Ok::<(), exi::Error>(())
/// ```
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Value<'a> {
    Str(&'a str),
    Bool(bool),
    /// A Boolean whose lexical form was `1` or `0` (types with a
    /// pattern facet).
    BoolDigit(bool),
    Int(i64),
    /// An integer outside `i64`: optional `-`, then decimal digits
    /// without leading zeros.
    BigInt(&'a str),
    Decimal(Decimal),
    /// A decimal whose integral or fraction part exceeds `u64`, in
    /// canonical lexical form.
    BigDecimal(&'a str),
    Float(Float),
    Binary(&'a [u8], BinaryKind),
    DateTime(DateTime),
    QName(Name<'a>),
    List(ListRef<'a>),
    Enum {
        ordinal: u32,
        text: &'a str,
    },
}

impl<'a> Value<'a> {
    /// The canonical lexical form (module table), written into `buf`.
    ///
    /// # Errors
    ///
    /// `ErrorKind::BufferFull` when `buf` is too small;
    /// `ErrorKind::Value` for a value no lexical form exists for: a
    /// `Decimal` whose fraction has more digits than `fraction_digits`,
    /// a `DateTime` with a field out of range (month 0 or above 12, day
    /// 0 or above 31, hour above 24, minute or second above 59, time
    /// zone beyond ±14:00).
    pub fn write_lexical<'b>(&self, buf: &'b mut [u8]) -> Result<&'b str> {
        let mut c = Cursor { buf, len: 0 };
        write_value(self, &mut c)?;
        let Cursor { buf, len } = c;
        // Every piece written was a &str, so the bytes are UTF-8.
        Ok(core::str::from_utf8(&buf[..len]).expect("UTF-8"))
    }
    /// The text of `Str`, `BigInt`, `BigDecimal` and `Enum`.
    pub fn as_str(&self) -> Option<&'a str> {
        match *self {
            Value::Str(s) | Value::BigInt(s) | Value::BigDecimal(s) => Some(s),
            Value::Enum { text, .. } => Some(text),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        if let Value::Int(i) = *self { Some(i) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) | Value::BoolDigit(b) => Some(b),
            _ => None,
        }
    }
    pub fn to_owned(&self) -> OwnedValue {
        match *self {
            Value::Str(s) => OwnedValue::Str(s.into()),
            Value::Bool(b) => OwnedValue::Bool(b),
            Value::BoolDigit(b) => OwnedValue::BoolDigit(b),
            Value::Int(i) => OwnedValue::Int(i),
            Value::BigInt(s) => OwnedValue::BigInt(s.into()),
            Value::Decimal(d) => OwnedValue::Decimal(d),
            Value::BigDecimal(s) => OwnedValue::BigDecimal(s.into()),
            Value::Float(f) => OwnedValue::Float(f),
            Value::Binary(b, k) => OwnedValue::Binary(b.into(), k),
            Value::DateTime(d) => OwnedValue::DateTime(d),
            Value::QName(n) => OwnedValue::QName(n.to_owned()),
            Value::List(items) => {
                OwnedValue::List(items.iter().map(|v| v.to_owned()).collect())
            }
            Value::Enum { ordinal, text } => {
                OwnedValue::Enum { ordinal, text: text.into() }
            }
        }
    }
}

/// The items of a list value: borrowed values, or the items of an
/// [`OwnedValue::List`].
///
/// ```
/// use exi::{ListRef, OwnedValue, Value};
/// let owned = OwnedValue::List(vec![OwnedValue::Int(1), OwnedValue::Str("a".into())]);
/// let v = owned.as_value();
/// let Value::List(items) = v else { unreachable!() };
/// assert_eq!(items.len(), 2);
/// assert_eq!(items.iter().collect::<Vec<_>>(), [Value::Int(1), Value::Str("a")]);
/// assert_eq!(v.to_owned(), owned);
/// assert!(ListRef::Values(&[]).is_empty());
/// ```
#[derive(Copy, Clone, Debug)]
pub enum ListRef<'a> {
    Values(&'a [Value<'a>]),
    Owned(&'a [OwnedValue]),
}

impl<'a> ListRef<'a> {
    pub fn len(&self) -> usize {
        match self {
            ListRef::Values(v) => v.len(),
            ListRef::Owned(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn get(&self, i: usize) -> Option<Value<'a>> {
        match self {
            ListRef::Values(v) => v.get(i).copied(),
            ListRef::Owned(v) => v.get(i).map(OwnedValue::as_value),
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = Value<'a>> + '_ {
        (0..self.len()).map(|i| self.get(i).expect("index below len"))
    }
}

/// Item by item.
impl PartialEq for ListRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

/// How a binary value is written as text.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BinaryKind {
    Base64,
    Hex,
}

/// A decimal number: `integral.fraction`, where the fraction has
/// `fraction_digits` digits including its leading zeros. 3.05 is
/// `integral 3, fraction 5, fraction_digits 2`.
///
/// `fraction` must be below `10^fraction_digits`, and `fraction_digits`
/// at most 19.
///
/// ```
/// use exi::Decimal;
/// assert_eq!(Decimal::from_scaled(2153, 2), Decimal::new(false, 21, 53, 2));
/// assert_eq!(Decimal::from_scaled(-5, 1), Decimal::new(true, 0, 5, 1));
/// assert_eq!(Decimal::from_scaled(7, 0), Decimal::new(false, 7, 0, 0));
/// assert_eq!(Decimal::from_scaled(i64::MIN, 0), Decimal::new(true, 9_223_372_036_854_775_808, 0, 0));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Decimal {
    pub negative: bool,
    pub integral: u64,
    pub fraction: u64,
    pub fraction_digits: u8,
}

impl Decimal {
    pub const fn new(
        negative: bool,
        integral: u64,
        fraction: u64,
        fraction_digits: u8,
    ) -> Self {
        Decimal { negative, integral, fraction, fraction_digits }
    }
    /// `value / 10^fraction_digits`: a scaled integer, as sensors
    /// report readings.
    pub const fn from_scaled(value: i64, fraction_digits: u8) -> Self {
        let negative = value < 0;
        let magnitude = value.unsigned_abs();
        let scale = 10u64.pow(fraction_digits as u32);
        Decimal {
            negative,
            integral: magnitude / scale,
            fraction: magnitude % scale,
            fraction_digits,
        }
    }
}

/// A float in EXI's form: `mantissa × 10^exponent`, base 10, not IEEE
/// 754.
///
/// Ranges: the mantissa is any `i64`; the exponent is `−(2^14 − 1) ..=
/// 2^14 − 1`. The exponent `−2^14` (`−16384`) marks the special values:
/// mantissa 1 is positive infinity, −1 negative infinity, anything else
/// NaN. Values written with an exponent outside the
/// range are `ErrorKind::Value`.
///
/// ```
/// use exi::Float;
/// assert!(Float::NAN.is_nan() && !Float::NAN.is_infinite());
/// assert!(Float::INFINITY.is_infinite() && Float::NEG_INFINITY.is_infinite());
/// assert!(Float { mantissa: 7, exponent: -16384 }.is_nan());    // any other mantissa: NaN
/// assert!(!Float { mantissa: 1, exponent: 16383 }.is_infinite()); // largest finite exponent
/// assert!(Float { mantissa: 0, exponent: 0 }.is_finite());
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Float {
    pub mantissa: i64,
    pub exponent: i16,
}

/// cbindgen:ignore
impl Float {
    pub const SPECIAL_EXPONENT: i16 = -16384;
    pub const MAX_EXPONENT: i16 = 16383;
    pub const MIN_EXPONENT: i16 = -16383;
    pub const INFINITY: Float = Float { mantissa: 1, exponent: -16384 };
    pub const NEG_INFINITY: Float = Float { mantissa: -1, exponent: -16384 };
    pub const NAN: Float = Float { mantissa: 0, exponent: -16384 };

    pub const fn is_nan(self) -> bool {
        self.exponent == Self::SPECIAL_EXPONENT
            && self.mantissa != 1
            && self.mantissa != -1
    }
    pub const fn is_infinite(self) -> bool {
        self.exponent == Self::SPECIAL_EXPONENT
            && (self.mantissa == 1 || self.mantissa == -1)
    }
    pub const fn is_finite(self) -> bool {
        self.exponent != Self::SPECIAL_EXPONENT
    }
}

/// The XML Schema date and time types EXI encodes as Date-Time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DateTimeKind {
    GYear,
    GYearMonth,
    Date,
    DateTime,
    GMonth,
    GMonthDay,
    GDay,
    Time,
}

/// A date, time or part of one. Fields the kind does not use are 0 and
/// ignored.
///
/// | Field | Range | Used by |
/// |---|---|---|
/// | `year` | any `i64` | `GYear`, `GYearMonth`, `Date`, `DateTime` |
/// | `month` | 1–12 | `GYearMonth`, `Date`, `DateTime`, `GMonth`, `GMonthDay` |
/// | `day` | 1–31 | `Date`, `DateTime`, `GMonthDay`, `GDay` |
/// | `hour`, `minute`, `second` | 0–24, 0–59, 0–59; hour 24 only with minute and second 0 | `DateTime`, `Time` |
/// | `fraction` | `(value, digits)`: the fraction of a second is `value / 10^digits`; `value < 10^digits`, digits 1–19 | `DateTime`, `Time` |
/// | `tz_minutes` | −840 to 840 (±14:00) | every kind |
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DateTime {
    pub kind: DateTimeKind,
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub fraction: Option<(u64, u8)>,
    pub tz_minutes: Option<i16>,
}

/// Facts about how a value was carried, reported in [`EventMeta`](crate::EventMeta).
///
/// | Flag | Set when |
/// |---|---|
/// | `UNTYPED` | the value was sent as a string where its schema type expected another datatype (non-strict grammars only) |
/// | `LEXICAL` | `Preserve.lexicalValues` is on: the value is a `Str` holding the original lexical form of a typed value |
///
/// ```
/// use exi::ValueFlags;
/// let f = ValueFlags::UNTYPED | ValueFlags::LEXICAL;
/// assert!(f.contains(ValueFlags::UNTYPED) && f.contains(ValueFlags::LEXICAL));
/// assert!(!ValueFlags::empty().contains(ValueFlags::UNTYPED));
/// assert_eq!(ValueFlags::empty().bits(), 0);
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ValueFlags(u8);

impl ValueFlags {
    pub const UNTYPED: ValueFlags = ValueFlags(1);
    pub const LEXICAL: ValueFlags = ValueFlags(2);
    pub const fn empty() -> Self {
        ValueFlags(0)
    }
    pub const fn bits(self) -> u8 {
        self.0
    }
    pub const fn contains(self, other: ValueFlags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for ValueFlags {
    type Output = ValueFlags;
    fn bitor(self, rhs: ValueFlags) -> ValueFlags {
        ValueFlags(self.0 | rhs.0)
    }
}

/// [`Value`] with owned data.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum OwnedValue {
    Str(Box<str>),
    Bool(bool),
    BoolDigit(bool),
    Int(i64),
    BigInt(Box<str>),
    Decimal(Decimal),
    BigDecimal(Box<str>),
    Float(Float),
    Binary(Box<[u8]>, BinaryKind),
    DateTime(DateTime),
    QName(OwnedName),
    List(Vec<OwnedValue>),
    Enum { ordinal: u32, text: Box<str> },
}

impl OwnedValue {
    pub fn as_value(&self) -> Value<'_> {
        match self {
            OwnedValue::Str(s) => Value::Str(s),
            OwnedValue::Bool(b) => Value::Bool(*b),
            OwnedValue::BoolDigit(b) => Value::BoolDigit(*b),
            OwnedValue::Int(i) => Value::Int(*i),
            OwnedValue::BigInt(s) => Value::BigInt(s),
            OwnedValue::Decimal(d) => Value::Decimal(*d),
            OwnedValue::BigDecimal(s) => Value::BigDecimal(s),
            OwnedValue::Float(f) => Value::Float(*f),
            OwnedValue::Binary(b, k) => Value::Binary(b, *k),
            OwnedValue::DateTime(d) => Value::DateTime(*d),
            OwnedValue::QName(n) => Value::QName(n.as_name()),
            OwnedValue::List(items) => Value::List(ListRef::Owned(items)),
            OwnedValue::Enum { ordinal, text } => {
                Value::Enum { ordinal: *ordinal, text }
            }
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Anything usable as a value in writer calls. Sealed. Implemented for
/// `&str`, `bool`, `i32`, `u32`, `i64`, [`Decimal`], [`Float`],
/// [`DateTime`], [`Value`] and `&OwnedValue`.
///
/// ```
/// use exi::{IntoValue, Value};
/// assert_eq!("hi".to_value(), Value::Str("hi"));
/// assert_eq!(7u32.to_value(), Value::Int(7));
/// assert_eq!(u32::MAX.to_value(), Value::Int(4_294_967_295));
/// assert_eq!(true.to_value(), Value::Bool(true));
/// ```
pub trait IntoValue: sealed::Sealed {
    fn to_value(&self) -> Value<'_>;
}

macro_rules! sealed_to_value {
    ($($t:ty => |$v:ident| $e:expr),* $(,)?) => { $(
        impl sealed::Sealed for $t {}
        impl IntoValue for $t {
            fn to_value(&self) -> Value<'_> {
                let $v = self;
                $e
            }
        }
    )* };
}
sealed_to_value!(
    &str => |v| Value::Str(v),
    bool => |v| Value::Bool(*v),
    i32 => |v| Value::Int(*v as i64),
    u32 => |v| Value::Int(*v as i64),
    i64 => |v| Value::Int(*v),
    Decimal => |v| Value::Decimal(*v),
    Float => |v| Value::Float(*v),
    DateTime => |v| Value::DateTime(*v),
    Value<'_> => |v| *v,
    &OwnedValue => |v| v.as_value(),
);

// ---------------------------------------------------------------------------------------
// Lexical forms
// ---------------------------------------------------------------------------------------

struct Cursor<'b> {
    buf: &'b mut [u8],
    len: usize,
}

impl Cursor<'_> {
    fn put(&mut self, s: &str) -> Result<()> {
        let end = self.len + s.len();
        if end > self.buf.len() {
            return Err(Error::new(
                ErrorKind::BufferFull,
                "lexical form longer than the buffer",
            ));
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
    fn put_fmt(&mut self, args: core::fmt::Arguments<'_>) -> Result<()> {
        use core::fmt::Write;
        let mut tmp = alloc::string::String::new();
        let _ = tmp.write_fmt(args);
        self.put(&tmp)
    }
}

fn bad(detail: &'static str) -> Error {
    Error::new(ErrorKind::Value, detail)
}

fn pow10(d: u8) -> Option<u64> {
    10u64.checked_pow(d as u32)
}

/// The fraction `v / 10^digits` as digits without trailing zeros (""
/// for zero).
fn fraction_digits(v: u64, digits: u8) -> alloc::string::String {
    let mut s = alloc::format!("{:0width$}", v, width = digits as usize);
    while s.ends_with('0') {
        s.pop();
    }
    s
}

const B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn write_value(v: &Value<'_>, c: &mut Cursor<'_>) -> Result<()> {
    match *v {
        Value::Str(s) | Value::BigInt(s) | Value::BigDecimal(s) => c.put(s),
        Value::Enum { text, .. } => c.put(text),
        Value::Bool(b) => c.put(if b { "true" } else { "false" }),
        Value::BoolDigit(b) => c.put(if b { "1" } else { "0" }),
        Value::Int(i) => c.put_fmt(format_args!("{i}")),
        Value::Decimal(d) => {
            if d.fraction_digits > 19
                || pow10(d.fraction_digits).is_some_and(|p| d.fraction >= p)
            {
                return Err(bad(
                    "decimal fraction has more digits than fraction_digits",
                ));
            }
            let mut f = fraction_digits(d.fraction, d.fraction_digits);
            if f.is_empty() {
                f.push('0');
            }
            let sign = if d.negative && (d.integral != 0 || d.fraction != 0) {
                "-"
            } else {
                ""
            };
            c.put_fmt(format_args!("{sign}{}.{f}", d.integral))
        }
        Value::Float(f) => {
            if f.is_nan() {
                return c.put("NaN");
            }
            if f.is_infinite() {
                return c.put(if f.mantissa > 0 { "INF" } else { "-INF" });
            }
            if !(Float::MIN_EXPONENT..=Float::MAX_EXPONENT)
                .contains(&f.exponent)
            {
                return Err(bad("float exponent out of range"));
            }
            if f.mantissa == 0 {
                return c.put("0.0E0");
            }
            let mut digits = alloc::format!("{}", f.mantissa.unsigned_abs());
            let mut exp = f.exponent as i64;
            while digits.ends_with('0') {
                digits.pop();
                exp += 1;
            }
            let e = exp + digits.len() as i64 - 1;
            let (first, rest) = digits.split_at(1);
            let rest = if rest.is_empty() { "0" } else { rest };
            let sign = if f.mantissa < 0 { "-" } else { "" };
            c.put_fmt(format_args!("{sign}{first}.{rest}E{e}"))
        }
        Value::Binary(data, BinaryKind::Hex) => {
            for b in data {
                c.put_fmt(format_args!("{b:02X}"))?;
            }
            Ok(())
        }
        Value::Binary(data, BinaryKind::Base64) => {
            for chunk in data.chunks(3) {
                let n = (chunk[0] as u32) << 16
                    | (*chunk.get(1).unwrap_or(&0) as u32) << 8
                    | *chunk.get(2).unwrap_or(&0) as u32;
                let mut out = [b'='; 4];
                for (i, o) in out.iter_mut().enumerate().take(chunk.len() + 1)
                {
                    *o = B64[((n >> (18 - 6 * i)) & 63) as usize];
                }
                c.put(core::str::from_utf8(&out).expect("ASCII"))?;
            }
            Ok(())
        }
        Value::DateTime(d) => write_datetime(&d, c),
        Value::QName(n) => match n.prefix() {
            Some(p) if !p.is_empty() => {
                c.put_fmt(format_args!("{p}:{}", n.local_name()))
            }
            _ => c.put(n.local_name()),
        },
        Value::List(items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    c.put(" ")?;
                }
                write_value(&item, c)?;
            }
            Ok(())
        }
    }
}

fn write_datetime(d: &DateTime, c: &mut Cursor<'_>) -> Result<()> {
    use DateTimeKind::*;
    let (year, month, day, time) = match d.kind {
        GYear => (true, false, false, false),
        GYearMonth => (true, true, false, false),
        Date => (true, true, true, false),
        DateTime => (true, true, true, true),
        GMonth => (false, true, false, false),
        GMonthDay => (false, true, true, false),
        GDay => (false, false, true, false),
        Time => (false, false, false, true),
    };
    let out_of_range = (month && !(1..=12).contains(&d.month))
        || (day && !(1..=31).contains(&d.day))
        || (time && (d.hour > 24 || d.minute > 59 || d.second > 59))
        || d.tz_minutes.is_some_and(|tz| !(-840..=840).contains(&tz))
        || d.fraction.is_some_and(|(v, n)| {
            n == 0 || n > 19 || pow10(n).is_some_and(|p| v >= p)
        });
    if out_of_range {
        return Err(bad("date-time field out of range"));
    }
    if year {
        let sign = if d.year < 0 { "-" } else { "" };
        c.put_fmt(format_args!("{sign}{:04}", d.year.unsigned_abs()))?;
    }
    match d.kind {
        GYearMonth => c.put_fmt(format_args!("-{:02}", d.month))?,
        Date | DateTime => {
            c.put_fmt(format_args!("-{:02}-{:02}", d.month, d.day))?
        }
        GMonth => c.put_fmt(format_args!("--{:02}", d.month))?,
        GMonthDay => {
            c.put_fmt(format_args!("--{:02}-{:02}", d.month, d.day))?
        }
        GDay => c.put_fmt(format_args!("---{:02}", d.day))?,
        _ => {}
    }
    if time {
        if d.kind == DateTime {
            c.put("T")?;
        }
        c.put_fmt(format_args!(
            "{:02}:{:02}:{:02}",
            d.hour, d.minute, d.second
        ))?;
        if let Some((v, n)) = d.fraction {
            let f = fraction_digits(v, n);
            if !f.is_empty() {
                c.put_fmt(format_args!(".{f}"))?;
            }
        }
    }
    match d.tz_minutes {
        None => Ok(()),
        Some(0) => c.put("Z"),
        Some(tz) => {
            let sign = if tz < 0 { '-' } else { '+' };
            let a = tz.unsigned_abs();
            c.put_fmt(format_args!("{sign}{:02}:{:02}", a / 60, a % 60))
        }
    }
}
