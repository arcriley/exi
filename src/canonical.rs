// src/canonical.rs
//
// Canonical EXI Writer
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
// The Apache license is incompatable with some free software licenses
// including the GPLv2, the MIT license is available for compatability
//
//! one stream for given events, options and grammars, so that different
//! implementations write the same bytes.
//!
//! Use with [`WriterConfig::canonical`](crate::WriterConfig::canonical).
//! A canonical stream is ordinary EXI: readers need nothing.
//!
//! Not applied: the `whiteSpace` facet of schema types (see 4.3.2.1).
//! Grammars do not carry facets so a type such as `xs:token` is
//! written as given.
//!
//! # Where this follows EXIficient instead of the specification
//!
//! EXIficient 1.0.7 is the reference these rules are tested against
//! (`fixtures/`). Where it differs from the text of EXI C14N, the
//! writer does what EXIficient does, so that the two write the same
//! bytes.
//!
//! Two EXIficient results are not followed, because they change the
//! value: with `utcTime` it writes `2026-01-01+14:00` as `2025-12-28Z`
//! (this crate: `2025-12-31Z`), a `gDay` with a time zone as some other
//! day (this crate: unchanged, since a day of no particular month has
//! no day before it), and `--01+05:00` (a `gMonth`) with day 28 in its
//! MonthDay component (this crate: 31, the last day of the month
//! before).
//!
//! Two of these choices hold outside Canonical EXI too, because
//! EXIficient makes them in every stream: `xsi:nil="false"` is left out
//! of schema-informed streams, and hour 24 is written as 00:00:00 of
//! the next day.

use crate::engine::Prepared;
use crate::{DateTime, DateTimeKind, Decimal, Float};

/// A value in its canonical form; `utc` converts date-times with a time
/// zone to UTC.
pub(crate) fn value(p: Prepared<'_>, utc: bool) -> Prepared<'_> {
    match p {
        Prepared::Float(f) => Prepared::Float(float(f)),
        Prepared::Decimal(d) => Prepared::Decimal(decimal(d)),
        Prepared::DateTime(d) => Prepared::DateTime(datetime(d, utc)),
        Prepared::List(v) => {
            Prepared::List(v.into_iter().map(|x| value(x, utc)).collect())
        }
        other => other,
    }
}

pub(crate) fn decimal(d: Decimal) -> Decimal {
    if d.integral == 0 && d.fraction == 0 {
        Decimal { negative: false, ..d }
    } else {
        d
    }
}

pub(crate) fn float(f: Float) -> Float {
    if f.exponent == Float::SPECIAL_EXPONENT {
        // INF and -INF have mantissa 1 and -1; every other special is
        // NaN, written with 0.
        return if f.mantissa == 1 || f.mantissa == -1 {
            f
        } else {
            Float::NAN
        };
    }
    if f.mantissa == 0 {
        return Float { mantissa: 0, exponent: 0 };
    }
    let (mut m, mut e) = (f.mantissa, f.exponent as i32);
    while m % 10 == 0 && e < Float::MAX_EXPONENT as i32 {
        m /= 10;
        e += 1;
    }
    Float { mantissa: m, exponent: e as i16 }
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in(y: i64, m: u8) -> u8 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Moves a date and time by whole minutes, seconds untouched (XML
/// Schema's "adding durations to dateTimes", as C14N 4.5.5 asks).
fn add_minutes(d: &mut DateTime, minutes: i64) {
    let total = d.hour as i64 * 60 + d.minute as i64 + minutes;
    let mut days = total.div_euclid(24 * 60);
    let rest = total.rem_euclid(24 * 60);
    d.hour = (rest / 60) as u8;
    d.minute = (rest % 60) as u8;
    if d.kind != DateTimeKind::DateTime {
        return; // a time alone wraps around the day
    }
    while days > 0 {
        if d.day < days_in(d.year, d.month) {
            d.day += 1;
        } else {
            d.day = 1;
            if d.month == 12 {
                d.month = 1;
                d.year += 1;
            } else {
                d.month += 1;
            }
        }
        days -= 1;
    }
    while days < 0 {
        if d.day > 1 {
            d.day -= 1;
        } else {
            if d.month == 1 {
                d.month = 12;
                d.year -= 1;
            } else {
                d.month -= 1;
            }
            d.day = days_in(d.year, d.month);
        }
        days += 1;
    }
}

/// 24:00:00 as 00:00:00 of the next day; a time alone wraps to
/// 00:00:00.
pub(crate) fn hour_24(mut d: DateTime) -> DateTime {
    if d.hour == 24
        && matches!(d.kind, DateTimeKind::DateTime | DateTimeKind::Time)
    {
        d.hour = 0;
        add_minutes(&mut d, 24 * 60);
    }
    d
}

pub(crate) fn datetime(mut d: DateTime, utc: bool) -> DateTime {
    use DateTimeKind::*;
    if matches!(d.kind, DateTime | Time) {
        if d.fraction.is_some_and(|(v, _)| v == 0) {
            d.fraction = None;
        }
        d = hour_24(d);
    }
    if utc && matches!(d.kind, GYearMonth | GMonth) && d.day == 0 {
        // The first moment it names is day 1; EXIficient writes that
        // day under utcTime, with a time zone or without.
        d.day = 1;
    }
    let Some(tz) = d.tz_minutes.filter(|_| utc) else { return d };
    if tz == 0 {
        return d; // already UTC
    }
    match d.kind {
        DateTime | Time => {
            add_minutes(&mut d, -(tz as i64));
        }
        // A day of no month in particular has no day before it: left as
        // it is.
        GDay => return d,
        // The first moment the value names (midnight; the first month
        // and day it leaves out; year 0, a leap year, when it has
        // none), moved to UTC; then the fields of its kind. As
        // EXIficient does.
        _ => {
            let mut m = d;
            m.kind = DateTime;
            m.hour = 0;
            m.minute = 0;
            if matches!(d.kind, GMonth | GMonthDay) {
                m.year = 0;
            }
            if matches!(d.kind, GYear) {
                m.month = 1;
            }
            if matches!(d.kind, GYear | GYearMonth | GMonth) {
                m.day = 1;
            }
            add_minutes(&mut m, -(tz as i64));
            if matches!(d.kind, GYear | GYearMonth | Date) {
                d.year = m.year;
            }
            if matches!(d.kind, GYearMonth | Date | GMonth | GMonthDay) {
                d.month = m.month;
            }
            // EXI codes gYearMonth and gMonth with a MonthDay
            // component: its day is the converted one, as EXIficient
            // writes it.
            if matches!(d.kind, Date | GMonthDay | GYearMonth | GMonth) {
                d.day = m.day;
            }
        }
    }
    d.tz_minutes = Some(0);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(
        y: i64,
        mo: u8,
        d: u8,
        h: u8,
        mi: u8,
        s: u8,
        tz: Option<i16>,
    ) -> DateTime {
        DateTime {
            kind: DateTimeKind::DateTime,
            year: y,
            month: mo,
            day: d,
            hour: h,
            minute: mi,
            second: s,
            fraction: None,
            tz_minutes: tz,
        }
    }

    #[test]
    fn floats() {
        assert_eq!(
            float(Float { mantissa: 123_012_300, exponent: -6 }),
            Float { mantissa: 1_230_123, exponent: -4 }
        );
        assert_eq!(
            float(Float { mantissa: 0, exponent: 5 }),
            Float { mantissa: 0, exponent: 0 }
        );
        assert_eq!(
            float(Float { mantissa: 7, exponent: Float::SPECIAL_EXPONENT }),
            Float::NAN
        );
        assert_eq!(float(Float::NEG_INFINITY), Float::NEG_INFINITY);
    }

    #[test]
    fn decimals() {
        assert!(!decimal(Decimal::new(true, 0, 0, 0)).negative);
        assert!(decimal(Decimal::new(true, 0, 5, 1)).negative);
    }

    #[test]
    fn datetimes() {
        // C14N example D-5: 2015-08-11T24:00:00-07:30 is
        // 2015-08-12T07:30:00Z.
        assert_eq!(
            datetime(dt(2015, 8, 11, 24, 0, 0, Some(-450)), true),
            dt(2015, 8, 12, 7, 30, 0, Some(0))
        );
        // Without utcTime only hour 24 changes.
        assert_eq!(
            datetime(dt(2015, 8, 11, 24, 0, 0, Some(-450)), false),
            dt(2015, 8, 12, 0, 0, 0, Some(-450))
        );
        // Across a year and a leap day.
        assert_eq!(
            datetime(dt(2024, 12, 31, 23, 30, 0, Some(-60)), true),
            dt(2025, 1, 1, 0, 30, 0, Some(0))
        );
        assert_eq!(
            datetime(dt(2024, 3, 1, 0, 15, 0, Some(60)), true),
            dt(2024, 2, 29, 23, 15, 0, Some(0))
        );
        // No zone: nothing to convert.
        assert_eq!(
            datetime(dt(2024, 3, 1, 0, 15, 0, None), true),
            dt(2024, 3, 1, 0, 15, 0, None)
        );
        let mut d = dt(2024, 3, 1, 1, 2, 3, None);
        d.fraction = Some((0, 3));
        assert_eq!(datetime(d, false).fraction, None);
    }

    #[test]
    fn dates_in_utc() {
        let k = |kind, y, m, d, tz| crate::DateTime {
            kind,
            year: y,
            month: m,
            day: d,
            hour: 0,
            minute: 0,
            second: 0,
            fraction: None,
            tz_minutes: Some(tz),
        };
        use DateTimeKind::*;
        // Midnight at +05:00 is 19:00 the day before in UTC.
        assert_eq!(
            datetime(k(Date, 2026, 11, 22, 300), true),
            k(Date, 2026, 11, 21, 0)
        );
        assert_eq!(
            datetime(k(Date, 2026, 3, 1, -480), true),
            k(Date, 2026, 3, 1, 0)
        );
        // EXIficient writes 2025-12-28 here.
        assert_eq!(
            datetime(k(Date, 2026, 1, 1, 840), true),
            k(Date, 2025, 12, 31, 0)
        );
        assert_eq!(
            datetime(k(GYearMonth, 2026, 11, 0, 300), true),
            k(GYearMonth, 2026, 10, 31, 0)
        );
        assert_eq!(datetime(k(GYear, 2026, 0, 0, 300), true).year, 2025);
        assert_eq!(
            datetime(k(GMonthDay, 0, 3, 1, 300), true),
            k(GMonthDay, 0, 2, 29, 0)
        );
        assert_eq!(datetime(k(GMonth, 0, 1, 0, 300), true).month, 12);
        // A day of no month: unchanged.
        assert_eq!(
            datetime(k(GDay, 0, 0, 1, 300), true),
            k(GDay, 0, 0, 1, 300)
        );
    }
}
