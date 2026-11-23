//! The string hash of the global tables (ST6).
//!
//! The hash only spreads strings over index keys. It is not cryptographic, and a wrong
//! guess costs a byte comparison, never a wrong answer.
//!
//! [`hash32`] is chosen at compile time:
//!
//! | Build | `hash32` is | Cost |
//! |---|---|---|
//! | `target_feature = "sse4.2"` (x86, x86_64) or `"crc"` (AArch64) | [`crc32c`] | One instruction per 8 bytes |
//! | anything else | [`xorshift32`] | A few shifts and XORs per 4 bytes, with no multiply, so it suits Cortex-M0 and RV32 cores without one |
//!
//! Both functions are always compiled, so tests check each against fixed vectors.
//!
//! **No seed.** Both functions are linear over bits: for strings of one length, which
//! strings collide does not depend on any starting value. A seed would therefore give no
//! protection against strings crafted to collide. Hostile input is bounded instead by the
//! session limits ([`Limits`](crate::Limits)) and the table limits
//! ([`GlobalConfig`](super::GlobalConfig)); a collision costs a longer scan of one key's
//! list, never a wrong result. A keyed hash for servers with 32-bit keys is an open
//! question for the benchmark suite.

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
    todo!()
}

/// CRC-32C (Castagnoli, reflected polynomial `0x82F6_3B78`, initial value `0xFFFF_FFFF`,
/// final XOR `0xFFFF_FFFF`) of `bytes`. With the hardware feature it uses the CRC32
/// instruction; without it, a table-free bitwise loop, used only by tests on other targets.
///
/// ```
/// use exi::strings::hash::crc32c;
/// assert_eq!(crc32c(b""), 0);
/// assert_eq!(crc32c(b"a"), 0xc1d0_4330);
/// assert_eq!(crc32c(b"123456789"), 0xe306_9283);             // the standard check value
/// assert_eq!(crc32c(b"juliet@example.com"), 0xb99b_016b);
/// ```
pub fn crc32c(bytes: &[u8]) -> u32 {
    todo!()
}

/// The table hash: [`crc32c`] where the hardware has it, otherwise [`xorshift32`].
/// Always computed over the string's UTF-8 bytes.
///
/// ```
/// use exi::strings::hash::{hash32, crc32c, xorshift32};
/// let hw = cfg!(any(target_feature = "sse4.2", target_feature = "crc"));
/// let s = b"urn:example:sensor:1";
/// assert_eq!(hash32(s), if hw { crc32c(s) } else { xorshift32(s) });
/// ```
pub fn hash32(bytes: &[u8]) -> u32 {
    todo!()
}

/// The 16-bit index key of a hash: the two halves XORed, `(h ^ (h >> 16)) as u16`.
///
/// ```
/// use exi::strings::hash::key16;
/// assert_eq!(key16(0x510c_4619), 0x1715);
/// assert_eq!(key16(0xfae0_1b6c), 0xe18c);
/// assert_eq!(key16(0x0000_ffff), 0xffff);
/// assert_eq!(key16(0xffff_ffff), 0x0000);
/// ```
pub const fn key16(h: u32) -> u16 {
    todo!()
}
