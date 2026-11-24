// src/strings/hash.rs
//
// String hash for the global string tables
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The string hash of the global tables (ST6).
//!
//! The hash only spreads strings over index keys. It is not
//! cryptographic, and a wrong guess costs a byte comparison, never a
//! wrong answer.
//!
//! [`hash32`] is chosen at compile time:
//!
//! | Build | `hash32` is | Cost |
//! |---|---|---|
//! | `target_feature = "sse4.2"` (x86, x86_64) or `"crc"` (AArch64) | [`crc32c`] | One instruction per 8 bytes |
//! | anything else | [`xorshift32`] | A few shifts and XORs per 4 bytes, with no multiply, so it suits Cortex-M0 and RV32 cores without one |
//!
//! Both functions are always compiled, so tests check each against
//! fixed vectors.
//!
//! **No seed.** Both functions are linear over bits: for strings of one
//! length, which strings collide does not depend on any starting value.
//! A seed would therefore give no protection against strings crafted to
//! collide. Hostile input is bounded instead by the session limits ([`Limits`](crate::Limits)) and the table limits
//! ([`GlobalConfig`](super::GlobalConfig)); a collision costs a longer scan of one key's
//! list, never a wrong result. A keyed hash for servers with 32-bit
//! keys is an open question for the benchmark suite.

/// xor-shift over 32-bit words of `bytes`. Exactly:
///
/// ```text
/// h = 0x9E37_79B9
/// for each complete 4-byte group w (little-endian u32), in order:
///     h ^= w;  h ^= h << 13;  h ^= h >> 17;  h ^= h << 5      (u32 shifts drop bits)
/// t = the 0 to 3 remaining bytes as a little-endian u32, zero-filled (0 when none remain)
/// h ^= t;  h ^= h << 13;  h ^= h >> 17;  h ^= h << 5           (always done, even for t = 0)
/// return h ^ (bytes.len() as u32)
/// ```
///
/// ```
/// use exi::strings::hash::xorshift32;
/// assert_eq!(xorshift32(b""), 0x510c_4619);
/// assert_eq!(xorshift32(b"a"), 0x5084_6a9f);
/// assert_eq!(xorshift32(b"abcd"), 0x86d9_b824);              // one complete group, empty tail
/// assert_eq!(xorshift32(b"juliet@example.com"), 0xfae0_1b6c);
/// ```
pub const fn xorshift32(bytes: &[u8]) -> u32 {
    const fn mix(mut h: u32) -> u32 {
        h ^= h << 13;
        h ^= h >> 17;
        h ^= h << 5;
        h
    }
    let mut h: u32 = 0x9E37_79B9;
    let n = bytes.len();
    let mut i = 0;
    while i + 4 <= n {
        let w = u32::from_le_bytes([
            bytes[i],
            bytes[i + 1],
            bytes[i + 2],
            bytes[i + 3],
        ]);
        h = mix(h ^ w);
        i += 4;
    }
    let mut t: u32 = 0;
    let mut k = 0;
    while i + k < n {
        t |= (bytes[i + k] as u32) << (8 * k);
        k += 1;
    }
    h = mix(h ^ t);
    h ^ (n as u32)
}

/// CRC-32C (Castagnoli, reflected polynomial `0x82F6_3B78`, initial
/// value `0xFFFF_FFFF`, final XOR `0xFFFF_FFFF`) of `bytes`. With the
/// hardware feature it uses the CRC32 instruction; without it, a
/// table-free bitwise loop, used only by tests on other targets.
///
/// ```
/// use exi::strings::hash::crc32c;
/// assert_eq!(crc32c(b""), 0);
/// assert_eq!(crc32c(b"a"), 0xc1d0_4330);
/// assert_eq!(crc32c(b"123456789"), 0xe306_9283);             // the standard check value
/// assert_eq!(crc32c(b"juliet@example.com"), 0xb99b_016b);
/// ```
pub fn crc32c(bytes: &[u8]) -> u32 {
    #[cfg(all(target_arch = "x86_64", target_feature = "sse4.2"))]
    {
        use core::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};
        let mut crc: u64 = 0xFFFF_FFFF;
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            // SAFETY: the sse4.2 target feature is enabled for this
            // build.
            crc = unsafe {
                _mm_crc32_u64(
                    crc,
                    u64::from_le_bytes(c.try_into().expect("8 bytes")),
                )
            };
        }
        let mut crc = crc as u32;
        for &b in chunks.remainder() {
            // SAFETY: as above.
            crc = unsafe { _mm_crc32_u8(crc, b) };
        }
        return !crc;
    }
    #[cfg(all(target_arch = "aarch64", target_feature = "crc"))]
    {
        use core::arch::aarch64::{__crc32cb, __crc32cd};
        let mut crc: u32 = 0xFFFF_FFFF;
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            // SAFETY: the crc target feature is enabled for this build.
            crc = unsafe {
                __crc32cd(
                    crc,
                    u64::from_le_bytes(c.try_into().expect("8 bytes")),
                )
            };
        }
        for &b in chunks.remainder() {
            // SAFETY: as above.
            crc = unsafe { __crc32cb(crc, b) };
        }
        return !crc;
    }
    #[allow(unreachable_code)]
    {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0x82F6_3B78 & 0u32.wrapping_sub(crc & 1));
            }
        }
        !crc
    }
}

/// The table hash: [`crc32c`] where the hardware has it, otherwise
/// [`xorshift32`]. Always computed over the string's UTF-8 bytes.
///
/// ```
/// use exi::strings::hash::{hash32, crc32c, xorshift32};
/// let hw = cfg!(any(target_feature = "sse4.2", target_feature = "crc"));
/// let s = b"urn:example:sensor:1";
/// assert_eq!(hash32(s), if hw { crc32c(s) } else { xorshift32(s) });
/// ```
pub fn hash32(bytes: &[u8]) -> u32 {
    if cfg!(any(target_feature = "sse4.2", target_feature = "crc")) {
        crc32c(bytes)
    } else {
        xorshift32(bytes)
    }
}

/// The 16-bit index key of a hash: the two halves XORed, `(h ^ (h >>
/// 16)) as u16`.
///
/// ```
/// use exi::strings::hash::key16;
/// assert_eq!(key16(0x510c_4619), 0x1715);
/// assert_eq!(key16(0xfae0_1b6c), 0xe18c);
/// assert_eq!(key16(0x0000_ffff), 0xffff);
/// assert_eq!(key16(0xffff_ffff), 0x0000);
/// ```
pub const fn key16(h: u32) -> u16 {
    (h ^ (h >> 16)) as u16
}
