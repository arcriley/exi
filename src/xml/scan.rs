// src/xml/scan.rs
//
// Byte scanners for XML text
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The loops that look at every byte of a document: finding the next
//! markup character, counting lines, checking for characters XML
//! forbids. Each has one portable version, 8 bytes at a time in a `u64`
//! (SWAR), and on `x86_64` a 16-byte SSE2 version. SSE2 is part of the
//! `x86_64` baseline, so there is nothing to detect at run time; other
//! targets (`riscv32`, `wasm32`, `aarch64`) use the portable version.
//!
//! | Scanner | Stops at, or counts |
//! |---|---|
//! | [`find3`] | the first of three bytes |
//! | [`find5`] | the first of five bytes |
//! | [`count`] | every occurrence of one byte |
//! | [`first_forbidden`] | the first byte that may begin a character XML forbids: a C0 control other than tab, LF and CR, or `0xEF` (U+FFFE and U+FFFF begin with it) |

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

/// A word with the high bit set in each byte of `w` that is zero, and
/// perhaps in bytes above such a byte (the borrow runs upward), so only
/// the lowest set bit is exact: enough for finding.
#[inline(always)]
fn zeros(w: u64) -> u64 {
    w.wrapping_sub(LO) & !w & HI
}

/// As [`zeros`], exact in every byte: for counting.
#[inline(always)]
fn zeros_exact(w: u64) -> u64 {
    const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    !(((w & LOW7).wrapping_add(LOW7)) | w | LOW7)
}

/// A word with the high bit set in each byte of `w` equal to `b`.
#[inline(always)]
fn eq(w: u64, b: u8) -> u64 {
    zeros(w ^ (LO * b as u64))
}

#[inline(always)]
fn word(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().unwrap())
}

/// The index of the first byte of `s` that is `a`, `b` or `c`, or
/// `s.len()`.
#[inline]
pub(crate) fn find3(s: &[u8], a: u8, b: u8, c: u8) -> usize {
    let mut i = 0;
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::*;
        // SAFETY: SSE2 is always present on x86_64; loads are unaligned
        // and stay inside `s`.
        unsafe {
            let (va, vb, vc) = (
                _mm_set1_epi8(a as i8),
                _mm_set1_epi8(b as i8),
                _mm_set1_epi8(c as i8),
            );
            while i + 16 <= s.len() {
                let v = _mm_loadu_si128(s.as_ptr().add(i).cast());
                let m = _mm_or_si128(
                    _mm_or_si128(_mm_cmpeq_epi8(v, va), _mm_cmpeq_epi8(v, vb)),
                    _mm_cmpeq_epi8(v, vc),
                );
                let bits = _mm_movemask_epi8(m) as u32;
                if bits != 0 {
                    return i + bits.trailing_zeros() as usize;
                }
                i += 16;
            }
        }
    }
    find3_from(s, i, a, b, c)
}

/// The portable part of [`find3`], from `i`.
fn find3_from(s: &[u8], mut i: usize, a: u8, b: u8, c: u8) -> usize {
    while i + 8 <= s.len() {
        let w = word(s, i);
        let m = eq(w, a) | eq(w, b) | eq(w, c);
        if m != 0 {
            return i + (m.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while i < s.len() && s[i] != a && s[i] != b && s[i] != c {
        i += 1;
    }
    i
}

/// The index of the first byte of `s` that is one of `set`, or
/// `s.len()`.
#[inline]
pub(crate) fn find5(s: &[u8], set: [u8; 5]) -> usize {
    let mut i = 0;
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::*;
        // SAFETY: as in `find3`.
        unsafe {
            let v0 = _mm_set1_epi8(set[0] as i8);
            let v1 = _mm_set1_epi8(set[1] as i8);
            let v2 = _mm_set1_epi8(set[2] as i8);
            let v3 = _mm_set1_epi8(set[3] as i8);
            let v4 = _mm_set1_epi8(set[4] as i8);
            while i + 16 <= s.len() {
                let v = _mm_loadu_si128(s.as_ptr().add(i).cast());
                let m = _mm_or_si128(
                    _mm_or_si128(
                        _mm_or_si128(_mm_cmpeq_epi8(v, v0), _mm_cmpeq_epi8(v, v1)),
                        _mm_or_si128(_mm_cmpeq_epi8(v, v2), _mm_cmpeq_epi8(v, v3)),
                    ),
                    _mm_cmpeq_epi8(v, v4),
                );
                let bits = _mm_movemask_epi8(m) as u32;
                if bits != 0 {
                    return i + bits.trailing_zeros() as usize;
                }
                i += 16;
            }
        }
    }
    find5_from(s, i, set)
}

/// The portable part of [`find5`], from `i`.
fn find5_from(s: &[u8], mut i: usize, set: [u8; 5]) -> usize {
    while i + 8 <= s.len() {
        let w = word(s, i);
        let m = eq(w, set[0]) | eq(w, set[1]) | eq(w, set[2]) | eq(w, set[3]) | eq(w, set[4]);
        if m != 0 {
            return i + (m.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while i < s.len() && !set.contains(&s[i]) {
        i += 1;
    }
    i
}

/// How many bytes of `s` are `b`.
#[inline]
pub(crate) fn count(s: &[u8], b: u8) -> usize {
    let mut n = 0;
    let mut i = 0;
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::*;
        // SAFETY: as in `find3`.
        unsafe {
            let vb = _mm_set1_epi8(b as i8);
            while i + 16 <= s.len() {
                let v = _mm_loadu_si128(s.as_ptr().add(i).cast());
                n += (_mm_movemask_epi8(_mm_cmpeq_epi8(v, vb)) as u32).count_ones() as usize;
                i += 16;
            }
        }
    }
    n + count_from(s, i, b)
}

/// The portable part of [`count`], from `i`.
fn count_from(s: &[u8], mut i: usize, b: u8) -> usize {
    let mut n = 0;
    while i + 8 <= s.len() {
        n += zeros_exact(word(s, i) ^ (LO * b as u64)).count_ones() as usize;
        i += 8;
    }
    n + s[i..].iter().filter(|&&x| x == b).count()
}

/// The index of the first byte of `s` that is a C0 control other than
/// tab, LF and CR, or `0xEF`; `s.len()` when there is none.
#[inline]
pub(crate) fn first_forbidden(s: &[u8]) -> usize {
    let mut i = 0;
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::*;
        // SAFETY: as in `find3`.
        unsafe {
            // Unsigned x < 0x20 as a signed compare after flipping the
            // top bit.
            let flip = _mm_set1_epi8(0x80u8 as i8);
            let limit = _mm_set1_epi8((0x20u8 ^ 0x80) as i8);
            let (tab, lf, cr, ef) = (
                _mm_set1_epi8(b'\t' as i8),
                _mm_set1_epi8(b'\n' as i8),
                _mm_set1_epi8(b'\r' as i8),
                _mm_set1_epi8(0xEFu8 as i8),
            );
            while i + 16 <= s.len() {
                let v = _mm_loadu_si128(s.as_ptr().add(i).cast());
                let low = _mm_cmplt_epi8(_mm_xor_si128(v, flip), limit);
                let ok = _mm_or_si128(
                    _mm_or_si128(_mm_cmpeq_epi8(v, tab), _mm_cmpeq_epi8(v, lf)),
                    _mm_cmpeq_epi8(v, cr),
                );
                let m = _mm_or_si128(_mm_andnot_si128(ok, low), _mm_cmpeq_epi8(v, ef));
                let bits = _mm_movemask_epi8(m) as u32;
                if bits != 0 {
                    return i + bits.trailing_zeros() as usize;
                }
                i += 16;
            }
        }
    }
    first_forbidden_from(s, i)
}

/// The portable part of [`first_forbidden`], from `i`.
fn first_forbidden_from(s: &[u8], mut i: usize) -> usize {
    let bad = |x: u8| (x < 0x20 && !matches!(x, b'\t' | b'\n' | b'\r')) || x == 0xEF;
    while i + 8 <= s.len() {
        let w = word(s, i);
        // Bytes below 0x20: the high three bits are zero.
        let low = zeros(w & (0xE0 * LO));
        let ok = eq(w, b'\t') | eq(w, b'\n') | eq(w, b'\r');
        if (low & !ok) | eq(w, 0xEF) != 0 {
            break; // find the byte exactly below
        }
        i += 8;
    }
    while i < s.len() && !bad(s[i]) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Every scanner against a plain loop, at every offset and length
    /// around the 8- and 16-byte steps.
    #[test]
    fn match_plain_loops() {
        let mut r = 12345u64;
        let mut next = || {
            r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (r >> 33) as u8
        };
        let picks = [b'<', b'&', b']', b'\n', b'\t', b'\r', 0x01, 0x1F, 0xEF, 0x80, 0xFF, b'a', b' ', b'"'];
        for len in 0..70 {
            for _ in 0..40 {
                let s: Vec<u8> = (0..len)
                    .map(|_| if next() % 5 == 0 { picks[next() as usize % picks.len()] } else { b'a' + next() % 26 })
                    .collect();
                let plain3 = s.iter().position(|&x| matches!(x, b'<' | b'&' | b']')).unwrap_or(s.len());
                assert_eq!(find3(&s, b'<', b'&', b']'), plain3, "{s:?}");
                let set = *b"\"<&\t\n";
                let plain5 = s.iter().position(|x| set.contains(x)).unwrap_or(s.len());
                assert_eq!(find5(&s, set), plain5, "{s:?}");
                assert_eq!(count(&s, b'\n'), s.iter().filter(|&&x| x == b'\n').count());
                let plainf = s
                    .iter()
                    .position(|&x| (x < 0x20 && !matches!(x, b'\t' | b'\n' | b'\r')) || x == 0xEF)
                    .unwrap_or(s.len());
                assert_eq!(first_forbidden(&s), plainf, "{s:?}");
                // The portable code alone, as riscv32 and wasm32 run it.
                assert_eq!(find3_from(&s, 0, b'<', b'&', b']'), plain3);
                assert_eq!(find5_from(&s, 0, set), plain5);
                assert_eq!(count_from(&s, 0, b'\n'), s.iter().filter(|&&x| x == b'\n').count());
                assert_eq!(first_forbidden_from(&s, 0), plainf);
            }
        }
    }
}
