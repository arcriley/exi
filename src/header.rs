// src/header.rs
//
// EXI Header Struct
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Layout
//!
//! A header is, in order:
//!
//! | Part | Bits | Content |
//! |---|---|---|
//! | Cookie | 0 or 32 | optional: the four octets `$EXI` (`24 45 58 49`) |
//! | Distinguishing bits | 2 | `1`, `0` |
//! | Options present | 1 | `1` when an options document follows |
//! | Version | 5 or more | a preview bit (`0` = final version), then 4-bit groups: each group's value is added to the version; a group of 15 means another group follows, any other value ends the number. The version is the sum plus 1. EXI 1.0 is `0 0000` |
//! | Options document | variable | when present (below) |
//! | Padding | 0–7 | zero bits to the next octet boundary, when the body's coding is not `BitPacked` |
//!
//! All of these are written most significant bit first, packed with no
//! alignment, whatever the body's coding. So without a cookie, the
//! first octet of an EXI 1.0 header is `0x80` without options and
//! `0xA0` with them, and a header with neither cookie nor options is
//! the single octet `0x80`.
//!
//! # Rules for readers
//!
//! | Input | Result |
//! |---|---|
//! | Distinguishing bits other than `10` | `ErrorKind::Header` |
//! | Preview bit 1 | `ErrorKind::Header`: preview versions are rejected |
//! | A version other than 1 | `ErrorKind::Unsupported` |
//! | No options in the stream and none given out of band | `ErrorKind::Header` (reader) |
//! | An options document longer than `Limits::max_options_bytes` | `ErrorKind::Limit` |
//! | An options document with user meta-data or `datatypeRepresentationMap` | `ErrorKind::Unsupported` (this build) |
//! | Options that [`OptionsBuilder::build`](crate::OptionsBuilder::build) rejects | `ErrorKind::Header` |
//!
//! Padding bits are ignored when read.
//!
//! # The options document
//!
//! The options are an XML document in the namespace `http://www.w3.org/2009/exi`, encoded
//! as an EXI body (no header of its own) with these fixed settings:
//! schema-informed by the options schema, `strict`, and otherwise
//! default options, which makes it **bit-packed** even when the
//! stream's body is byte-aligned. Its string tables are its own and
//! start from the initial entries of a schema-informed stream.
//!
//! The document's structure, with every element optional and in this
//! order:
//!
//! ```text
//! header
//!   lesscommon
//!     uncommon
//!       (user meta-data: elements in other namespaces, any number)
//!       alignment          one of: byte | pre-compress   (both empty elements)
//!       selfContained      empty
//!       valueMaxLength     unsignedInt
//!       valuePartitionCapacity   unsignedInt
//!       datatypeRepresentationMap   (any number)
//!     preserve
//!       dtd prefixes lexicalValues comments pis      (each empty)
//!     blockSize            unsignedInt, at least 1
//!   common
//!     compression          empty
//!     fragment             empty
//!     schemaId             string, nillable
//!   strict                 empty
//! ```
//!
//! `Coding::Compression` is `<common><compression/></common>`;
//! `ByteAligned` is `<alignment><byte/></alignment>`; `PreCompression`
//! is `<alignment><pre-compress/></alignment>`; `BitPacked` is neither.
//! Writers write only options that differ from their defaults.
//!
//! ## Its grammar
//!
//! Each state lists its productions in event-code order; the code is
//! the position. The code takes `ceil(log2 n)` bits for `n` productions
//! (0 bits for one). Every production has a one-part code. `→ X` names
//! the next state; an SE production also enters the child's grammar,
//! and the state after the child's EE is the one named.
//!
//! | State | Productions | Bits |
//! |---|---|---|
//! | Doc | SE(header) → DocEnd; SE(\*) → DocEnd | 1 |
//! | DocEnd | ED | 0 |
//! | Header0 | SE(lesscommon) → Header1; SE(common) → Header2; SE(strict) → Header3; EE | 2 |
//! | Header1 | SE(common) → Header2; SE(strict) → Header3; EE | 2 |
//! | Header2 | SE(strict) → Header3; EE | 1 |
//! | Header3 | EE | 0 |
//! | Less0 | SE(uncommon) → Less1; SE(preserve) → Less2; SE(blockSize) → Less3; EE | 2 |
//! | Less1 | SE(preserve) → Less2; SE(blockSize) → Less3; EE | 2 |
//! | Less2 | SE(blockSize) → Less3; EE | 1 |
//! | Less3 | EE | 0 |
//! | Unc0 | SE(alignment) → Unc1; SE(selfContained) → Unc2; SE(valueMaxLength) → Unc3; SE(valuePartitionCapacity) → Unc4; SE(datatypeRepresentationMap) → Unc5; SE(\*) → Unc0; EE | 3 |
//! | Unc1 | SE(selfContained) → Unc2; SE(valueMaxLength) → Unc3; SE(valuePartitionCapacity) → Unc4; SE(datatypeRepresentationMap) → Unc5; EE | 3 |
//! | Unc2 | SE(valueMaxLength) → Unc3; SE(valuePartitionCapacity) → Unc4; SE(datatypeRepresentationMap) → Unc5; EE | 2 |
//! | Unc3 | SE(valuePartitionCapacity) → Unc4; SE(datatypeRepresentationMap) → Unc5; EE | 2 |
//! | Unc4, Unc5 | SE(datatypeRepresentationMap) → Unc5; EE | 1 |
//! | Align0 | SE(byte) → Align1; SE(pre-compress) → Align1 | 1 |
//! | Align1 | EE | 0 |
//! | Pres0 | SE(dtd); SE(prefixes); SE(lexicalValues); SE(comments); SE(pis); EE — each SE → the state after that flag | 3 |
//! | after dtd | SE(prefixes); SE(lexicalValues); SE(comments); SE(pis); EE | 3 |
//! | after prefixes | SE(lexicalValues); SE(comments); SE(pis); EE | 2 |
//! | after lexicalValues | SE(comments); SE(pis); EE | 2 |
//! | after comments | SE(pis); EE | 1 |
//! | after pis | EE | 0 |
//! | Common0 | SE(compression) → Common1; SE(fragment) → Common2; SE(schemaId) → Common3; EE | 2 |
//! | Common1 | SE(fragment) → Common2; SE(schemaId) → Common3; EE | 2 |
//! | Common2 | SE(schemaId) → Common3; EE | 1 |
//! | Common3 | EE | 0 |
//! | Empty element (byte, pre-compress, selfContained, the preserve flags, compression, fragment, strict) | EE | 0 |
//! | UInt0 (valueMaxLength, valuePartitionCapacity, blockSize) | CH → UInt1 | 0 |
//! | UInt1 | EE | 0 |
//! | Sid0 (schemaId) | AT(xsi:nil) → Sid0n; CH → Sid1 | 1 |
//! | Sid0n | if the nil value was true: EE; if false: CH → Sid1 | 0 |
//! | Sid1 | EE | 0 |
//!
//! Values: an unsignedInt is an EXI unsigned integer. A blockSize of 0
//! is `ErrorKind::Header`. The `xsi:nil` value is one bit (1 = true).
//! The schemaId string is coded as a string value in the value tables
//! of the options document: an unsigned integer `L + 2` for a literal
//! of `L` characters, then each character as an unsigned integer code
//! point. (In the options document's fresh tables a schemaId is always
//! a literal.)
//!
//! An SE(\*) in Doc or Unc0, or any SE(datatypeRepresentationMap), is
//! `ErrorKind::Unsupported` in this build.
//!
//! These tables were derived by hand from the options schema. Before
//! this module is marked done, its header test vectors are confirmed
//! against fixtures from a second implementation (D6).

use crate::coding::bits::{BitDecoder, BitEncoder};
use crate::coding::datatypes::{
    read_chars, read_uint, write_chars, write_uint,
};
use crate::coding::{Channel, Decoder, Encoder, Sink};
use crate::options::{Coding, Preserve, SchemaId};
use crate::{Error, ErrorKind, Limits, Options, Result};

const S: Channel = Channel::Structure;
const COOKIE: &[u8; 4] = b"$EXI";

fn header_err(detail: &'static str) -> Error {
    Error::new(ErrorKind::Header, detail)
}

fn unsupported(detail: &'static str) -> Error {
    Error::new(ErrorKind::Unsupported, detail)
}

/// The header a writer writes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Header {
    /// Options in the stream: the stream describes itself.
    WithOptions { cookie: bool },
    /// No options: they are agreed out of band. Without a cookie this
    /// is the octet `0x80`.
    Bare { cookie: bool },
}

impl Default for Header {
    /// `WithOptions { cookie: false }`.
    fn default() -> Self {
        Header::WithOptions { cookie: false }
    }
}

/// A header that was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedHeader {
    pub cookie: bool,
    /// `None` when the header carried no options.
    pub options: Option<Options>,
    /// Where the body starts, in bits from the start of the input: a
    /// multiple of 8 unless the body is bit-packed.
    pub body_offset_bits: u64,
}

/// Reads a header from the start of `input`. Returns `Ok(None)` when
/// `input` ends before the header does; call again with more input.
///
/// When the header has no options, padding cannot be known from the
/// header alone: the body offset assumes a byte-aligned body (the
/// reader corrects it once it knows the options).
///
/// # Errors
///
/// As in the module's "Rules for readers".
///
/// ```
/// use exi::header::parse_header;
/// use exi::{Limits, Coding};
/// let l = Limits::new();
/// let h = parse_header(&[0x80], &l)?.unwrap();
/// assert_eq!((h.cookie, h.options.is_none(), h.body_offset_bits), (false, true, 8));
/// let h = parse_header(b"$EXI\x80", &l)?.unwrap();
/// assert_eq!((h.cookie, h.body_offset_bits), (true, 40));
/// // Options: byte-aligned. Bits after 0xA0: 0 00 00 000 0 100 10 10 (see the grammar).
/// let h = parse_header(&[0xA0, 0x00, 0x4A], &l)?.unwrap();
/// assert_eq!(h.options.unwrap().coding(), Coding::ByteAligned);
/// assert_eq!(h.body_offset_bits, 24);
/// // Options: byte-aligned and fragment; 20 bits, padded to 24.
/// let h = parse_header(&[0xA0, 0x00, 0x48, 0x70], &l)?.unwrap();
/// let o = h.options.unwrap();
/// assert!(o.fragment() && o.coding() == Coding::ByteAligned);
/// assert_eq!(h.body_offset_bits, 32);
/// // Too short: ask for more.
/// assert_eq!(parse_header(&[], &l)?, None);
/// assert_eq!(parse_header(b"$EX", &l)?, None);
/// assert_eq!(parse_header(&[0xA0, 0x00], &l)?, None);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Malformed headers:
///
/// ```
/// use exi::header::parse_header;
/// use exi::{Limits, ErrorKind};
/// let l = Limits::new();
/// let kind = |b: &[u8]| parse_header(b, &l).unwrap_err().kind();
/// assert_eq!(kind(&[0x00]), ErrorKind::Header);          // distinguishing bits 00
/// assert_eq!(kind(&[0xC0]), ErrorKind::Header);          // distinguishing bits 11
/// assert_eq!(kind(&[0x90]), ErrorKind::Header);          // preview version
/// assert_eq!(kind(&[0x81]), ErrorKind::Unsupported);     // version 2
/// assert_eq!(kind(&[0x8F, 0x00]), ErrorKind::Unsupported); // version 16: 1111 then 0000
/// assert_eq!(kind(b"$EXJ\x80"), ErrorKind::Header);      // not a cookie, not 10
/// ```
pub fn parse_header(
    input: &[u8],
    limits: &Limits,
) -> Result<Option<ParsedHeader>> {
    let cookie = match sniff(input) {
        Sniff::NeedMoreData => return Ok(None),
        Sniff::NotExi => {
            return Err(header_err(
                "not an EXI stream: distinguishing bits are not 10",
            ));
        }
        Sniff::Exi { cookie } => cookie,
    };
    let start = if cookie { 32 } else { 0 };
    let mut d = BitDecoder::new(input, start);
    let result = (|| -> Result<ParsedHeader> {
        if d.nbit(S, 2)? != 0b10 {
            return Err(header_err("distinguishing bits are not 10"));
        }
        let has_options = d.nbit(S, 1)? == 1;
        if d.nbit(S, 1)? == 1 {
            return Err(header_err("preview version"));
        }
        let mut version: u32 = 1;
        loop {
            let g = d.nbit(S, 4)?;
            version = version.saturating_add(g);
            if g != 15 {
                break;
            }
        }
        if version != 1 {
            return Err(unsupported("EXI version other than 1"));
        }
        let doc_start = d.bit_position();
        let options = if has_options {
            let o = read_options_doc(&mut d, limits)?;
            if (d.bit_position() - doc_start).div_ceil(8)
                > limits.get_max_options_bytes() as u64
            {
                return Err(Error::new(
                    ErrorKind::Limit,
                    "options document longer than max_options_bytes",
                ));
            }
            Some(o)
        } else {
            None
        };
        let mut offset = d.bit_position();
        if options.as_ref().is_none_or(|o| o.coding() != Coding::BitPacked) {
            offset = offset.div_ceil(8) * 8;
            if offset > 8 * input.len() as u64 {
                return Err(Error::new(
                    ErrorKind::Truncated,
                    "input ends inside the header padding",
                ));
            }
        }
        Ok(ParsedHeader { cookie, options, body_offset_bits: offset })
    })();
    match result {
        Ok(h) => Ok(Some(h)),
        Err(e) if e.kind() == ErrorKind::Truncated => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes a header for `opts` (the options document only with
/// `Header::WithOptions`), including the padding a non-bit-packed body
/// needs.
///
/// # Errors
///
/// `ErrorKind::BufferFull` when the sink is full; nothing is guaranteed
/// about what was written. `ErrorKind::Unsupported` for options this
/// build cannot write.
///
/// ```
/// use exi::header::{write_header, Header};
/// use exi::{Options, Coding};
/// let mut out = Vec::new();
/// write_header(&mut out, Header::Bare { cookie: false }, &Options::default())?;
/// assert_eq!(out, [0x80]);
/// let mut out = Vec::new();
/// write_header(&mut out, Header::Bare { cookie: true }, &Options::default())?;
/// assert_eq!(out, *b"$EXI\x80");
/// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// let mut out = Vec::new();
/// write_header(&mut out, Header::WithOptions { cookie: false }, &byte)?;
/// assert_eq!(out, [0xA0, 0x00, 0x4A]);
/// let frag = Options::builder().coding(Coding::ByteAligned).fragment(true).build()?;
/// let mut out = Vec::new();
/// write_header(&mut out, Header::WithOptions { cookie: false }, &frag)?;
/// assert_eq!(out, [0xA0, 0x00, 0x48, 0x70]);
/// // Default options with an options document: <header/> is the bits 0 (SE header) 11 (EE).
/// let mut out = Vec::new();
/// write_header(&mut out, Header::WithOptions { cookie: false }, &Options::default())?;
/// assert_eq!(out, [0xA0, 0b0110_0000]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Every option survives a round trip:
///
/// ```
/// use exi::header::{write_header, parse_header, Header};
/// use exi::{Options, Coding, Preserve, SchemaId, Limits};
/// let all = [
///     Options::builder().coding(Coding::ByteAligned).strict(true).schema_id(SchemaId::Nil).build()?,
///     Options::builder().coding(Coding::ByteAligned).schema_id(SchemaId::BuiltinTypes).build()?,
///     Options::builder().coding(Coding::Compression).block_size(1).build()?,
///     Options::builder().coding(Coding::PreCompression).value_max_length(Some(0)).build()?,
///     Options::builder().coding(Coding::ByteAligned).preserve(Preserve::all()).self_contained(true)
///         .value_partition_capacity(Some(u32::MAX)).schema_id(SchemaId::Id("urn:x:é".into())).build()?,
/// ];
/// for o in all {
///     let mut out = Vec::new();
///     write_header(&mut out, Header::WithOptions { cookie: true }, &o)?;
///     assert_eq!(parse_header(&out, &Limits::new())?.unwrap().options, Some(o));
/// }
/// # Ok::<(), exi::Error>(())
/// ```
pub fn write_header<S: Sink>(
    sink: &mut S,
    h: Header,
    opts: &Options,
) -> Result<()> {
    let mut e = BitEncoder::new(sink);
    write_header_bits(&mut e, h, opts)?;
    // A sink takes whole octets, so the last bits are padded here even
    // for a bit-packed body; the writer continues a bit-packed body in
    // the same octet instead.
    e.align()
}

/// The header's bits, without the padding: the writer of a bit-packed
/// body continues in the same octet.
pub(crate) fn write_header_bits<E: Encoder>(
    e: &mut E,
    h: Header,
    opts: &Options,
) -> Result<()> {
    let (cookie, with_options) = match h {
        Header::WithOptions { cookie } => (cookie, true),
        Header::Bare { cookie } => (cookie, false),
    };
    if cookie {
        e.octets(S, COOKIE)?;
    }
    e.nbit(S, 0b10, 2)?;
    e.nbit(S, with_options as u32, 1)?;
    e.nbit(S, 0, 5)?; // final version, EXI 1.0
    if with_options {
        write_options_doc(e, opts)?;
    }
    Ok(())
}

/// What the start of some input looks like.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sniff {
    /// EXI. Certain with the cookie; without it, only the
    /// distinguishing bits say so.
    Exi {
        cookie: bool,
    },
    NotExi,
    /// Too short to tell.
    NeedMoreData,
}

/// Tells EXI from other input.
///
/// | Input | Result |
/// |---|---|
/// | starts with `$EXI` | `Exi { cookie: true }` |
/// | a proper prefix of `$EXI`, or empty | `NeedMoreData` |
/// | first octet `0b10xx_xxxx` | `Exi { cookie: false }` |
/// | anything else | `NotExi` |
///
/// Text in UTF-8, UTF-16 or ASCII never starts with an octet
/// `0x80`–`0xBF`, so the second rule never mistakes XML or HTML text
/// for EXI.
///
/// ```
/// use exi::header::{sniff, Sniff};
/// assert_eq!(sniff(b"$EXI\xA0"), Sniff::Exi { cookie: true });
/// assert_eq!(sniff(b"$EX"), Sniff::NeedMoreData);
/// assert_eq!(sniff(b""), Sniff::NeedMoreData);
/// assert_eq!(sniff(&[0x80]), Sniff::Exi { cookie: false });
/// assert_eq!(sniff(&[0xBF]), Sniff::Exi { cookie: false });
/// assert_eq!(sniff(b"<html>"), Sniff::NotExi);
/// assert_eq!(sniff(&[0xEF, 0xBB, 0xBF]), Sniff::NotExi);   // UTF-8 byte order mark
/// assert_eq!(sniff(&[0xFF, 0xFE]), Sniff::NotExi);         // UTF-16 byte order mark
/// assert_eq!(sniff(b"$EXJ"), Sniff::NotExi);
/// ```
pub fn sniff(input: &[u8]) -> Sniff {
    let n = input.len().min(4);
    if input[..n] == COOKIE[..n] {
        return if n == 4 {
            Sniff::Exi { cookie: true }
        } else {
            Sniff::NeedMoreData
        };
    }
    match input.first() {
        Some(b) if b & 0xC0 == 0x80 => Sniff::Exi { cookie: false },
        _ => Sniff::NotExi,
    }
}

// ---------------------------------------------------------------------------------------
// The options document, read and written by the grammar in the module
// documentation. Each element's choice is its position among the
// productions of its state; `ee` is the position of EE in that state.
// ---------------------------------------------------------------------------------------

/// Writes the code `pos` of a state with `n` productions.
fn code<E: Encoder>(e: &mut E, pos: u32, n: u32) -> Result<()> {
    e.nbit(S, pos, crate::coding::width(n))
}

fn write_options_doc<E: Encoder>(e: &mut E, o: &Options) -> Result<()> {
    let byte_or_pre =
        matches!(o.coding(), Coding::ByteAligned | Coding::PreCompression);
    let uncommon = byte_or_pre
        || o.self_contained()
        || o.value_max_length().is_some()
        || o.value_partition_capacity().is_some();
    let preserve = o.preserve() != Preserve::empty();
    // blockSize has an effect only with compression and
    // pre-compression, so it is sent only then (EXIficient does the
    // same).
    let block = o.block_size() != 1_000_000
        && matches!(o.coding(), Coding::Compression | Coding::PreCompression);
    let lesscommon = uncommon || preserve || block;
    let compression = o.coding() == Coding::Compression;
    let common =
        compression || o.fragment() || o.schema_id() != &SchemaId::Absent;

    code(e, 0, 2)?; // Doc: SE(header)
    // Header0..3: lesscommon, common, strict, EE
    let mut h = 0;
    if lesscommon {
        code(e, 0, 4)?;
        h = 1;
        let mut l = 0; // Less0..3: uncommon, preserve, blockSize, EE
        if uncommon {
            code(e, 0, 4)?;
            l = 1;
            // Unc0 has 7 productions (alignment …
            // datatypeRepresentationMap, SE(*), EE); Unc1..5 drop the
            // earlier SEs and SE(*).
            let unc_n = [7u32, 5, 4, 3, 2, 2];
            let unc_ee = [6u32, 4, 3, 2, 1, 1];
            let mut u = 0usize;
            let go = |e: &mut E, target: usize, u: &mut usize| -> Result<()> {
                // In Unc0, SE(alignment) is 0; in Unc_k the SE leading
                // to Unc_t is t - k - 1.
                code(e, (target - *u - 1) as u32, unc_n[*u])?;
                *u = target;
                Ok(())
            };
            if byte_or_pre {
                go(e, 1, &mut u)?;
                code(
                    e,
                    if o.coding() == Coding::ByteAligned { 0 } else { 1 },
                    2,
                )?; // Align0
                // byte / pre-compress: EE (0 bits); Align1: EE (0 bits)
            }
            if o.self_contained() {
                go(e, 2, &mut u)?; // selfContained: EE (0 bits)
            }
            if let Some(n) = o.value_max_length() {
                go(e, 3, &mut u)?;
                write_uint(e, S, n as u64)?; // UInt0: CH; UInt1: EE
            }
            if let Some(n) = o.value_partition_capacity() {
                go(e, 4, &mut u)?;
                write_uint(e, S, n as u64)?;
            }
            code(e, unc_ee[u], unc_n[u])?;
        }
        if preserve {
            code(e, 1 - l, 4 - l)?;
            l = 2;
            let flags = [
                Preserve::DTD,
                Preserve::PREFIXES,
                Preserve::LEXICAL_VALUES,
                Preserve::COMMENTS,
                Preserve::PIS,
            ];
            let mut at = 0usize; // flags before `at` are behind us
            for (i, f) in flags.iter().enumerate() {
                if o.preserve().contains(*f) {
                    code(e, (i - at) as u32, (6 - at) as u32)?; // flag element: EE (0 bits)
                    at = i + 1;
                }
            }
            code(e, (5 - at) as u32, (6 - at) as u32)?;
        }
        if block {
            code(e, 2 - l, 4 - l)?;
            l = 3;
            write_uint(e, S, o.block_size() as u64)?;
        }
        code(e, 3 - l, 4 - l)?;
    }
    if common {
        code(e, 1 - h, 4 - h)?;
        h = 2;
        let mut c = 0; // Common0..3: compression, fragment, schemaId, EE
        if compression {
            code(e, 0, 4)?;
            c = 1;
        }
        if o.fragment() {
            code(e, 1 - c, 4 - c)?;
            c = 2;
        }
        if o.schema_id() != &SchemaId::Absent {
            code(e, 2 - c, 4 - c)?;
            c = 3;
            match o.schema_id() {
                SchemaId::Nil => {
                    code(e, 0, 2)?; // Sid0: AT(xsi:nil)
                    e.nbit(S, 1, 1)?; // true; then EE (0 bits)
                }
                SchemaId::BuiltinTypes => {
                    code(e, 1, 2)?; // Sid0: CH
                    write_uint(e, S, 2)?; // literal of 0 characters
                }
                SchemaId::Id(s) => {
                    code(e, 1, 2)?;
                    write_uint(e, S, s.chars().count() as u64 + 2)?;
                    write_chars(e, S, s)?;
                }
                SchemaId::Absent => unreachable!(),
            }
        }
        code(e, 3 - c, 4 - c)?;
    }
    if o.strict() {
        code(e, 2 - h, 4 - h)?;
        h = 3;
    }
    code(e, 3 - h, 4 - h)?;
    Ok(()) // DocEnd: ED (0 bits)
}

/// Reads the code of a state with `n` productions.
fn choice(d: &mut BitDecoder<'_>, n: u32) -> Result<u32> {
    let v = d.nbit(S, crate::coding::width(n))?;
    if v >= n {
        return Err(header_err("event code beyond the options grammar"));
    }
    Ok(v)
}

fn read_u32(d: &mut BitDecoder<'_>, limits: &Limits) -> Result<u32> {
    let v = read_uint(d, S, limits.get_max_integer_octets())?;
    u32::try_from(v)
        .map_err(|_| header_err("options value beyond unsignedInt"))
}

fn read_options_doc(
    d: &mut BitDecoder<'_>,
    limits: &Limits,
) -> Result<Options> {
    let mut b = Options::builder();
    let mut coding = Coding::BitPacked;
    let mut block_size = 1_000_000;
    if choice(d, 2)? == 1 {
        return Err(unsupported(
            "options document with an unknown root element",
        ));
    }
    let mut h = 0; // Header state
    loop {
        let c = choice(d, 4 - h)? + h; // 0 lesscommon, 1 common, 2 strict, 3 EE
        match c {
            0 => {
                let mut l = 0;
                loop {
                    let c = choice(d, 4 - l)? + l; // 0 uncommon, 1 preserve, 2 blockSize, 3 EE
                    match c {
                        0 => {
                            let mut u = 0u32;
                            loop {
                                // Positions in Unc0: 0 alignment, 1
                                // selfContained, 2 valueMaxLength,
                                // 3 valuePartitionCapacity, 4
                                // datatypeRepresentationMap, 5 SE(*), 6
                                // EE.
                                let (n, base) = match u {
                                    0 => (7, 0),
                                    1 => (5, 1),
                                    2 => (4, 2),
                                    3 => (3, 3),
                                    _ => (2, 4),
                                };
                                let mut c = choice(d, n)? + base;
                                if u > 0 && c == base + n - 1 {
                                    c = 6; // EE
                                }
                                match c {
                                    0 => {
                                        coding = if choice(d, 2)? == 0 {
                                            Coding::ByteAligned
                                        } else {
                                            Coding::PreCompression
                                        };
                                        u = 1;
                                    }
                                    1 => {
                                        b = b.self_contained(true);
                                        u = 2;
                                    }
                                    2 => {
                                        b = b.value_max_length(Some(
                                            read_u32(d, limits)?,
                                        ));
                                        u = 3;
                                    }
                                    3 => {
                                        b = b.value_partition_capacity(Some(
                                            read_u32(d, limits)?,
                                        ));
                                        u = 4;
                                    }
                                    4 => {
                                        return Err(unsupported(
                                            "datatypeRepresentationMap in the options",
                                        ));
                                    }
                                    5 => {
                                        return Err(unsupported(
                                            "user meta-data in the options",
                                        ));
                                    }
                                    _ => break,
                                }
                            }
                            l = 1;
                        }
                        1 => {
                            let flags = [
                                Preserve::DTD,
                                Preserve::PREFIXES,
                                Preserve::LEXICAL_VALUES,
                                Preserve::COMMENTS,
                                Preserve::PIS,
                            ];
                            let mut p = Preserve::empty();
                            let mut at = 0usize;
                            loop {
                                let i =
                                    choice(d, (6 - at) as u32)? as usize + at;
                                if i == 5 {
                                    break;
                                }
                                p = p | flags[i];
                                at = i + 1;
                            }
                            b = b.preserve(p);
                            l = 2;
                        }
                        2 => {
                            block_size = read_u32(d, limits)?;
                            if block_size == 0 {
                                return Err(header_err("blockSize 0"));
                            }
                            l = 3;
                        }
                        _ => break,
                    }
                }
                h = 1;
            }
            1 => {
                let mut c = 0;
                loop {
                    let k = choice(d, 4 - c)? + c; // 0 compression, 1 fragment, 2 schemaId, 3 EE
                    match k {
                        0 => coding = Coding::Compression,
                        1 => b = b.fragment(true),
                        2 => {
                            let id = if choice(d, 2)? == 0 {
                                if d.nbit(S, 1)? == 1 {
                                    SchemaId::Nil
                                } else {
                                    read_schema_string(d, limits)?
                                }
                            } else {
                                read_schema_string(d, limits)?
                            };
                            b = b.schema_id(id);
                        }
                        _ => break,
                    }
                    c = k + 1;
                }
                h = 2;
            }
            2 => {
                b = b.strict(true);
                h = 3;
            }
            _ => break,
        }
    }
    b.coding(coding)
        .block_size(block_size)
        .build()
        .map_err(|_| header_err("options that cannot be combined"))
}

fn read_schema_string(
    d: &mut BitDecoder<'_>,
    limits: &Limits,
) -> Result<SchemaId> {
    let v = read_uint(d, S, limits.get_max_integer_octets())?;
    if v < 2 {
        return Err(header_err(
            "schemaId is a string-table hit in fresh tables",
        ));
    }
    let len = v - 2;
    if len > limits.get_max_options_bytes() as u64 {
        return Err(Error::new(
            ErrorKind::Limit,
            "options document longer than max_options_bytes",
        ));
    }
    let mut s = alloc::string::String::new();
    read_chars(d, S, len, &mut s)?;
    Ok(if s.is_empty() {
        SchemaId::BuiltinTypes
    } else {
        SchemaId::Id(s.into())
    })
}
