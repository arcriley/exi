// src/coding/datatypes.rs
//
// EXI datatype encoding and decoding
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Each function is the complete rule for its datatype. Examples use
//! byte-aligned coding, where each n-bit value is whole octets (see
//! [`crate::coding`]).
//!
//! | Datatype | Representation |
//! |---|---|
//! | Unsigned Integer | 7 value bits per octet, least significant group first; the high bit of each octet is 1 when another octet follows. Each octet is an 8-bit n-bit value, so in bit-packed coding it is 8 bits at any bit position |
//! | Integer (unbounded) | a Boolean sign (1 = negative), then an Unsigned Integer magnitude; for a negative value the magnitude stored is `−value − 1` |
//! | Integer (bounded range of 4096 values or fewer) | an n-bit value `value − min`, `n = width(max − min + 1)` |
//! | Boolean | 1 bit (1 = true); 2 bits when the type has a pattern facet: 0 `false`, 1 `0`, 2 `true`, 3 `1` |
//! | Decimal | a Boolean sign, the integral part as an Unsigned Integer, then the fraction's digits **reversed** as an Unsigned Integer (3.05 → fraction digits `05` → reversed `50` → 50), so leading zeros survive; trailing zeros of the fraction are lost (3.50 reads back as 3.5) |
//! | Float | the mantissa as an Integer, then the exponent as an Integer |
//! | Binary | the length in octets as an Unsigned Integer, then the octets |
//! | String | the length in characters as an Unsigned Integer, then each character as an Unsigned Integer code point; or, for a restricted character set, each character as an n-bit index (below) |
//! | Date-Time | the components of its kind (below) |
//! | List | the number of items as an Unsigned Integer, then each item |
//! | Enumeration | an n-bit ordinal, `n = width(number of values)`, in the type's value order |
//!
//! # Date-time components
//!
//! | Component | Representation |
//! |---|---|
//! | Year | Integer, the year minus 2000 |
//! | MonthDay | 9-bit `month × 32 + day` (month 0 for `GDay`, day 0 for `GYearMonth` and `GMonth`) |
//! | Time | 17-bit `((hour × 64) + minute) × 64 + second` |
//! | FractionalSecs | a presence Boolean, then the fraction's digits reversed as an Unsigned Integer |
//! | TimeZone | a presence Boolean, then 11-bit `hours × 64 + minutes + 896`, where the offset is `hours:minutes` and both carry its sign |
//!
//! | Kind | Components, in order |
//! |---|---|
//! | `GYear` | Year, TimeZone |
//! | `GYearMonth`, `Date` | Year, MonthDay, TimeZone |
//! | `DateTime` | Year, MonthDay, Time, FractionalSecs, TimeZone |
//! | `GMonth`, `GMonthDay`, `GDay` | MonthDay, TimeZone |
//! | `Time` | Time, FractionalSecs, TimeZone |
//!
//! # Restricted character sets
//!
//! A string type whose pattern allows fewer than 256 characters uses
//! that set, sorted by code point, with `N` members. Each character is
//! an n-bit value, `n = width(N + 1)`: its index in the set, or `N`
//! followed by the character's code point as an Unsigned Integer
//! when it is outside the set.
//!
//! # Sizes readers must accept
//!
//! Readers accept Unsigned Integers of up to
//! `Limits::max_integer_octets` octets, and more octets than a value
//! needs (`80 00` is 0). Readers must handle every value below 2^31;
//! libexi handles every value of 64 bits, and larger Integers as
//! decimal digits (`read_uint_big`, `read_int_big`). Writers always
//! write the fewest octets.

use alloc::string::String;

use crate::coding::width;
use crate::coding::{Channel, Decoder, Encoder};
use crate::value::{DateTime, DateTimeKind, Decimal, Float};
use crate::{Error, ErrorKind, Result};
use alloc::vec::Vec;

/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_uint};
/// let enc = |v: u64| { let mut e = ByteEncoder::new(Vec::new()); write_uint(&mut e, S, v).unwrap(); e.into_sink() };
/// assert_eq!(enc(0), [0x00]);
/// assert_eq!(enc(127), [0x7F]);
/// assert_eq!(enc(128), [0x80, 0x01]);
/// assert_eq!(enc(300), [0xAC, 0x02]);
/// assert_eq!(enc(u64::MAX), [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
/// ```
pub fn write_uint<E: Encoder>(e: &mut E, ch: Channel, v: u64) -> Result<()> {
    let mut v = v;
    loop {
        let group = (v & 0x7F) as u32;
        v >>= 7;
        if v == 0 {
            return e.nbit(ch, group, 8);
        }
        e.nbit(ch, group | 0x80, 8)?;
    }
}

/// # Errors
///
/// `ErrorKind::Limit` when the value takes more than `max_octets`
/// octets; `ErrorKind::Value` when it exceeds `u64` (read it with
/// [`read_uint_big`]); `ErrorKind::Truncated` when input ends.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_uint};
/// use exi::ErrorKind;
/// let dec = |b: &[u8], max| read_uint(&mut ByteDecoder::new(b, &[]), S, max);
/// assert_eq!(dec(&[0xAC, 0x02], 64)?, 300);
/// assert_eq!(dec(&[0x80, 0x00], 64)?, 0);                          // more octets than needed
/// assert_eq!(dec(&[0xFF; 9].iter().copied().chain([0x01]).collect::<Vec<_>>(), 64)?, u64::MAX);
/// assert_eq!(dec(&[0xFF; 9].iter().copied().chain([0x02]).collect::<Vec<_>>(), 64).unwrap_err().kind(), ErrorKind::Value);
/// assert_eq!(dec(&[0x80, 0x80, 0x01], 2).unwrap_err().kind(), ErrorKind::Limit);
/// assert_eq!(dec(&[0x80, 0x80, 0x01], 3)?, 16_384);                // exactly the limit
/// assert_eq!(dec(&[0x80], 64).unwrap_err().kind(), ErrorKind::Truncated);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_uint<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
) -> Result<u64> {
    let mut v: u64 = 0;
    let mut shift: u32 = 0;
    let mut n: u32 = 0;
    loop {
        if n >= max_octets {
            return Err(err(
                ErrorKind::Limit,
                "unsigned integer longer than max_integer_octets",
            ));
        }
        let b = d.nbit(ch, 8)?;
        n += 1;
        let group = (b & 0x7F) as u64;
        if group != 0 {
            if shift >= 64 || (shift > 57 && group >> (64 - shift) != 0) {
                return Err(err(
                    ErrorKind::Value,
                    "unsigned integer exceeds 64 bits",
                ));
            }
            v |= group << shift;
        }
        if b & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
    }
}

/// Reads an Unsigned Integer of any size up to `max_octets` and appends
/// its decimal digits to `out`, without leading zeros ("0" for zero).
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_uint_big};
/// let bytes: Vec<u8> = [0xFF; 9].iter().copied().chain([0x02]).collect();   // 2^64 + 2^63 − 1
/// let mut out = String::new();
/// read_uint_big(&mut ByteDecoder::new(&bytes, &[]), S, 64, &mut out)?;
/// assert_eq!(out, "27670116110564327423");
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_uint_big<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
    out: &mut String,
) -> Result<()> {
    let mut groups = Vec::new();
    read_uint_groups(d, ch, max_octets, &mut groups)?;
    groups_to_decimal(&groups, out);
    Ok(())
}

/// Writes an Unsigned Integer given as decimal digits, for values
/// beyond `u64`.
///
/// # Errors
///
/// `ErrorKind::Value` when `digits` is empty or holds anything but
/// ASCII digits.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// let mut e = ByteEncoder::new(Vec::new());
/// write_uint_digits(&mut e, S, "27670116110564327423")?;
/// let bytes = e.into_sink();
/// let mut out = String::new();
/// read_uint_big(&mut ByteDecoder::new(&bytes, &[]), S, 64, &mut out)?;
/// assert_eq!(out, "27670116110564327423");
/// assert_eq!(write_uint_digits(&mut ByteEncoder::new(Vec::new()), S, "").unwrap_err().kind(), exi::ErrorKind::Value);
/// assert_eq!(write_uint_digits(&mut ByteEncoder::new(Vec::new()), S, "12a").unwrap_err().kind(), exi::ErrorKind::Value);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_uint_digits<E: Encoder>(
    e: &mut E,
    ch: Channel,
    digits: &str,
) -> Result<()> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err(ErrorKind::Value, "not a string of decimal digits"));
    }
    // Repeated division by 128 gives the groups, least significant
    // first.
    let mut num: Vec<u8> = digits.bytes().map(|b| b - b'0').collect();
    let mut groups = Vec::new();
    loop {
        let mut rem: u32 = 0;
        let mut next = Vec::with_capacity(num.len());
        for &dgt in &num {
            let cur = rem * 10 + dgt as u32;
            let q = cur / 128;
            rem = cur % 128;
            if !next.is_empty() || q != 0 {
                next.push(q as u8);
            }
        }
        groups.push(rem as u8);
        if next.is_empty() {
            break;
        }
        num = next;
    }
    let last = groups.len() - 1;
    for (i, g) in groups.iter().enumerate() {
        let more = if i < last { 0x80 } else { 0 };
        e.nbit(ch, (*g as u32) | more, 8)?;
    }
    Ok(())
}

/// An unbounded Integer.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_int};
/// let enc = |v: i64| { let mut e = ByteEncoder::new(Vec::new()); write_int(&mut e, S, v).unwrap(); e.into_sink() };
/// assert_eq!(enc(0), [0x00, 0x00]);
/// assert_eq!(enc(5), [0x00, 0x05]);
/// assert_eq!(enc(-1), [0x01, 0x00]);                  // magnitude −(−1) − 1 = 0
/// assert_eq!(enc(i64::MAX), [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
/// assert_eq!(enc(i64::MIN), [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
/// ```
pub fn write_int<E: Encoder>(e: &mut E, ch: Channel, v: i64) -> Result<()> {
    if v < 0 {
        write_bool(e, ch, true)?;
        write_uint(e, ch, !v as u64)
    } else {
        write_bool(e, ch, false)?;
        write_uint(e, ch, v as u64)
    }
}

/// # Errors
///
/// As [`read_uint`]; `ErrorKind::Value` when the value is outside `i64`
/// (read it with [`read_int_big`]).
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_int};
/// let dec = |b: &[u8]| read_int(&mut ByteDecoder::new(b, &[]), S, 64);
/// assert_eq!(dec(&[0x01, 0x00])?, -1);
/// assert_eq!(dec(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F])?, i64::MIN);
/// assert_eq!(dec(&[0x00, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).unwrap_err().kind(),
///            exi::ErrorKind::Value);                    // 2^63 does not fit
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_int<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
) -> Result<i64> {
    let negative = read_bool(d, ch)?;
    let m = read_uint(d, ch, max_octets)?;
    if m > i64::MAX as u64 {
        return Err(err(ErrorKind::Value, "integer outside i64"));
    }
    Ok(if negative { !(m as i64) } else { m as i64 })
}

/// Reads an Integer of any size and appends `-` (when negative) and its
/// decimal digits.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_int_big};
/// let mut out = String::new();
/// // Sign negative (0x01), then magnitude 2^64 + 2^63 − 1 → value −(2^64 + 2^63)
/// let mut bytes = vec![0x01];
/// bytes.extend([0xFF; 9]);
/// bytes.push(0x02);
/// read_int_big(&mut ByteDecoder::new(&bytes, &[]), S, 64, &mut out)?;
/// assert_eq!(out, "-27670116110564327424");
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_int_big<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
    out: &mut String,
) -> Result<()> {
    let negative = read_bool(d, ch)?;
    let mut digits = String::new();
    read_uint_big(d, ch, max_octets, &mut digits)?;
    if negative {
        decimal_add_one(&mut digits);
        out.push('-');
    }
    out.push_str(&digits);
    Ok(())
}

fn bounded_width(min: i64, max: i64) -> Result<(i128, u8)> {
    let range = max as i128 - min as i128 + 1;
    if !(1..=4096).contains(&range) {
        return Err(err(
            ErrorKind::Internal,
            "bounded integer range must hold 1 to 4096 values",
        ));
    }
    Ok((range, width(range as u32)))
}

/// An Integer whose type has `max − min + 1 ≤ 4096` values.
///
/// # Errors
///
/// `ErrorKind::Value` when `v` is outside `min..=max`;
/// `ErrorKind::Internal` when the range has more than 4096 values.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_bounded};
/// let enc = |v, min, max| { let mut e = ByteEncoder::new(Vec::new()); write_bounded(&mut e, S, v, min, max).map(|_| e.into_sink()) };
/// assert_eq!(enc(37, 0, 100)?, [37]);                          // 101 values: 7 bits
/// assert_eq!(enc(-10, -10, 10)?, [0]);                         // offset from min
/// assert_eq!(enc(5, 5, 5)?, Vec::<u8>::new());                // one value: 0 bits
/// assert_eq!(enc(4095, 0, 4095)?, [0xFF, 0x0F]);              // 4096 values: 12 bits
/// assert_eq!(enc(101, 0, 100).unwrap_err().kind(), exi::ErrorKind::Value);
/// assert_eq!(enc(0, 0, 4096).unwrap_err().kind(), exi::ErrorKind::Internal);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_bounded<E: Encoder>(
    e: &mut E,
    ch: Channel,
    v: i64,
    min: i64,
    max: i64,
) -> Result<()> {
    let (_, w) = bounded_width(min, max)?;
    if v < min || v > max {
        return Err(err(ErrorKind::Value, "integer outside its type's range"));
    }
    e.nbit(ch, (v as i128 - min as i128) as u32, w)
}

/// # Errors
///
/// `ErrorKind::Value` when the n-bit value exceeds `max − min`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_bounded};
/// let dec = |b: &[u8], min, max| read_bounded(&mut ByteDecoder::new(b, &[]), S, min, max);
/// assert_eq!(dec(&[37], 0, 100)?, 37);
/// assert_eq!(dec(&[], 5, 5)?, 5);
/// assert_eq!(dec(&[101], 0, 100).unwrap_err().kind(), exi::ErrorKind::Value);
/// assert_eq!(dec(&[0], 0, 4096).unwrap_err().kind(), exi::ErrorKind::Internal);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_bounded<D: Decoder>(
    d: &mut D,
    ch: Channel,
    min: i64,
    max: i64,
) -> Result<i64> {
    let (range, w) = bounded_width(min, max)?;
    let x = d.nbit(ch, w)? as i128;
    if x >= range {
        return Err(err(ErrorKind::Value, "bounded integer beyond its range"));
    }
    Ok((min as i128 + x) as i64)
}

/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// let mut e = ByteEncoder::new(Vec::new());
/// write_bool(&mut e, S, false)?;
/// write_bool(&mut e, S, true)?;
/// let bytes = e.into_sink();
/// assert_eq!(bytes, [0, 1]);
/// let mut d = ByteDecoder::new(&bytes, &[]);
/// assert!(!read_bool(&mut d, S)?);
/// assert!(read_bool(&mut d, S)?);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_bool<E: Encoder>(e: &mut E, ch: Channel, v: bool) -> Result<()> {
    e.nbit(ch, v as u32, 1)
}

/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_bool};
/// let mut d = ByteDecoder::new(&[0, 1], &[]);
/// assert_eq!((read_bool(&mut d, S)?, read_bool(&mut d, S)?), (false, true));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_bool<D: Decoder>(d: &mut D, ch: Channel) -> Result<bool> {
    Ok(d.nbit(ch, 1)? == 1)
}

/// A Boolean of a type with a pattern facet: `digit` selects the forms
/// `0`/`1` over `false`/`true`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// let mut e = ByteEncoder::new(Vec::new());
/// for (v, digit) in [(false, false), (false, true), (true, false), (true, true)] {
///     write_bool_pattern(&mut e, S, v, digit)?;
/// }
/// let bytes = e.into_sink();
/// assert_eq!(bytes, [0, 1, 2, 3]);
/// let mut d = ByteDecoder::new(&bytes, &[]);
/// assert_eq!(read_bool_pattern(&mut d, S)?, (false, false));
/// assert_eq!(read_bool_pattern(&mut d, S)?, (false, true));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_bool_pattern<E: Encoder>(
    e: &mut E,
    ch: Channel,
    v: bool,
    digit: bool,
) -> Result<()> {
    e.nbit(ch, (v as u32) * 2 + digit as u32, 2)
}

/// Returns `(value, digit)`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_bool_pattern};
/// let mut d = ByteDecoder::new(&[2, 3], &[]);
/// assert_eq!(read_bool_pattern(&mut d, S)?, (true, false));
/// assert_eq!(read_bool_pattern(&mut d, S)?, (true, true));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_bool_pattern<D: Decoder>(
    d: &mut D,
    ch: Channel,
) -> Result<(bool, bool)> {
    let x = d.nbit(ch, 2)?;
    Ok((x >= 2, x % 2 == 1))
}

/// # Errors
///
/// `ErrorKind::Value` when `fraction ≥ 10^fraction_digits` or
/// `fraction_digits > 19`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// use exi::Decimal;
/// let enc = |d| { let mut e = ByteEncoder::new(Vec::new()); write_decimal(&mut e, S, &d).map(|_| e.into_sink()) };
/// assert_eq!(enc(Decimal::new(false, 3, 5, 2))?, [0x00, 0x03, 0x32]);   // fraction 05 → 50
/// assert_eq!(enc(Decimal::new(true, 0, 5, 1))?, [0x01, 0x00, 0x05]);
/// assert_eq!(enc(Decimal::new(false, 7, 0, 0))?, [0x00, 0x07, 0x00]);
/// assert_eq!(enc(Decimal::new(false, 1, 10, 1)).unwrap_err().kind(), exi::ErrorKind::Value);
/// let back = read_decimal(&mut ByteDecoder::new(&[0x00, 0x03, 0x32], &[]), S, 64)?;
/// assert_eq!(back, Decimal::new(false, 3, 5, 2));
/// let trailing = read_decimal(&mut ByteDecoder::new(&[0x00, 0x03, 0x05], &[]), S, 64)?;
/// assert_eq!(trailing, Decimal::new(false, 3, 5, 1));               // 3.50 was sent as 3.5
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_decimal<E: Encoder>(
    e: &mut E,
    ch: Channel,
    d: &Decimal,
) -> Result<()> {
    if d.fraction_digits > 19
        || (d.fraction_digits < 19
            && d.fraction >= 10u64.pow(d.fraction_digits as u32))
    {
        return Err(err(
            ErrorKind::Value,
            "decimal fraction has more digits than fraction_digits",
        ));
    }
    write_bool(e, ch, d.negative)?;
    write_uint(e, ch, d.integral)?;
    write_uint(e, ch, reverse_digits(d.fraction, d.fraction_digits))
}

/// # Errors
///
/// As [`read_uint`]; `ErrorKind::Unsupported` when a part exceeds `u64`
/// or the fraction has more than 19 digits (big decimals come later).
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_decimal};
/// use exi::{Decimal, ErrorKind};
/// assert_eq!(read_decimal(&mut ByteDecoder::new(&[0x01, 0x00, 0x05], &[]), S, 64)?, Decimal::new(true, 0, 5, 1));
/// let mut too_big = vec![0x00];
/// too_big.extend([0xFF; 9]);
/// too_big.push(0x02);
/// assert_eq!(read_decimal(&mut ByteDecoder::new(&too_big, &[]), S, 64).unwrap_err().kind(), ErrorKind::Unsupported);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_decimal<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
) -> Result<Decimal> {
    let big = |e: Error| {
        if e.kind() == ErrorKind::Value {
            err(ErrorKind::Unsupported, "decimal part beyond 64 bits")
        } else {
            e
        }
    };
    let negative = read_bool(d, ch)?;
    let integral = read_uint(d, ch, max_octets).map_err(big)?;
    let rev = read_uint(d, ch, max_octets).map_err(big)?;
    let (fraction, fraction_digits) = unreverse(rev).ok_or_else(|| {
        err(ErrorKind::Unsupported, "decimal fraction beyond 19 digits")
    })?;
    Ok(Decimal { negative, integral, fraction, fraction_digits })
}

/// # Errors
///
/// `ErrorKind::Value` when the exponent is outside `−16383..=16383` and
/// the value is not a special value.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_float};
/// use exi::Float;
/// let enc = |f| { let mut e = ByteEncoder::new(Vec::new()); write_float(&mut e, S, f).map(|_| e.into_sink()) };
/// assert_eq!(enc(Float { mantissa: 2153, exponent: -2 })?, [0x00, 0xE9, 0x10, 0x01, 0x01]);
/// assert_eq!(enc(Float::INFINITY)?, [0x00, 0x01, 0x01, 0xFF, 0x7F]);    // exponent −16384
/// assert_eq!(enc(Float { mantissa: 1, exponent: 16383 })?.len(), 5);
/// assert_eq!(enc(Float { mantissa: 1, exponent: i16::MAX }).unwrap_err().kind(), exi::ErrorKind::Value);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_float<E: Encoder>(
    e: &mut E,
    ch: Channel,
    f: Float,
) -> Result<()> {
    if f.exponent != Float::SPECIAL_EXPONENT
        && !(Float::MIN_EXPONENT..=Float::MAX_EXPONENT).contains(&f.exponent)
    {
        return Err(err(ErrorKind::Value, "float exponent out of range"));
    }
    write_int(e, ch, f.mantissa)?;
    write_int(e, ch, f.exponent as i64)
}

/// # Errors
///
/// `ErrorKind::Value` when the mantissa exceeds `i64` or the exponent
/// is outside `−16384..=16383`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// use exi::{ErrorKind, Float};
/// let mut e = ByteEncoder::new(Vec::new());
/// write_float(&mut e, S, Float::INFINITY)?;
/// let bytes = e.into_sink();
/// assert_eq!(read_float(&mut ByteDecoder::new(&bytes, &[]), S, 64)?, Float::INFINITY);
/// // Exponent 16384: sign 0, magnitude 16384 (0x80, 0x80, 0x01).
/// assert_eq!(read_float(&mut ByteDecoder::new(&[0x00, 0x01, 0x00, 0x80, 0x80, 0x01], &[]), S, 64).unwrap_err().kind(), ErrorKind::Value);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_float<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
) -> Result<Float> {
    let mantissa = read_int(d, ch, max_octets)?;
    let exponent = read_int(d, ch, max_octets)?;
    if !(Float::SPECIAL_EXPONENT as i64..=Float::MAX_EXPONENT as i64)
        .contains(&exponent)
    {
        return Err(err(ErrorKind::Value, "float exponent out of range"));
    }
    Ok(Float { mantissa, exponent: exponent as i16 })
}

/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_binary};
/// let mut e = ByteEncoder::new(Vec::new());
/// write_binary(&mut e, S, b"hi")?;
/// write_binary(&mut e, S, b"")?;
/// assert_eq!(e.into_sink(), [0x02, b'h', b'i', 0x00]);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_binary<E: Encoder>(
    e: &mut E,
    ch: Channel,
    data: &[u8],
) -> Result<()> {
    write_uint(e, ch, data.len() as u64)?;
    e.octets(ch, data)
}

/// Reads a binary value's length. The caller then reads that many
/// octets with [`Decoder::octets`], in pieces if needed.
///
/// # Errors
///
/// `ErrorKind::Limit` when the length exceeds `max_bytes`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Decoder, byte_aligned::ByteDecoder, datatypes::read_binary_len};
/// let mut d = ByteDecoder::new(&[0x02, b'h', b'i'], &[]);
/// let n = read_binary_len(&mut d, S, 64, 10)?;
/// let mut buf = vec![0u8; n as usize];
/// d.octets(S, &mut buf)?;
/// assert_eq!(buf, b"hi");
/// assert_eq!(read_binary_len(&mut ByteDecoder::new(&[0x0B], &[]), S, 64, 10).unwrap_err().kind(), exi::ErrorKind::Limit);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_binary_len<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
    max_bytes: u64,
) -> Result<u64> {
    let n = read_uint(d, ch, max_octets)?;
    if n > max_bytes {
        return Err(err(
            ErrorKind::Limit,
            "binary value longer than max_binary_bytes",
        ));
    }
    Ok(n)
}

/// # Errors
///
/// `ErrorKind::Value` when a field is out of the range [`DateTime`]
/// documents.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_datetime};
/// use exi::{DateTime, DateTimeKind};
/// let base = DateTime { kind: DateTimeKind::Date, year: 2026, month: 11, day: 21, hour: 0,
///                       minute: 0, second: 0, fraction: None, tz_minutes: None };
/// let enc = |d| { let mut e = ByteEncoder::new(Vec::new()); write_datetime(&mut e, S, &d).map(|_| e.into_sink()) };
/// assert_eq!(enc(base)?, [0x00, 0x1A, 0x75, 0x01, 0x00]);           // 26, 11×32+21, no zone
/// let dt = DateTime { kind: DateTimeKind::DateTime, hour: 22, minute: 35, tz_minutes: Some(0), ..base };
/// assert_eq!(enc(dt)?, [0x00, 0x1A, 0x75, 0x01, 0xC0, 0x68, 0x01, 0x00, 0x01, 0x80, 0x03]);
/// let y = DateTime { kind: DateTimeKind::GYear, year: 1999, ..base };
/// assert_eq!(enc(y)?, [0x01, 0x00, 0x00]);                          // year −1 from 2000
/// let bad = DateTime { month: 13, ..base };
/// assert_eq!(enc(bad).unwrap_err().kind(), exi::ErrorKind::Value);
/// let far = DateTime { tz_minutes: Some(841), ..base };              // beyond +14:00
/// assert_eq!(enc(far).unwrap_err().kind(), exi::ErrorKind::Value);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_datetime<E: Encoder>(
    e: &mut E,
    ch: Channel,
    d: &DateTime,
) -> Result<()> {
    use DateTimeKind::*;
    let bad = || err(ErrorKind::Value, "date-time field out of range");
    let (has_year, has_month, has_day, has_time) = match d.kind {
        GYear => (true, false, false, false),
        GYearMonth => (true, true, false, false),
        Date => (true, true, true, false),
        DateTime => (true, true, true, true),
        GMonth => (false, true, false, false),
        GMonthDay => (false, true, true, false),
        GDay => (false, false, true, false),
        Time => (false, false, false, true),
    };
    if (has_month && !(1..=12).contains(&d.month))
        || (has_day && !(1..=31).contains(&d.day))
    {
        return Err(bad());
    }
    if has_time {
        if d.hour > 24 || d.minute > 59 || d.second > 59 {
            return Err(bad());
        }
        if d.hour == 24
            && (d.minute != 0
                || d.second != 0
                || d.fraction.is_some_and(|(v, _)| v != 0))
        {
            return Err(bad());
        }
        if let Some((v, digits)) = d.fraction {
            if !(1..=19).contains(&digits)
                || (digits < 19 && v >= 10u64.pow(digits as u32))
            {
                return Err(bad());
            }
        }
    }
    if d.tz_minutes.is_some_and(|tz| !(-840..=840).contains(&tz)) {
        return Err(bad());
    }
    if has_year {
        write_int(e, ch, d.year.checked_sub(2000).ok_or_else(bad)?)?;
    }
    if has_month || has_day {
        let month = if has_month { d.month as u32 } else { 0 };
        // gYearMonth and gMonth have no day, but their MonthDay
        // component has room for one: values read from text carry 0;
        // Canonical EXI under utcTime carries the day its conversion
        // gives, as EXIficient writes it.
        let day = if has_day
            || (matches!(d.kind, GYearMonth | GMonth) && d.day <= 31)
        {
            d.day as u32
        } else {
            0
        };
        e.nbit(ch, month * 32 + day, 9)?;
    }
    if has_time {
        e.nbit(
            ch,
            ((d.hour as u32 * 64) + d.minute as u32) * 64 + d.second as u32,
            17,
        )?;
        match d.fraction {
            Some((v, digits)) => {
                write_bool(e, ch, true)?;
                write_uint(e, ch, reverse_digits(v, digits))?;
            }
            None => write_bool(e, ch, false)?,
        }
    }
    match d.tz_minutes {
        Some(tz) => {
            write_bool(e, ch, true)?;
            let (h, m) = (tz as i32 / 60, tz as i32 % 60);
            e.nbit(ch, (h * 64 + m + 896) as u32, 11)
        }
        None => write_bool(e, ch, false),
    }
}

/// # Errors
///
/// `ErrorKind::Value` for a component out of range (month above 12,
/// minutes above 59, a time zone beyond ±14:00).
///
/// Round trips, including a negative time zone, hour 24, and a fraction
/// whose trailing zero is lost on the wire (.050 reads back as .05):
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// use exi::{DateTime, DateTimeKind::*};
/// let z = DateTime { kind: DateTime, year: 1, month: 2, day: 29, hour: 24, minute: 0, second: 0,
///                    fraction: None, tz_minutes: Some(-330) };
/// let t = DateTime { kind: Time, year: 0, month: 0, day: 0, hour: 23, minute: 59, second: 59,
///                    fraction: Some((5, 3)), tz_minutes: Some(840) };
/// let g = DateTime { kind: GDay, year: 0, month: 0, day: 31, hour: 0, minute: 0, second: 0,
///                    fraction: None, tz_minutes: None };
/// for v in [z, t, g] {
///     let mut e = ByteEncoder::new(Vec::new());
///     write_datetime(&mut e, S, &v)?;
///     let bytes = e.into_sink();
///     assert_eq!(read_datetime(&mut ByteDecoder::new(&bytes, &[]), S, v.kind, 64)?, v);
/// }
/// let lossy = DateTime { fraction: Some((50, 3)), ..t };
/// let mut e = ByteEncoder::new(Vec::new());
/// write_datetime(&mut e, S, &lossy)?;
/// let bytes = e.into_sink();
/// assert_eq!(read_datetime(&mut ByteDecoder::new(&bytes, &[]), S, Time, 64)?.fraction, Some((5, 2)));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_datetime<D: Decoder>(
    d: &mut D,
    ch: Channel,
    kind: DateTimeKind,
    max_octets: u32,
) -> Result<DateTime> {
    use DateTimeKind::*;
    let bad = || err(ErrorKind::Value, "date-time component out of range");
    let (has_year, has_monthday, has_time) = match kind {
        GYear => (true, false, false),
        GYearMonth | Date | DateTime => (true, true, kind == DateTime),
        GMonth | GMonthDay | GDay => (false, true, false),
        Time => (false, false, true),
    };
    let mut out = crate::value::DateTime {
        kind,
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        fraction: None,
        tz_minutes: None,
    };
    if has_year {
        out.year =
            read_int(d, ch, max_octets)?.checked_add(2000).ok_or_else(bad)?;
    }
    if has_monthday {
        let md = d.nbit(ch, 9)?;
        out.month = (md / 32) as u8;
        out.day = (md % 32) as u8;
        if out.month > 12 {
            return Err(bad());
        }
    }
    if has_time {
        let t = d.nbit(ch, 17)?;
        out.second = (t % 64) as u8;
        out.minute = ((t / 64) % 64) as u8;
        out.hour = (t / 4096) as u8;
        if out.hour > 24 || out.minute > 59 || out.second > 59 {
            return Err(bad());
        }
        if read_bool(d, ch)? {
            let rev = read_uint(d, ch, max_octets)?;
            out.fraction = Some(match unreverse(rev).ok_or_else(bad)? {
                (_, 0) => (0, 1),
                other => other,
            });
        }
    }
    if read_bool(d, ch)? {
        let t = d.nbit(ch, 11)? as i32 - 896;
        let (h, m) = (t / 64, t % 64);
        if m.abs() > 59 || (h * 60 + m).abs() > 840 {
            return Err(bad());
        }
        out.tz_minutes = Some((h * 60 + m) as i16);
    }
    Ok(out)
}

/// Writes the characters of `s` (no length) as code points.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_chars};
/// let mut e = ByteEncoder::new(Vec::new());
/// write_chars(&mut e, S, "aé")?;
/// assert_eq!(e.into_sink(), [0x61, 0xE9, 0x01]);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_chars<E: Encoder>(e: &mut E, ch: Channel, s: &str) -> Result<()> {
    // An aligned ASCII run is already in wire form. An unaligned bit
    // encoder would need a shifted copy of the whole run.
    if !s.is_empty() && e.bit_position() % 8 == 0 && s.is_ascii() {
        return e.octets(ch, s.as_bytes());
    }
    for c in s.chars() {
        write_uint(e, ch, c as u64)?;
    }
    Ok(())
}

/// Writes a String: its length in characters, then its characters.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_string};
/// let enc = |s: &str| { let mut e = ByteEncoder::new(Vec::new()); write_string(&mut e, S, s).unwrap(); e.into_sink() };
/// assert_eq!(enc(""), [0x00]);
/// assert_eq!(enc("hi"), [0x02, b'h', b'i']);
/// assert_eq!(enc("é"), [0x01, 0xE9, 0x01]);                  // U+00E9: two octets
/// assert_eq!(enc("a😀"), [0x02, 0x61, 0x80, 0xEC, 0x07]);   // U+1F600: three octets
/// ```
pub fn write_string<E: Encoder>(
    e: &mut E,
    ch: Channel,
    s: &str,
) -> Result<()> {
    write_uint(e, ch, s.chars().count() as u64)?;
    write_chars(e, ch, s)
}

/// Reads `n` characters and appends them to `out` as UTF-8.
///
/// In byte-aligned coding a code point below 128 is one octet equal to
/// its ASCII byte, so a run of such octets is copied to `out` at once
/// (`ByteDecoder::peek_contiguous`), eight at a time; other characters
/// take the general path.
///
/// # Errors
///
/// `ErrorKind::Value` for a code point that is not a Unicode scalar
/// value (above `0x10FFFF`, or a surrogate `0xD800`–`0xDFFF`);
/// `ErrorKind::Truncated` when input ends, with the characters read so
/// far left in `out`, so a caller can continue after more input.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_chars};
/// let mut out = String::new();
/// read_chars(&mut ByteDecoder::new(&[0x61, 0x80, 0xEC, 0x07], &[]), S, 2, &mut out)?;
/// assert_eq!(out, "a😀");
/// let surrogate = [0x80, 0xB0, 0x03];                         // 0xD800
/// let err = read_chars(&mut ByteDecoder::new(&surrogate, &[]), S, 1, &mut String::new()).unwrap_err();
/// assert_eq!(err.kind(), exi::ErrorKind::Value);
/// let mut part = String::new();
/// let err = read_chars(&mut ByteDecoder::new(b"ab", &[]), S, 3, &mut part).unwrap_err();
/// assert_eq!((err.kind(), part.as_str()), (exi::ErrorKind::Truncated, "ab"));
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_chars<D: Decoder>(
    d: &mut D,
    ch: Channel,
    n: u64,
    out: &mut String,
) -> Result<()> {
    let mut left = n;
    while left > 0 {
        let fast =
            d.take_ascii(ch, left.min(usize::MAX as u64) as usize, out) as u64;
        left -= fast;
        if left == 0 {
            break;
        }
        // A code point needs at most 3 octets; 4 allows a redundant
        // one.
        let cp = read_uint(d, ch, 4)?;
        let c = u32::try_from(cp).ok().and_then(char::from_u32).ok_or_else(
            || {
                err(
                    ErrorKind::Value,
                    "code point is not a Unicode scalar value",
                )
            },
        )?;
        out.push(c);
        left -= 1;
    }
    Ok(())
}

/// Writes the characters of `s` with a restricted character set (module
/// documentation). `set` is sorted by code point, without duplicates,
/// with 1 to 255 members.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// let digits: Vec<char> = ('0'..='9').collect();             // 10 members: width(11) = 4 bits
/// let mut e = ByteEncoder::new(Vec::new());
/// write_restricted_chars(&mut e, S, "12x", &digits)?;
/// let bytes = e.into_sink();
/// assert_eq!(bytes, [0x01, 0x02, 0x0A, 0x78]);               // 'x' is outside: 10, then U+0078
/// let mut out = String::new();
/// read_restricted_chars(&mut ByteDecoder::new(&bytes, &[]), S, 3, &digits, &mut out)?;
/// assert_eq!(out, "12x");
/// let bad = read_restricted_chars(&mut ByteDecoder::new(&[0x0B], &[]), S, 1, &digits, &mut String::new());
/// assert_eq!(bad.unwrap_err().kind(), exi::ErrorKind::Grammar);   // index 11 > N
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_restricted_chars<E: Encoder>(
    e: &mut E,
    ch: Channel,
    s: &str,
    set: &[char],
) -> Result<()> {
    debug_assert!(
        !set.is_empty()
            && set.len() <= 255
            && set.windows(2).all(|w| w[0] < w[1])
    );
    let n = set.len() as u32;
    let w = width(n + 1);
    for c in s.chars() {
        match set.binary_search(&c) {
            Ok(i) => e.nbit(ch, i as u32, w)?,
            Err(_) => {
                e.nbit(ch, n, w)?;
                write_uint(e, ch, c as u64)?;
            }
        }
    }
    Ok(())
}

/// # Errors
///
/// `ErrorKind::Grammar` for an index above `N`; otherwise as
/// [`read_chars`].
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_restricted_chars};
/// let digits: Vec<char> = ('0'..='9').collect();
/// let mut out = String::new();
/// read_restricted_chars(&mut ByteDecoder::new(&[0x01, 0x0A, 0x78], &[]), S, 2, &digits, &mut out)?;
/// assert_eq!(out, "1x");
/// assert_eq!(read_restricted_chars(&mut ByteDecoder::new(&[0x0B], &[]), S, 1, &digits, &mut String::new()).unwrap_err().kind(), exi::ErrorKind::Grammar);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_restricted_chars<D: Decoder>(
    d: &mut D,
    ch: Channel,
    n: u64,
    set: &[char],
    out: &mut String,
) -> Result<()> {
    let size = set.len() as u32;
    let w = width(size + 1);
    for _ in 0..n {
        let x = d.nbit(ch, w)?;
        if x < size {
            out.push(set[x as usize]);
        } else if x == size {
            let cp = read_uint(d, ch, 4)?;
            let c = u32::try_from(cp)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| {
                    err(
                        ErrorKind::Value,
                        "code point is not a Unicode scalar value",
                    )
                })?;
            out.push(c);
        } else {
            return Err(err(
                ErrorKind::Grammar,
                "restricted character index out of range",
            ));
        }
    }
    Ok(())
}

/// An enumeration ordinal among `count` values.
///
/// # Errors
///
/// `ErrorKind::Internal` when `ordinal ≥ count`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::{ByteEncoder, ByteDecoder}, datatypes::*};
/// let mut e = ByteEncoder::new(Vec::new());
/// write_enum(&mut e, S, 0, 1)?;                               // 1 value: 0 bits
/// write_enum(&mut e, S, 2, 3)?;                               // 3 values: width(3) = 2 bits
/// let bytes = e.into_sink();
/// assert_eq!(bytes, [0x02]);
/// let mut d = ByteDecoder::new(&bytes, &[]);
/// assert_eq!((read_enum(&mut d, S, 1)?, read_enum(&mut d, S, 3)?), (0, 2));
/// assert_eq!(write_enum(&mut ByteEncoder::new(Vec::new()), S, 3, 3).unwrap_err().kind(), exi::ErrorKind::Internal);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_enum<E: Encoder>(
    e: &mut E,
    ch: Channel,
    ordinal: u32,
    count: u32,
) -> Result<()> {
    if ordinal >= count {
        return Err(err(
            ErrorKind::Internal,
            "enumeration ordinal out of range",
        ));
    }
    e.nbit(ch, ordinal, width(count))
}

/// # Errors
///
/// `ErrorKind::Grammar` when the ordinal read is `≥ count`.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteDecoder, datatypes::read_enum};
/// assert_eq!(read_enum(&mut ByteDecoder::new(&[0x02], &[]), S, 3)?, 2);
/// assert_eq!(read_enum(&mut ByteDecoder::new(&[0x03], &[]), S, 3).unwrap_err().kind(), exi::ErrorKind::Grammar);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_enum<D: Decoder>(
    d: &mut D,
    ch: Channel,
    count: u32,
) -> Result<u32> {
    let x = d.nbit(ch, width(count))?;
    if x >= count {
        return Err(err(
            ErrorKind::Grammar,
            "enumeration ordinal out of range",
        ));
    }
    Ok(x)
}

/// Writes an event code. Each part is `(value, count)`: the part's
/// value and how many alternatives exist at that level, given the parts
/// before it. A part takes `width(count)` bits, so a level with one
/// alternative takes none.
///
/// ```
/// use exi::coding::{Channel::Structure as S, byte_aligned::ByteEncoder, datatypes::write_event_code};
/// let enc = |parts: &[(u32, u32)]| { let mut e = ByteEncoder::new(Vec::new()); write_event_code(&mut e, parts).map(|_| e.into_sink()) };
/// assert_eq!(enc(&[(0, 1)])?, Vec::<u8>::new());             // one alternative: no octets
/// assert_eq!(enc(&[(2, 4)])?, [0x02]);
/// assert_eq!(enc(&[(0, 1), (3, 4)])?, [0x03]);                // 0.3: first part takes nothing
/// assert_eq!(enc(&[(1, 2), (1, 2), (0, 2)])?, [0x01, 0x01, 0x00]);
/// assert_eq!(enc(&[(4, 4)]).unwrap_err().kind(), exi::ErrorKind::Internal);
/// assert_eq!(enc(&[]).unwrap_err().kind(), exi::ErrorKind::Internal);   // 1 to 3 parts
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_event_code<E: Encoder>(
    e: &mut E,
    parts: &[(u32, u32)],
) -> Result<()> {
    if parts.is_empty()
        || parts.len() > 3
        || parts.iter().any(|&(v, c)| v >= c)
    {
        return Err(err(ErrorKind::Internal, "malformed event code"));
    }
    for &(v, c) in parts {
        e.nbit(Channel::Structure, v, width(c))?;
    }
    Ok(())
}

/// Reads one part of an event code among `count` alternatives.
///
/// # Errors
///
/// `ErrorKind::Grammar` when the value read is `≥ count`.
///
/// ```
/// use exi::coding::{byte_aligned::ByteDecoder, datatypes::read_code_part};
/// let mut d = ByteDecoder::new(&[0x02, 0x03], &[]);
/// assert_eq!(read_code_part(&mut d, 1)?, 0);                  // 0 bits consumed
/// assert_eq!(read_code_part(&mut d, 3)?, 2);
/// assert_eq!(read_code_part(&mut d, 3).unwrap_err().kind(), exi::ErrorKind::Grammar);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn read_code_part<D: Decoder>(d: &mut D, count: u32) -> Result<u32> {
    let x = d.nbit(Channel::Structure, width(count))?;
    if x >= count {
        return Err(err(ErrorKind::Grammar, "event code part out of range"));
    }
    Ok(x)
}

fn err(kind: ErrorKind, detail: &'static str) -> Error {
    Error::new(kind, detail)
}

/// Decimal digits of a number given as base-128 groups, least
/// significant first.
fn groups_to_decimal(groups: &[u8], out: &mut String) {
    // Little-endian base 10^9 limbs.
    let mut limbs: Vec<u32> = Vec::new();
    for &g in groups.iter().rev() {
        let mut carry = g as u64;
        for l in limbs.iter_mut() {
            let v = (*l as u64) * 128 + carry;
            *l = (v % 1_000_000_000) as u32;
            carry = v / 1_000_000_000;
        }
        while carry != 0 {
            limbs.push((carry % 1_000_000_000) as u32);
            carry /= 1_000_000_000;
        }
    }
    match limbs.last() {
        None => out.push('0'),
        Some(top) => {
            use core::fmt::Write;
            let _ = write!(out, "{top}");
            for l in limbs.iter().rev().skip(1) {
                let _ = write!(out, "{l:09}");
            }
        }
    }
}

/// Adds one to a string of decimal digits.
fn decimal_add_one(digits: &mut String) {
    let mut bytes: Vec<u8> = digits.bytes().collect();
    let mut i = bytes.len();
    loop {
        if i == 0 {
            bytes.insert(0, b'1');
            break;
        }
        i -= 1;
        if bytes[i] == b'9' {
            bytes[i] = b'0';
        } else {
            bytes[i] += 1;
            break;
        }
    }
    digits.clear();
    digits.push_str(core::str::from_utf8(&bytes).expect("digits"));
}

/// The digits of `v` reversed, as a number: 50 → 5 reads "05" written
/// as 50.
fn reverse_digits(v: u64, digits: u8) -> u64 {
    let mut r = 0u64;
    let mut x = v;
    for _ in 0..digits {
        r = r * 10 + x % 10;
        x /= 10;
    }
    r
}

/// The reverse of a value read from the wire: returns (fraction, digit
/// count).
fn unreverse(rev: u64) -> Option<(u64, u8)> {
    if rev == 0 {
        return Some((0, 0));
    }
    let mut digits = 0u8;
    let mut x = rev;
    let mut f = 0u64;
    while x != 0 {
        f = f * 10 + x % 10;
        x /= 10;
        digits += 1;
    }
    if digits > 19 { None } else { Some((f, digits)) }
}

fn read_uint_groups<D: Decoder>(
    d: &mut D,
    ch: Channel,
    max_octets: u32,
    groups: &mut Vec<u8>,
) -> Result<()> {
    loop {
        if groups.len() as u64 >= max_octets as u64 {
            return Err(err(
                ErrorKind::Limit,
                "unsigned integer longer than max_integer_octets",
            ));
        }
        let b = d.nbit(ch, 8)? as u8;
        groups.push(b & 0x7F);
        if b & 0x80 == 0 {
            return Ok(());
        }
    }
}
