// src/lexical.rs
//
// Parsing lexical forms into typed values
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! What the writer does with a value given as text for a typed slot
//! Every parser trims XML whitespace first and returns `None` for text
//! that is not a valid lexical form.
//!
//! These are the canonical and common XML Schema forms; anything they
//! do not accept is `ErrorKind::Unencodable` in the writer.

use alloc::string::String;
use alloc::vec::Vec;

use crate::value::{DateTime as Dt, DateTimeKind, Decimal, Float};

fn trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `(value, is_digit_form)`: `true`/`false` or `1`/`0`.
pub(crate) fn parse_bool(s: &str) -> Option<(bool, bool)> {
    match trim(s) {
        "true" => Some((true, false)),
        "false" => Some((false, false)),
        "1" => Some((true, true)),
        "0" => Some((false, true)),
        _ => None,
    }
}

/// An integer: optional sign, digits. `Err(digits)` when it does not
/// fit `i64`: the sign and the digits without leading zeros.
pub(crate) fn parse_int(s: &str) -> Option<Result<i64, (bool, String)>> {
    let s = trim(s);
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if !digits(body) {
        return None;
    }
    match s.parse::<i64>() {
        Ok(v) => Some(Ok(v)),
        Err(_) => {
            let d = body.trim_start_matches('0');
            Some(Err((neg, String::from(if d.is_empty() { "0" } else { d }))))
        }
    }
}

/// A decimal: optional sign, digits, optional point and digits. At most
/// 19 fraction digits and an integral part that fits `u64` (longer
/// forms return `None`).
pub(crate) fn parse_decimal(s: &str) -> Option<Decimal> {
    let s = trim(s);
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body, ""),
    };
    if (int.is_empty() && frac.is_empty())
        || !(int.is_empty() || digits(int))
        || !(frac.is_empty() || digits(frac))
    {
        return None;
    }
    let frac = frac.trim_end_matches('0');
    if frac.len() > 19 {
        return None;
    }
    let integral = if int.is_empty() { 0 } else { int.parse::<u64>().ok()? };
    let fraction =
        if frac.is_empty() { 0 } else { frac.parse::<u64>().ok()? };
    Some(Decimal::new(
        neg && (integral != 0 || fraction != 0),
        integral,
        fraction,
        frac.len() as u8,
    ))
}

/// A float or double: `INF`, `-INF`, `NaN`, or a decimal with an
/// optional exponent.
pub(crate) fn parse_float(s: &str) -> Option<Float> {
    let s = trim(s);
    match s {
        "INF" | "+INF" => return Some(Float::INFINITY),
        "-INF" => return Some(Float::NEG_INFINITY),
        "NaN" => {
            return Some(Float {
                mantissa: 0,
                exponent: Float::SPECIAL_EXPONENT,
            });
        }
        _ => {}
    }
    let (num, exp) = match s.find(['e', 'E']) {
        Some(i) => {
            (&s[..i], s[i + 1..].trim_start_matches('+').parse::<i64>().ok()?)
        }
        None => (s, 0),
    };
    let (neg, body) = match num.as_bytes().first()? {
        b'-' => (true, &num[1..]),
        b'+' => (false, &num[1..]),
        _ => (false, num),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if (int.is_empty() && frac.is_empty())
        || !(int.is_empty() || digits(int))
        || !(frac.is_empty() || digits(frac))
    {
        return None;
    }
    let mut all: String = [int, frac].concat();
    let mut exp = exp - frac.len() as i64;
    let trimmed = all.trim_start_matches('0').len();
    all.drain(..all.len() - trimmed);
    if all.is_empty() {
        return Some(Float { mantissa: 0, exponent: 0 });
    }
    while all.ends_with('0') {
        all.pop();
        exp += 1;
    }
    // Keep at most 18 digits: the mantissa is an i64. Extra digits are
    // dropped (rounded toward zero), as a binary double carries no more
    // precision than that anyway.
    while all.len() > 18 {
        all.pop();
        exp += 1;
    }
    let m: i64 = all.parse().ok()?;
    if !(Float::MIN_EXPONENT as i64..=Float::MAX_EXPONENT as i64)
        .contains(&exp)
    {
        return None;
    }
    Some(Float { mantissa: if neg { -m } else { m }, exponent: exp as i16 })
}

fn num<T: core::str::FromStr>(s: &str, len: usize) -> Option<T> {
    if s.len() != len || !digits(s) {
        return None;
    }
    s.parse().ok()
}

/// A date-time form of `kind`, with an optional time zone (`Z` or
/// `±hh:mm`).
pub(crate) fn parse_datetime(s: &str, kind: DateTimeKind) -> Option<Dt> {
    use DateTimeKind::*;
    let mut s = trim(s);
    // Every date-time form is ASCII; checking first also keeps the
    // slicing below on character boundaries.
    if !s.is_ascii() {
        return None;
    }
    let mut d = Dt {
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
    // Time zone at the end.
    if let Some(rest) = s.strip_suffix('Z') {
        d.tz_minutes = Some(0);
        s = rest;
    } else if s.len() >= 6 {
        let (head, tail) = s.split_at(s.len() - 6);
        let b = tail.as_bytes();
        // Date parts never hold ':', so a sign followed by hh:mm at the
        // end is a zone.
        if (b[0] == b'+' || b[0] == b'-') && b[3] == b':' && !head.is_empty() {
            let h: i16 = num(&tail[1..3], 2)?;
            let m: i16 = num(&tail[4..6], 2)?;
            if h > 14 || m > 59 {
                return None;
            }
            let v = h * 60 + m;
            d.tz_minutes = Some(if b[0] == b'-' { -v } else { v });
            s = head;
        }
    }
    let year = |s: &str| -> Option<i64> {
        let (neg, body) = match s.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, s),
        };
        if body.len() < 4
            || !digits(body)
            || (body.len() > 4 && body.starts_with('0'))
        {
            return None;
        }
        let y: i64 = body.parse().ok()?;
        Some(if neg { -y } else { y })
    };
    let time = |d: &mut Dt, s: &str| -> Option<()> {
        let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
        let p: Vec<&str> = hms.split(':').collect();
        if p.len() != 3 {
            return None;
        }
        d.hour = num(p[0], 2)?;
        d.minute = num(p[1], 2)?;
        d.second = num(p[2], 2)?;
        if !frac.is_empty() {
            if !digits(frac) {
                return None;
            }
            let f = frac.trim_end_matches('0');
            if f.len() > 19 {
                return None;
            }
            if !f.is_empty() {
                d.fraction = Some((f.parse().ok()?, f.len() as u8));
            }
        }
        if d.hour > 24
            || d.minute > 59
            || d.second > 59
            || (d.hour == 24
                && (d.minute, d.second, d.fraction) != (0, 0, None))
        {
            return None;
        }
        Some(())
    };
    // Splits "YYYY-MM-DD" style text from the right: the year may have
    // a sign and more digits.
    let ymd = |d: &mut Dt, s: &str, parts: usize| -> Option<()> {
        let mut rest = s;
        let mut tail = Vec::new();
        for _ in 1..parts {
            let i = rest.rfind('-')?;
            tail.push(&rest[i + 1..]);
            rest = &rest[..i];
        }
        d.year = year(rest)?;
        tail.reverse();
        if let Some(m) = tail.first() {
            d.month = num(m, 2)?;
        }
        if let Some(dd) = tail.get(1) {
            d.day = num(dd, 2)?;
        }
        Some(())
    };
    match kind {
        GYear => d.year = year(s)?,
        GYearMonth => ymd(&mut d, s, 2)?,
        Date => ymd(&mut d, s, 3)?,
        DateTime => {
            let (date, t) = s.split_once('T')?;
            ymd(&mut d, date, 3)?;
            time(&mut d, t)?;
        }
        Time => time(&mut d, s)?,
        GMonth => d.month = num(s.strip_prefix("--")?, 2)?,
        GMonthDay => {
            let (m, dd) = s.strip_prefix("--")?.split_once('-')?;
            d.month = num(m, 2)?;
            d.day = num(dd, 2)?;
        }
        GDay => d.day = num(s.strip_prefix("---")?, 2)?,
    }
    let uses_month =
        matches!(kind, GYearMonth | Date | DateTime | GMonth | GMonthDay);
    let uses_day = matches!(kind, Date | DateTime | GMonthDay | GDay);
    if (uses_month && !(1..=12).contains(&d.month))
        || (uses_day && !(1..=31).contains(&d.day))
    {
        return None;
    }
    Some(d)
}

pub(crate) fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let s = trim(s).as_bytes();
    if s.len() % 2 != 0 {
        return None;
    }
    let v = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    s.chunks(2).map(|p| Some(v(p[0])? << 4 | v(p[1])?)).collect()
}

pub(crate) fn parse_base64(s: &str) -> Option<Vec<u8>> {
    let v = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        .collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for (i, q) in clean.chunks(4).enumerate() {
        let last = i + 1 == clean.len() / 4;
        let pad = q.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &q[..4 - pad] {
            n = n << 6 | v(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

/// The items of a list: whitespace-separated.
pub(crate) fn list_items(s: &str) -> impl Iterator<Item = &str> {
    s.split([' ', '\t', '\n', '\r']).filter(|x| !x.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text from anywhere (an inferred schema tries every value against
    /// every type) never panics a parser: non-ASCII characters at every
    /// position and length.
    #[test]
    fn any_text_is_refused_without_panicking() {
        let pieces = [
            "\u{2764}",
            "é",
            "\u{1F600}",
            "-",
            "+",
            ":",
            "T",
            "Z",
            "1",
            ".",
            "e",
            "=",
        ];
        let kinds = [
            DateTimeKind::GYear,
            DateTimeKind::GYearMonth,
            DateTimeKind::Date,
            DateTimeKind::DateTime,
            DateTimeKind::Time,
            DateTimeKind::GMonth,
            DateTimeKind::GMonthDay,
            DateTimeKind::GDay,
        ];
        let mut seed = 7u32;
        for len in 0..12 {
            for _ in 0..400 {
                let mut t = String::new();
                for _ in 0..len {
                    seed =
                        seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    t.push_str(pieces[(seed >> 16) as usize % pieces.len()]);
                }
                let _ = (
                    parse_bool(&t),
                    parse_int(&t),
                    parse_decimal(&t),
                    parse_float(&t),
                    parse_hex(&t),
                    parse_base64(&t),
                );
                for k in kinds {
                    let _ = parse_datetime(&t, k);
                }
            }
        }
        // The value that found the bug: a heart where a time zone could
        // be.
        assert!(
            parse_datetime(
                "Love great \u{2764}\u{fe0f}",
                DateTimeKind::DateTime
            )
            .is_none()
        );
    }
}
