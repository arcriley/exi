// src/capi/mod.rs
//
// C ABI for libexi (feature `capi`)
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! `include/exi.h` declares this module for C, written by hand;
//! `tests/c_api.rs` checks it against the code.
//! Names, numbers and layouts here are the ABI. Layer 2, the document
//! tree, is [`dom`], declared by the hand-written `include/exi_dom.h`.
//!
//! # Conventions
//!
//! | Rule | Detail |
//! |---|---|
//! | Status codes | `exi_status` (`int32_t`): `EXI_OK` 0, `EXI_END` 1, `EXI_NEED_INPUT` 2, negative codes one per `ErrorKind` (see [`crate::ErrorKind`]) |
//! | Strings | `exi_str { ptr, len }`, UTF-8. Strings libexi returns also have a NUL at `ptr[len]` that `len` does not count. Strings passed in need no NUL. An `exi_str` with `len` 0 may have a NULL `ptr` |
//! | Objects | opaque pointers. `_free(NULL)` is a no-op. Every handle starts with a magic tag; each call checks it and returns `EXI_E_ARG` (or NULL) for a wrong or freed handle |
//! | Config structs | start with `uint32_t size` = `sizeof` the struct the caller was compiled with. They only grow at the end; fields beyond `size` read as zero, and zero means the default. Constants are plain integers: an unknown value is `EXI_E_ARG` |
//! | Panics | every export catches panics and returns `EXI_E_INTERNAL` (or NULL); the object is poisoned. Callbacks are plain `extern "C"` and must not unwind |
//! | Poisoning | after a fatal error every call on the object except `_free` returns `EXI_E_STATE`. `EXI_E_UNENCODABLE` is not fatal |
//! | Threads | readers, writers and string tables: one thread at a time. Grammar sets are immutable and may be shared |
//! | Lifetimes | an `exi_strings` must outlive every reader and writer using it. Event payloads are valid until the next call on the same reader |
//!
//! # Readers and writers own their parts
//!
//! An `exi_reader` (likewise `exi_writer`) is one heap block holding,
//! in this drop order, the Rust reader, the grammar set attached to
//! string tables, and its private `GlobalStrings` when the config gave
//! none. Inside the block the Rust lifetimes are erased; that is sound
//! because the block is created and freed as a unit, and fields drop in
//! the order listed. Given an `exi_strings`, the block borrows it
//! instead.
//!
//! # Reader behaviour
//!
//! 1. Header options govern; `cfg->options` is used only when the
//!    header has none. With neither: `EXI_E_HEADER`.
//! 2. `exi_reader_feed` copies into the ring and stores the count taken
//!    in `*accepted`; the caller's buffer is free when the call
//!    returns. Flags apply once all `len` octets are taken.
//!    `exi_reader_buffer` returns free ring space; after reading into
//!    it, call `exi_reader_commit(r, n)`.
//! 3. With `EXI_FORMAT_XML` the reader parses XML text and returns the
//!    same events; `options` and `sequence` are ignored.
//! 4. `exi_reader_run` calls the handlers until the reader needs input
//!    or ends, and returns that status. A handler returning non-zero
//!    stops the run with `EXI_E_ABORTED` and poisons the reader.
//!
//! # Writer behaviour
//!
//! As [`crate::writer`]. With `write` NULL the writer accumulates its
//! output, and `exi_writer_take` returns it (valid until the next call)
//! and empties the buffer. With `EXI_FORMAT_XML` it writes XML text.

#![allow(non_camel_case_types)]

use core::ffi::{c_char, c_int, c_void};
use std::boxed::Box;

#[cfg(feature = "dom")]
pub mod dom;
mod imp;

use imp::*;

/// The reader behind a handle, or `None` for NULL or a wrong or freed
/// handle.
///
/// # Safety
///
/// `r` is NULL or a pointer from `exi_reader_new` not yet freed.
unsafe fn reader<'a>(r: *mut exi_reader) -> Option<&'a mut ReaderObj> {
    // SAFETY: the caller guarantees the pointer.
    let o = unsafe { (r as *mut ReaderObj).as_mut() }?;
    (o.magic == MAGIC_READER).then_some(o)
}

/// # Safety
///
/// As [`reader`], for writers.
unsafe fn writer<'a>(w: *mut exi_writer) -> Option<&'a mut WriterObj> {
    // SAFETY: the caller guarantees the pointer.
    let o = unsafe { (w as *mut WriterObj).as_mut() }?;
    (o.magic == MAGIC_WRITER).then_some(o)
}

/// Runs an export on a block: `EXI_E_ARG` without one; a panic poisons
/// the block and returns `EXI_E_INTERNAL`.
fn on_block<B: Block>(
    b: Option<&mut B>,
    f: impl FnOnce(&mut B) -> exi_status,
) -> exi_status {
    let Some(b) = b else { return EXI_E_ARG };
    let p: *mut B = b;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(b)))
        .unwrap_or_else(|_| {
            // SAFETY: still live: only `_free` ends it.
            unsafe { (*p).poison() };
            EXI_E_INTERNAL
        })
}

fn on_reader(
    r: *mut exi_reader,
    f: impl FnOnce(&mut ReaderObj) -> exi_status,
) -> exi_status {
    // SAFETY: the export's contract.
    guard(EXI_E_INTERNAL, || on_block(unsafe { reader(r) }, f))
}

fn on_writer(
    w: *mut exi_writer,
    f: impl FnOnce(&mut WriterObj) -> exi_status,
) -> exi_status {
    // SAFETY: the export's contract.
    guard(EXI_E_INTERNAL, || on_block(unsafe { writer(w) }, f))
}

/// The grammar set behind a handle, or `None` for NULL or a wrong
/// handle.
///
/// # Safety
///
/// `g` is NULL or a live grammar set.
unsafe fn grammar_set<'a>(
    g: *const exi_grammar_set,
) -> Option<&'a GrammarSetObj> {
    // SAFETY: the caller guarantees the pointer.
    let o = unsafe { (g as *const GrammarSetObj).as_ref() }?;
    (o.magic == MAGIC_GRAMMARS).then_some(o)
}

/// A status code.
pub type exi_status = i32;

pub const EXI_OK: exi_status = 0;
pub const EXI_END: exi_status = 1;
pub const EXI_NEED_INPUT: exi_status = 2;
pub const EXI_E_ARG: exi_status = -1;
pub const EXI_E_STATE: exi_status = -2;
pub const EXI_E_NOMEM: exi_status = -3;
pub const EXI_E_IO: exi_status = -4;
pub const EXI_E_HEADER: exi_status = -5;
pub const EXI_E_GRAMMAR: exi_status = -6;
pub const EXI_E_VALUE: exi_status = -7;
pub const EXI_E_TRUNCATED: exi_status = -8;
pub const EXI_E_LIMIT: exi_status = -9;
pub const EXI_E_SCHEMA: exi_status = -10;
pub const EXI_E_UNSUPPORTED: exi_status = -11;
pub const EXI_E_XML: exi_status = -12;
pub const EXI_E_INTERNAL: exi_status = -13;
pub const EXI_E_ABORTED: exi_status = -14;
pub const EXI_E_BUFFER_FULL: exi_status = -15;
pub const EXI_E_UNENCODABLE: exi_status = -16;

/// The ABI version: major in the high 16 bits, minor in the low 16.
pub const EXI_ABI_MAJOR: u32 = 0;
pub const EXI_ABI_MINOR: u32 = 12;

/// Feature bits from [`exi_features`].
pub const EXI_FEAT_XML_TEXT: u64 = 1 << 1;
pub const EXI_FEAT_SELF_CONTAINED: u64 = 1 << 4;
/// Bit-packed bodies (`EXI_CODING_BIT_PACKED`).
pub const EXI_FEAT_BIT_PACKED: u64 = 1 << 7;
/// Pre-compression and compression (`EXI_CODING_PRE_COMPRESSION`,
/// `EXI_CODING_COMPRESSION`).
pub const EXI_FEAT_DEFLATE: u64 = 1 << 0;
/// Layer 2, the document tree (`include/exi_dom.h`).
pub const EXI_FEAT_DOM: u64 = 1 << 8;

pub const EXI_NO_ID: u32 = u32::MAX;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_str {
    pub ptr: *const c_char,
    pub len: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_bytes {
    pub ptr: *const u8,
    pub len: usize,
}

/// A name. `uri_id`/`local_id` are the session numbers and
/// `global_uri`/`global_local` the global ids; `EXI_NO_ID` when
/// unknown.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_name {
    pub uri: exi_str,
    pub local: exi_str,
    pub prefix: exi_str,
    pub uri_id: u32,
    pub local_id: u32,
    pub global_uri: u32,
    pub global_local: u32,
}

pub const EXI_V_NONE: u32 = 0;
pub const EXI_V_STRING: u32 = 1;
pub const EXI_V_BOOL: u32 = 2;
pub const EXI_V_INT: u32 = 3;
pub const EXI_V_DECIMAL: u32 = 4;
pub const EXI_V_FLOAT: u32 = 5;
pub const EXI_V_BINARY: u32 = 6;
pub const EXI_V_DATETIME: u32 = 7;
pub const EXI_V_QNAME: u32 = 8;
pub const EXI_V_LIST: u32 = 9;
pub const EXI_V_ENUM: u32 = 10;
/// `text` holds the value's digits: a `BigInt` or `BigDecimal`.
pub const EXI_VF_TEXT_ONLY: u32 = 1 << 0;
pub const EXI_VF_LEXICAL: u32 = 1 << 1;
pub const EXI_VF_UNTYPED: u32 = 1 << 2;
pub const EXI_VF_HEX: u32 = 1 << 3;
pub const EXI_VF_BOOL_DIGIT: u32 = 1 << 4;

pub const EXI_DATETIME_G_YEAR: u32 = 1;
pub const EXI_DATETIME_G_YEAR_MONTH: u32 = 2;
pub const EXI_DATETIME_DATE: u32 = 3;
pub const EXI_DATETIME_DATE_TIME: u32 = 4;
pub const EXI_DATETIME_G_MONTH: u32 = 5;
pub const EXI_DATETIME_G_MONTH_DAY: u32 = 6;
pub const EXI_DATETIME_G_DAY: u32 = 7;
pub const EXI_DATETIME_TIME: u32 = 8;

/// `kind` is an `EXI_DATETIME_*` constant.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_datetime {
    pub kind: u32,
    pub tz_minutes: i32,
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub has_tz: u8,
    pub has_frac: u8,
    pub _pad: u8,
    pub frac_digits: u32,
    pub frac: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_decimal {
    pub negative: u32,
    pub fraction_digits: u32,
    pub integral: u64,
    pub fraction: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_float {
    pub mantissa: i64,
    pub exponent: i32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_list {
    pub items: *const exi_value,
    pub count: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union exi_value_u {
    pub boolean: i32,
    pub integer: i64,
    pub dec: exi_decimal,
    pub flt: exi_float,
    pub binary: exi_bytes,
    pub datetime: exi_datetime,
    pub qname: exi_name,
    pub list: exi_list,
    pub enum_ordinal: u32,
}

/// A value: `type` is an `EXI_V_*` constant and selects the union
/// member. `text` is always set for strings and enumerations, and for
/// any value when `EXI_VF_LEXICAL` is set.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct exi_value {
    pub r#type: u32,
    pub flags: u32,
    pub text: exi_str,
    pub u: exi_value_u,
}

pub const EXI_EV_SD: u32 = 1;
pub const EXI_EV_ED: u32 = 2;
pub const EXI_EV_SE: u32 = 3;
pub const EXI_EV_EE: u32 = 4;
pub const EXI_EV_AT: u32 = 5;
pub const EXI_EV_CH: u32 = 6;
pub const EXI_EV_NS: u32 = 7;
pub const EXI_EV_CM: u32 = 8;
pub const EXI_EV_PI: u32 = 9;
pub const EXI_EV_DT: u32 = 10;
pub const EXI_EV_ER: u32 = 11;
pub const EXI_EV_SC: u32 = 12;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_nsdecl {
    pub prefix: exi_str,
    pub uri: exi_str,
    pub local_element_ns: u32,
    pub _pad: u32,
}

/// One event. Only the fields listed for its type are set:
///
/// | Type | Fields |
/// |---|---|
/// | SE, EE | `name` |
/// | AT | `name`, `value` |
/// | CH | `value` |
/// | NS | `ns` (one entry) |
/// | CM | `value.text` |
/// | PI | `name.local` (target), `value.text` (data) |
/// | DT | `name.local`, `pubid`, `sysid`, `value.text` (internal subset) |
/// | ER | `name.local` |
#[repr(C)]
#[derive(Copy, Clone)]
pub struct exi_event {
    pub r#type: u32,
    pub depth: u32,
    pub name: exi_name,
    pub value: exi_value,
    pub pubid: exi_str,
    pub sysid: exi_str,
    pub ns: *const exi_nsdecl,
    pub ns_count: usize,
}

pub const EXI_CODING_BIT_PACKED: u32 = 0;
pub const EXI_CODING_BYTE_ALIGNED: u32 = 1;
pub const EXI_CODING_PRE_COMPRESSION: u32 = 2;
pub const EXI_CODING_COMPRESSION: u32 = 3;
pub const EXI_OPT_STRICT: u32 = 1 << 0;
pub const EXI_OPT_FRAGMENT: u32 = 1 << 1;
pub const EXI_OPT_SELF_CONTAINED: u32 = 1 << 2;
pub const EXI_PRESERVE_DTD: u32 = 1 << 0;
pub const EXI_PRESERVE_PREFIXES: u32 = 1 << 1;
pub const EXI_PRESERVE_LEXICAL: u32 = 1 << 2;
pub const EXI_PRESERVE_COMMENTS: u32 = 1 << 3;
pub const EXI_PRESERVE_PIS: u32 = 1 << 4;
pub const EXI_SCHEMA_ID_ABSENT: u32 = 0;
pub const EXI_SCHEMA_ID_NIL: u32 = 1;
pub const EXI_SCHEMA_ID_BUILTIN: u32 = 2;
pub const EXI_SCHEMA_ID_VALUE: u32 = 3;
/// "Not set" for the optional numeric options.
pub const EXI_ABSENT: u64 = u64::MAX;

/// Options. `exi_options_init` fills the EXI defaults.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_options {
    pub size: u32,
    pub coding: u32,
    pub flags: u32,
    pub preserve: u32,
    pub block_size: u64,
    pub value_max_length: u64,
    pub value_partition_capacity: u64,
    pub schema_id_kind: u32,
    pub _pad: u32,
    pub schema_id: exi_str,
}

/// Limits; a zero field means the default.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_limits {
    pub size: u32,
    pub max_depth: u32,
    pub max_attributes: u32,
    pub max_integer_octets: u32,
    pub max_names: u32,
    pub max_list_items: u32,
    pub max_options_bytes: u32,
    pub max_entity_depth: u32,
    pub max_value_chars: u64,
    pub max_binary_bytes: u64,
    pub max_string_table_bytes: u64,
    pub max_learned_productions: u64,
    pub max_expanded_bytes: u64,
}

/// Opaque: global string tables.
#[repr(C)]
pub struct exi_strings {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_strings_config {
    pub size: u32,
    /// 16 (0 = default) or 32.
    pub hash_bits: u32,
    /// 0 = 16 KiB.
    pub chunk_bytes: u32,
    pub _pad: u32,
    /// 0 = no limit.
    pub max_bytes: u64,
    pub max_strings: u64,
}

/// Opaque: a grammar set.
#[repr(C)]
pub struct exi_grammar_set {
    _private: [u8; 0],
}

/// Opaque: a reader.
#[repr(C)]
pub struct exi_reader {
    _private: [u8; 0],
}

/// Opaque: a writer.
#[repr(C)]
pub struct exi_writer {
    _private: [u8; 0],
}

pub const EXI_FORMAT_EXI: u32 = 0;
pub const EXI_FORMAT_XML: u32 = 1;
pub const EXI_SEQ_SINGLE: u32 = 0;
pub const EXI_SEQ_STREAMS: u32 = 1;
pub const EXI_SEQ_FRAMES: u32 = 2;
pub const EXI_RD_XML_DOCTYPE: u32 = 1 << 3;
pub const EXI_RD_XML_PREDEFINED_ONLY: u32 = 1 << 4;
pub const EXI_FEED_FINAL: u32 = 1 << 0;
pub const EXI_FEED_END_FRAME: u32 = 1 << 1;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_reader_config {
    pub size: u32,
    pub flags: u32,
    pub sequence: u32,
    pub format: u32,
    pub options: *const exi_options,
    /// NULL: schema-less.
    pub grammars: *mut exi_grammar_set,
    pub limits: *const exi_limits,
    /// NULL: private string tables.
    pub strings: *mut exi_strings,
    /// Ring size: 0 = 4096; otherwise 64, 256, 1024, 4096, 16384 or
    /// 65536.
    pub input_buffer_size: u32,
    pub _pad: u32,
}

/// Push callbacks; any may be NULL. Non-zero from a callback stops the
/// run.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_handlers {
    pub size: u32,
    pub start_document: Option<extern "C" fn(user: *mut c_void) -> c_int>,
    pub end_document: Option<extern "C" fn(user: *mut c_void) -> c_int>,
    pub start_element: Option<
        extern "C" fn(user: *mut c_void, name: *const exi_name) -> c_int,
    >,
    pub end_element: Option<
        extern "C" fn(user: *mut c_void, name: *const exi_name) -> c_int,
    >,
    pub attribute: Option<
        extern "C" fn(
            user: *mut c_void,
            name: *const exi_name,
            value: *const exi_value,
        ) -> c_int,
    >,
    pub characters: Option<
        extern "C" fn(user: *mut c_void, value: *const exi_value) -> c_int,
    >,
    pub namespace_decl: Option<
        extern "C" fn(user: *mut c_void, ns: *const exi_nsdecl) -> c_int,
    >,
    pub comment:
        Option<extern "C" fn(user: *mut c_void, text: exi_str) -> c_int>,
    pub processing_instruction: Option<
        extern "C" fn(
            user: *mut c_void,
            target: exi_str,
            data: exi_str,
        ) -> c_int,
    >,
    pub doctype: Option<
        extern "C" fn(
            user: *mut c_void,
            name: exi_str,
            public_id: exi_str,
            system_id: exi_str,
            text: exi_str,
        ) -> c_int,
    >,
    pub entity_reference:
        Option<extern "C" fn(user: *mut c_void, name: exi_str) -> c_int>,
    pub self_contained: Option<extern "C" fn(user: *mut c_void) -> c_int>,
}

pub const EXI_HDR_COOKIE: u32 = 1 << 0;
pub const EXI_HDR_NO_OPTIONS: u32 = 1 << 1;
pub const EXI_WR_XML_DECLARATION: u32 = 1 << 3;
/// Canonical EXI (EXI C14N); refused with `EXI_HDR_COOKIE` and
/// `EXI_CODING_COMPRESSION`.
pub const EXI_WR_CANONICAL: u32 = 1 << 4;
/// With `EXI_WR_CANONICAL`: date-times in UTC (C14N `utcTime`).
pub const EXI_WR_UTC_TIME: u32 = 1 << 5;

/// Called with output octets; a status other than `EXI_OK` stops the
/// writer with that status.
pub type exi_write_fn = Option<
    extern "C" fn(
        user: *mut c_void,
        data: *const u8,
        len: usize,
    ) -> exi_status,
>;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct exi_writer_config {
    pub size: u32,
    pub flags: u32,
    pub header: u32,
    pub sequence: u32,
    pub format: u32,
    pub _pad: u32,
    pub options: *const exi_options,
    pub grammars: *mut exi_grammar_set,
    /// NULL: accumulate; see `exi_writer_take`.
    pub write: exi_write_fn,
    pub write_user: *mut c_void,
    pub limits: *const exi_limits,
    pub strings: *mut exi_strings,
}

// ---------- general ----------

/// `(EXI_ABI_MAJOR << 16) | EXI_ABI_MINOR`.
///
/// ```
/// assert_eq!(exi::capi::exi_abi_version(), (exi::capi::EXI_ABI_MAJOR << 16) | exi::capi::EXI_ABI_MINOR);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn exi_abi_version() -> u32 {
    (EXI_ABI_MAJOR << 16) | EXI_ABI_MINOR
}

/// The crate version, NUL-terminated, static.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let v = core::ffi::CStr::from_ptr(exi_version_string()).to_str().unwrap();
///     assert_eq!(v, env!("CARGO_PKG_VERSION"));
/// }
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn exi_version_string() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// The `EXI_FEAT_*` bits of this build: `EXI_FEAT_SELF_CONTAINED`
/// always, and one bit for each of the features `xml`, `dom`,
/// `bit-packed` and `compression`.
///
/// ```
/// use exi::capi::*;
/// let f = exi_features();
/// assert_ne!(f & EXI_FEAT_SELF_CONTAINED, 0);
/// assert_ne!(f & EXI_FEAT_DOM, 0);
/// assert_eq!(f & EXI_FEAT_BIT_PACKED != 0, cfg!(feature = "bit-packed"));
/// assert_eq!(f & EXI_FEAT_DEFLATE != 0, cfg!(feature = "compression"));
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn exi_features() -> u64 {
    let on = |yes: bool, bit: u64| if yes { bit } else { 0 };
    EXI_FEAT_SELF_CONTAINED
        | on(cfg!(feature = "xml"), EXI_FEAT_XML_TEXT)
        | on(cfg!(feature = "dom"), EXI_FEAT_DOM)
        | on(cfg!(feature = "bit-packed"), EXI_FEAT_BIT_PACKED)
        | on(cfg!(feature = "compression"), EXI_FEAT_DEFLATE)
}

/// A static, NUL-terminated description of a status: `"ok"`, `"end"`,
/// `"need input"`, the `ErrorKind` in words for errors, `"unknown
/// status"` otherwise.
///
/// ```
/// use exi::capi::*;
/// use std::ffi::CStr;
/// let s = |c| unsafe { CStr::from_ptr(exi_status_string(c)) }.to_str().unwrap();
/// assert_eq!(s(EXI_OK), "ok");
/// assert_eq!(s(EXI_E_UNENCODABLE), "event fits no production");
/// assert_eq!(s(12345), "unknown status");
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn exi_status_string(s: exi_status) -> *const c_char {
    status_text(s).as_ptr()
}

// ---------- options and limits ----------

/// Fills `*o` with EXI's defaults and sets `o->size`. NULL is a no-op.
///
/// # Safety
///
/// `o` is NULL or points to writable memory for an `exi_options`.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut o = core::mem::zeroed::<exi_options>();
///     exi_options_init(&mut o);
///     assert_eq!(o.size as usize, core::mem::size_of::<exi_options>());
///     assert_eq!((o.coding, o.flags, o.preserve), (EXI_CODING_BIT_PACKED, 0, 0));
///     assert_eq!(o.schema_id_kind, EXI_SCHEMA_ID_ABSENT);
///     exi_options_init(null_mut());                                // no-op
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_options_init(o: *mut exi_options) {
    if !o.is_null() {
        // SAFETY: the caller guarantees writable memory.
        unsafe { o.write(default_options()) };
    }
}

/// Checks a combination (see [`crate::options`]). On error, `*why`
/// (when not NULL) gets a static NUL-terminated reason.
///
/// # Safety
///
/// `o` points to an `exi_options`; `why` is NULL or writable.
///
/// ```
/// use exi::capi::*;
/// unsafe {
///     let mut o = core::mem::zeroed::<exi_options>();
///     exi_options_init(&mut o);
///     assert_eq!((o.coding, o.block_size, o.value_max_length), (EXI_CODING_BIT_PACKED, 1_000_000, EXI_ABSENT));
///     assert_eq!(exi_options_validate(&o, core::ptr::null_mut()), EXI_OK);
///     o.flags = EXI_OPT_STRICT;
///     o.preserve = EXI_PRESERVE_PREFIXES;
///     let mut why = core::ptr::null();
///     assert_eq!(exi_options_validate(&o, &mut why), EXI_E_ARG);
///     assert!(!why.is_null());
///     o.flags = 0; o.preserve = 0; o.coding = 9;                 // unknown constant
///     assert_eq!(exi_options_validate(&o, core::ptr::null_mut()), EXI_E_ARG);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_options_validate(
    o: *const exi_options,
    why: *mut *const c_char,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees the pointers.
        let Some(o) = (unsafe { read_sized(o) }) else { return EXI_E_ARG };
        // SAFETY: as above.
        match unsafe { options_from(&o) } {
            Ok(_) => EXI_OK,
            Err(reason) => {
                if !why.is_null() {
                    // SAFETY: the caller guarantees `why` is writable.
                    unsafe { why.write(reason.as_ptr()) };
                }
                EXI_E_ARG
            }
        }
    })
}

/// Fills `*l` with the default limits and sets `l->size`. NULL is a
/// no-op.
///
/// # Safety
///
/// `l` is NULL or writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut l = core::mem::zeroed::<exi_limits>();
///     exi_limits_init(&mut l);
///     assert_eq!(l.size as usize, core::mem::size_of::<exi_limits>());
///     assert_eq!(l.max_depth, exi::Limits::new().get_max_depth());
///     exi_limits_init(null_mut());                                 // no-op
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_limits_init(l: *mut exi_limits) {
    if !l.is_null() {
        // SAFETY: the caller guarantees writable memory.
        unsafe { l.write(default_limits()) };
    }
}

// ---------- string tables ----------

/// New global string tables; NULL config: defaults. NULL on a bad
/// config.
///
/// # Safety
///
/// `cfg` is NULL or points to an `exi_strings_config` of at least
/// `cfg->size` bytes.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let t = exi_strings_new(null());                             // defaults
///     assert!(!t.is_null());
///     exi_strings_free(t);
///     let mut c = core::mem::zeroed::<exi_strings_config>();
///     c.size = core::mem::size_of::<exi_strings_config>() as u32;
///     c.hash_bits = 32;
///     let t = exi_strings_new(&c);
///     assert!(!t.is_null());
///     exi_strings_free(t);
///     c.hash_bits = 7;                                             // 16 or 32 only
///     assert!(exi_strings_new(&c).is_null());
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_strings_new(
    cfg: *const exi_strings_config,
) -> *mut exi_strings {
    guard(core::ptr::null_mut(), || {
        // SAFETY: the caller guarantees the pointer.
        let c = unsafe { imp::read_sized(cfg) };
        let mut g = crate::strings::GlobalConfig::new();
        if let Some(c) = c {
            match c.hash_bits {
                0 | 16 => {}
                32 => g = g.hash_bits(32),
                _ => return core::ptr::null_mut(),
            }
            if c.chunk_bytes != 0 {
                g = g.chunk_bytes(c.chunk_bytes);
            }
            if c.max_bytes != 0 {
                g = g.max_bytes(c.max_bytes);
            }
            if c.max_strings != 0 {
                g = g.max_strings(
                    u32::try_from(c.max_strings).unwrap_or(u32::MAX),
                );
            }
        }
        match crate::strings::GlobalStrings::new(g) {
            Ok(strings) => Box::into_raw(Box::new(StringsObj {
                magic: MAGIC_STRINGS,
                strings,
            })) as *mut exi_strings,
            Err(_) => core::ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `s` is NULL or from `exi_strings_new`, and no reader or writer still
/// uses it.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     // Tables shared by two readers, freed after both.
///     let t = exi_strings_new(null());
///     let mut cfg = reader_config(&byte);
///     cfg.strings = t;
///     let (a, b) = (exi_reader_new(&cfg), exi_reader_new(&cfg));
///     exi_reader_free(a);
///     exi_reader_free(b);
///     exi_strings_free(t);
///     exi_strings_free(null_mut());                                // no-op
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_strings_free(s: *mut exi_strings) {
    let p = s as *mut StringsObj;
    // SAFETY: the caller guarantees NULL or a live handle.
    if let Some(o) = unsafe { p.as_mut() } {
        if o.magic == MAGIC_STRINGS {
            o.magic = 0;
            guard((), || {
                // SAFETY: from Box::into_raw in `exi_strings_new`.
                drop(unsafe { Box::from_raw(p) })
            });
        }
    }
}

// ---------- grammar sets ----------

/// The schema-less grammar set: static; retain and release do nothing
/// to it.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let g = exi_grammar_set_schemaless();
///     assert_eq!(g, exi_grammar_set_schemaless());                 // one static set
///     exi_grammar_set_release(exi_grammar_set_retain(g));          // no counting
///     let mut d = [0 as core::ffi::c_char; EXI_DIGEST_LEN];
///     assert_eq!(exi_grammar_set_digest(g, d.as_mut_ptr()), EXI_E_ARG);   // no .exig form
/// }
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn exi_grammar_set_schemaless() -> *mut exi_grammar_set {
    imp::schemaless() as *mut exi_grammar_set
}

/// Adds a reference; returns `g`.
///
/// # Safety
///
/// `g` is a live grammar set.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
/// # #[cfg(feature = "exig")] {
/// # let mut b = exi::grammar::GrammarSet::builder();
/// # let empty = b.empty_type(None);
/// # let a = b.element(exi::OwnedName::new("", "a"), empty, false);
/// # b.global(a);
/// # let file = b.build().unwrap().to_exig().unwrap();
///     let mut g = null_mut();
///     assert_eq!(exi_grammar_set_from_exig(file.as_ptr(), file.len(), &mut g), EXI_OK);
///     let mut cfg = reader_config(&byte);
///     cfg.grammars = exi_grammar_set_retain(g);                    // a second reference
///     exi_grammar_set_release(g);                                  // the first
///     let r = exi_reader_new(&cfg);                                // the set is still live
///     assert!(!r.is_null());
///     exi_reader_free(r);
///     exi_grammar_set_release(cfg.grammars);                       // freed here
/// # }
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_retain(
    g: *mut exi_grammar_set,
) -> *mut exi_grammar_set {
    // SAFETY: the caller guarantees a live set.
    if unsafe { grammar_set(g) }.is_some() {
        // SAFETY: as above.
        unsafe { imp::retain(g as *mut GrammarSetObj) };
    }
    g
}

/// Drops a reference; the set is freed at zero. NULL is a no-op.
///
/// # Safety
///
/// `g` is NULL or a live grammar set the caller holds a reference to.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
/// # #[cfg(feature = "exig")] {
/// # let mut b = exi::grammar::GrammarSet::builder();
/// # let empty = b.empty_type(None);
/// # let a = b.element(exi::OwnedName::new("", "a"), empty, false);
/// # b.global(a);
/// # let file = b.build().unwrap().to_exig().unwrap();
///     let mut g = null_mut();
///     assert_eq!(exi_grammar_set_from_exig(file.as_ptr(), file.len(), &mut g), EXI_OK);
///     let r = exi_reader_new(&{ let mut c = reader_config(&byte); c.grammars = g; c });
///     exi_grammar_set_release(g);                                  // the reader holds its own
///     let mut ev = core::mem::zeroed::<exi_event>();
///     exi_reader_feed(r, [0x80u8, 0x00].as_ptr().cast(), 2, EXI_FEED_FINAL, null_mut());
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_OK);             // SD
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_OK);             // SE(a)
///     assert_eq!(ev.r#type, EXI_EV_SE);
///     exi_reader_free(r);                                          // and drops it
///     exi_grammar_set_release(null_mut());                         // no-op
/// # }
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_release(g: *mut exi_grammar_set) {
    // SAFETY: the caller guarantees NULL or a live set.
    if unsafe { grammar_set(g) }.is_some() {
        // SAFETY: the caller holds a reference.
        unsafe { imp::release(g as *mut GrammarSetObj) };
    }
}

/// The fixed numbers of a declared name; `EXI_E_ARG` when it is not an
/// initial entry, or an output is NULL.
///
/// # Safety
///
/// `g` is live; `uri_id` and `local_id` are writable.
///
/// ```
/// use exi::capi::*;
/// let s = |t: &str| exi_str { ptr: t.as_ptr().cast(), len: t.len() };
/// let g = exi_grammar_set_schemaless();
/// let (mut u, mut l) = (0, 0);
/// unsafe {
///     let xml = "http://www.w3.org/XML/1998/namespace";
///     assert_eq!(exi_grammar_set_name_id(g, s(xml), s("lang"), &mut u, &mut l), EXI_OK);
///     assert_eq!((u, l), (1, 2));                              // base, id, lang, space
///     assert_eq!(exi_grammar_set_name_id(g, s(""), s("nope"), &mut u, &mut l), EXI_E_ARG);
///     assert_eq!(exi_grammar_set_name_id(g, s(xml), s("lang"), core::ptr::null_mut(), &mut l), EXI_E_ARG);
///     // A pointer that is not a grammar set is refused, not used.
///     let mut not_a_set = [0u64; 8];
///     let bogus = not_a_set.as_mut_ptr().cast::<exi_grammar_set>();
///     assert_eq!(exi_grammar_set_name_id(bogus, s(xml), s("lang"), &mut u, &mut l), EXI_E_ARG);
///     assert_eq!(exi_grammar_set_retain(bogus), bogus);        // nothing done
///     exi_grammar_set_release(bogus);
///     assert_eq!(not_a_set, [0; 8]);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_name_id(
    g: *const exi_grammar_set,
    uri: exi_str,
    local: exi_str,
    uri_id: *mut u32,
    local_id: *mut u32,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees the pointers and strings.
        let (Some(o), Some(u), Some(l)) =
            (unsafe { grammar_set(g) }, unsafe { str_of(uri) }, unsafe {
                str_of(local)
            })
        else {
            return EXI_E_ARG;
        };
        if uri_id.is_null() || local_id.is_null() {
            return EXI_E_ARG;
        }
        match o.set.name_id(u, l) {
            Some(id) => {
                // SAFETY: the caller guarantees writable outputs.
                unsafe {
                    uri_id.write(id.uri);
                    local_id.write(id.local);
                }
                EXI_OK
            }
            None => EXI_E_ARG,
        }
    })
}

/// A grammar set from an `.exig` file, with one reference (release it
/// with `exi_grammar_set_release`). NULL `out` or data is `EXI_E_ARG`;
/// a file that fails its checks returns that error, and `*out` is left
/// unchanged.
///
/// ```
/// use exi::capi::*;
/// # #[cfg(feature = "xsd")] {
/// let xsd = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a"/></xs:schema>"#;
/// let file = exi::grammar::GrammarSet::from_xsd(xsd).unwrap().to_exig().unwrap();
/// let mut g: *mut exi_grammar_set = core::ptr::null_mut();
/// unsafe {
///     assert_eq!(exi_grammar_set_from_exig(file.as_ptr(), file.len(), &mut g), EXI_OK);
///     let mut digest = [0 as core::ffi::c_char; EXI_DIGEST_LEN];
///     assert_eq!(exi_grammar_set_digest(g, digest.as_mut_ptr()), EXI_OK);
///     let d = core::ffi::CStr::from_ptr(digest.as_ptr()).to_str().unwrap();
///     assert_eq!(d, exi::grammar::GrammarSet::from_exig(&file).unwrap().digest().unwrap());
///
///     extern "C" fn collect(user: *mut core::ffi::c_void, data: *const u8, len: usize) -> exi_status {
///         let v = unsafe { &mut *(user as *mut Vec<u8>) };
///         v.extend_from_slice(unsafe { core::slice::from_raw_parts(data, len) });
///         EXI_OK
///     }
///     let mut again: Vec<u8> = Vec::new();
///     assert_eq!(exi_grammar_set_to_exig(g, Some(collect), &mut again as *mut _ as *mut _), EXI_OK);
///     assert_eq!(again, file);
///     exi_grammar_set_release(g);
///
///     assert_eq!(exi_grammar_set_from_exig(b"\x80".as_ptr(), 1, &mut g), EXI_E_HEADER);
///     assert_eq!(exi_grammar_set_to_exig(exi_grammar_set_schemaless(), Some(collect), &mut again as *mut _ as *mut _), EXI_E_ARG);
/// }
/// # }
/// ```
///
/// # Safety
///
/// `data` is readable for `len` bytes; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_from_exig(
    data: *const u8,
    len: usize,
    out: *mut *mut exi_grammar_set,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        if out.is_null() || (data.is_null() && len != 0) {
            return EXI_E_ARG;
        }
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees `data` for `len` bytes.
            unsafe { core::slice::from_raw_parts(data, len) }
        };
        match crate::grammar::GrammarSet::from_exig(bytes) {
            Ok(set) => {
                let obj = Box::new(GrammarSetObj {
                    magic: MAGIC_GRAMMARS,
                    refs: core::sync::atomic::AtomicUsize::new(1),
                    is_static: false,
                    set,
                });
                // SAFETY: the caller guarantees `out` is writable.
                unsafe {
                    out.write(Box::into_raw(obj) as *mut exi_grammar_set)
                };
                EXI_OK
            }
            Err(e) => status_of(e.kind()),
        }
    })
}

/// Writes the set's `.exig` file through `write` (called one or more
/// times with `user`); a status other than `EXI_OK` from it stops the
/// writing with that status. Schema-less sets have no `.exig` form:
/// `EXI_E_ARG`.
///
/// # Safety
///
/// `g` is live.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
/// # #[cfg(feature = "exig")] {
/// # let mut b = exi::grammar::GrammarSet::builder();
/// # let empty = b.empty_type(None);
/// # let a = b.element(exi::OwnedName::new("", "a"), empty, false);
/// # b.global(a);
/// # let file = b.build().unwrap().to_exig().unwrap();
///     let mut g = null_mut();
///     assert_eq!(exi_grammar_set_from_exig(file.as_ptr(), file.len(), &mut g), EXI_OK);
///     extern "C" fn refuse(_: *mut core::ffi::c_void, _: *const u8, _: usize) -> exi_status { EXI_E_IO }
///     assert_eq!(exi_grammar_set_to_exig(g, Some(refuse), null_mut()), EXI_E_IO);   // its status
///     assert_eq!(exi_grammar_set_to_exig(g, None, null_mut()), EXI_E_ARG);
///     exi_grammar_set_release(g);
/// # }
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_to_exig(
    g: *const exi_grammar_set,
    write: Option<extern "C" fn(*mut c_void, *const u8, usize) -> exi_status>,
    user: *mut c_void,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees `g`.
        let (Some(o), Some(write)) = (unsafe { grammar_set(g) }, write) else {
            return EXI_E_ARG;
        };
        match o.set.to_exig() {
            Ok(bytes) => write(user, bytes.as_ptr(), bytes.len()),
            Err(e) => status_of(e.kind()),
        }
    })
}

/// Bytes a digest takes with its NUL: `sha256:`, 64 hex digits, NUL.
pub const EXI_DIGEST_LEN: usize = 72;

/// The set's digest (`sha256:` and 64 lowercase hex digits,
/// NUL-terminated) into `out`, which has room for `EXI_DIGEST_LEN`
/// bytes. Schema-less sets: `EXI_E_ARG`.
///
/// # Safety
///
/// `g` is live; `out` is writable for `EXI_DIGEST_LEN` bytes.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
/// # #[cfg(feature = "exig")] {
/// # let mut b = exi::grammar::GrammarSet::builder();
/// # let empty = b.empty_type(None);
/// # let a = b.element(exi::OwnedName::new("", "a"), empty, false);
/// # b.global(a);
/// # let file = b.build().unwrap().to_exig().unwrap();
///     let mut g = null_mut();
///     assert_eq!(exi_grammar_set_from_exig(file.as_ptr(), file.len(), &mut g), EXI_OK);
///     let mut d = [0 as core::ffi::c_char; EXI_DIGEST_LEN];
///     assert_eq!(exi_grammar_set_digest(g, d.as_mut_ptr()), EXI_OK);
///     let digest = core::ffi::CStr::from_ptr(d.as_ptr()).to_str().unwrap();
///     assert_eq!(digest.len(), EXI_DIGEST_LEN - 1);
///     assert!(digest.starts_with("sha256:"));
///     assert_eq!(exi_grammar_set_digest(g, null_mut()), EXI_E_ARG);
///     exi_grammar_set_release(g);
/// # }
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_digest(
    g: *const exi_grammar_set,
    out: *mut c_char,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees `g`.
        let Some(o) = (unsafe { grammar_set(g) }) else { return EXI_E_ARG };
        if out.is_null() {
            return EXI_E_ARG;
        }
        match o.set.digest() {
            Ok(d) => {
                // SAFETY: the caller guarantees room for EXI_DIGEST_LEN
                // bytes; d is 71.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        d.as_ptr(),
                        out.cast::<u8>(),
                        d.len(),
                    );
                    out.add(d.len()).write(0);
                }
                EXI_OK
            }
            Err(e) => status_of(e.kind()),
        }
    })
}

// ---------- reader ----------

/// A new reader; NULL on a bad config (call nothing else to find out
/// why: check the config with `exi_options_validate`).
///
/// # Safety
///
/// `cfg` points to an `exi_reader_config` of at least `cfg->size` bytes
/// whose pointers are NULL or valid for the reader's life (options and
/// limits are copied).
///
/// ```
/// use exi::capi::*;
/// unsafe {
///     let mut o = core::mem::zeroed::<exi_options>();
///     exi_options_init(&mut o);
///     o.coding = EXI_CODING_BYTE_ALIGNED;
///     let mut cfg = core::mem::zeroed::<exi_reader_config>();
///     cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
///     cfg.options = &o;
///     let r = exi_reader_new(&cfg);
///     assert!(!r.is_null());
///     let input = [0x80u8, 0x01, 0x02, b'a', 0x00];
///     let mut taken = 0;
///     assert_eq!(exi_reader_feed(r, input.as_ptr().cast(), input.len(), EXI_FEED_FINAL, &mut taken), EXI_OK);
///     assert_eq!(taken, 5);
///     let mut ev = core::mem::zeroed::<exi_event>();
///     let mut types = Vec::new();
///     loop {
///         match exi_reader_next(r, &mut ev) {
///             EXI_OK => types.push(ev.r#type),
///             EXI_END => break,
///             s => panic!("{s}"),
///         }
///     }
///     assert_eq!(types, [EXI_EV_SD, EXI_EV_SE, EXI_EV_EE, EXI_EV_ED]);
///     exi_reader_free(r);
///     exi_reader_free(core::ptr::null_mut());                 // no-op
///     cfg.flags = 1 << 1;                                     // not a reader flag
///     assert!(exi_reader_new(&cfg).is_null());
///     cfg.flags = 0;
///     cfg.input_buffer_size = 100;                            // not an allowed size
///     assert!(exi_reader_new(&cfg).is_null());
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_new(
    cfg: *const exi_reader_config,
) -> *mut exi_reader {
    guard(core::ptr::null_mut(), || {
        // SAFETY: the caller guarantees the config and its pointers.
        let Some(c) = (unsafe { read_sized(cfg) }) else {
            return core::ptr::null_mut();
        };
        // SAFETY: as above.
        match unsafe { imp::reader_new(&c) } {
            Some(o) => Box::into_raw(Box::new(o)) as *mut exi_reader,
            None => core::ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `r` is NULL or a live reader.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let r = exi_reader_new(&reader_config(&byte));
///     exi_reader_free(r);
///     exi_reader_free(null_mut());                                 // no-op
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_free(r: *mut exi_reader) {
    // SAFETY: the caller guarantees NULL or a live reader.
    if unsafe { reader(r) }.is_some() {
        guard((), || {
            // SAFETY: from Box::into_raw in `exi_reader_new`.
            drop(unsafe { Box::from_raw(r as *mut ReaderObj) })
        });
    }
}

/// Copies as much of `data` as fits into the ring; `*accepted` (may be
/// NULL) gets the count. `EXI_FEED_FINAL` and `EXI_FEED_END_FRAME` take
/// effect only when all `len` octets were taken. `data` may be NULL
/// when `len` is 0, to give flags alone.
///
/// # Safety
///
/// `r` is live; `data` is valid for `len` bytes.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut cfg = reader_config(&byte);
///     cfg.input_buffer_size = 64;
///     let r = exi_reader_new(&cfg);
///     let input = [0u8; 100];
///     let mut taken = 0;
///     assert_eq!(exi_reader_feed(r, input.as_ptr().cast(), input.len(), EXI_FEED_FINAL, &mut taken), EXI_OK);
///     assert_eq!(taken, 64);                                       // the ring is full; FINAL not applied
///     assert_eq!(exi_reader_feed(r, null(), 0, 1 << 9, null_mut()), EXI_E_ARG);     // unknown flag
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_feed(
    r: *mut exi_reader,
    data: *const c_void,
    len: usize,
    flags: u32,
    accepted: *mut usize,
) -> exi_status {
    on_reader(r, |o| {
        if o.poisoned {
            return EXI_E_STATE;
        }
        if flags & !(EXI_FEED_FINAL | EXI_FEED_END_FRAME) != 0
            || (data.is_null() && len != 0)
        {
            return EXI_E_ARG;
        }
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: the caller guarantees `data` for `len` bytes.
            unsafe { core::slice::from_raw_parts(data.cast(), len) }
        };
        let n = o.reader.feed(bytes);
        if !accepted.is_null() {
            // SAFETY: the caller guarantees `accepted` is writable.
            unsafe { accepted.write(n) };
        }
        if n == len {
            if flags & EXI_FEED_END_FRAME != 0 {
                o.reader.end_frame();
            }
            if flags & EXI_FEED_FINAL != 0 {
                o.reader.end_input();
            }
        }
        EXI_OK
    })
}

/// Free ring space for reading in place; `*len` gets its size.
///
/// # Safety
///
/// `r` is live; `len` is writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let r = exi_reader_new(&reader_config(&byte));
///     let mut room = 0;
///     let buf = exi_reader_buffer(r, &mut room);
///     assert!(room >= 5);
///     let doc = [0x80u8, 0x01, 0x02, b'a', 0x00];                   // <a/>
///     core::ptr::copy_nonoverlapping(doc.as_ptr(), buf, doc.len()); // e.g. read(2) into it
///     assert_eq!(exi_reader_commit(r, doc.len()), EXI_OK);
///     exi_reader_feed(r, null(), 0, EXI_FEED_FINAL, null_mut());
///     let mut ev = core::mem::zeroed::<exi_event>();
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_OK);
///     assert_eq!(ev.r#type, EXI_EV_SD);
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_buffer(
    r: *mut exi_reader,
    len: *mut usize,
) -> *mut u8 {
    guard(core::ptr::null_mut(), || {
        // SAFETY: the caller guarantees a live reader.
        let Some(o) = (unsafe { reader(r) }) else {
            return core::ptr::null_mut();
        };
        let b = o.reader.buffer();
        if !len.is_null() {
            // SAFETY: the caller guarantees `len` is writable.
            unsafe { len.write(b.len()) };
        }
        b.as_mut_ptr()
    })
}

/// # Safety
///
/// `r` is live; `n` is at most the length `exi_reader_buffer` gave.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let r = exi_reader_new(&reader_config(&byte));
///     let mut room = 0;
///     exi_reader_buffer(r, &mut room);
///     assert_eq!(exi_reader_commit(r, room + 1), EXI_E_ARG);       // more than there is room for
///     assert_eq!(exi_reader_commit(r, 0), EXI_OK);
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_commit(
    r: *mut exi_reader,
    n: usize,
) -> exi_status {
    on_reader(r, |o| {
        if o.poisoned {
            return EXI_E_STATE;
        }
        if n > o.reader.buffer().len() {
            return EXI_E_ARG;
        }
        o.reader.commit(n);
        EXI_OK
    })
}

/// `EXI_OK` with `*out` filled, `EXI_NEED_INPUT`, `EXI_END`, or an
/// error.
///
/// # Safety
///
/// `r` is live; `out` is writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let r = exi_reader_new(&reader_config(&byte));
///     let mut ev = core::mem::zeroed::<exi_event>();
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_NEED_INPUT);
///     let doc = [0x80u8, 0x01, 0x02, b'a', 0x00];
///     exi_reader_feed(r, doc.as_ptr().cast(), doc.len(), EXI_FEED_FINAL, null_mut());
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_OK);             // SD
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_OK);
///     assert_eq!((ev.r#type, ev.depth), (EXI_EV_SE, 1));
///     let local = core::slice::from_raw_parts(ev.name.local.ptr.cast::<u8>(), ev.name.local.len + 1);
///     assert_eq!(local, b"a\0");                                   // NUL-terminated too
///     assert_eq!(exi_reader_next(r, null_mut()), EXI_E_ARG);
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_next(
    r: *mut exi_reader,
    out: *mut exi_event,
) -> exi_status {
    if out.is_null() {
        return EXI_E_ARG;
    }
    on_reader(r, |o| {
        // SAFETY: the caller guarantees `out` is writable.
        o.next(unsafe { &mut *out })
    })
}

/// The stream's options once the header is read, else NULL. Valid while
/// the reader lives; with `EXI_SEQ_STREAMS`, a later stream's header
/// changes what it holds.
///
/// # Safety
///
/// `r` is live.
///
/// ```
/// # #[cfg(feature = "exig")] {
/// use exi::capi::*;
/// use exi::{Coding, EventSink, OwnedName, Options, SchemaId, Writer, WriterConfig};
/// use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalConfig, GlobalStrings};
/// let mut b = GrammarSet::builder();
/// let empty = b.empty_type(None);
/// let a = b.element(OwnedName::new("", "a"), empty, false);
/// b.global(a);
/// let set = b.build()?;
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let grammars = set.attach(&strings)?;
/// // Two streams back to back, the second with a schemaId.
/// let mut input = Vec::new();
/// for id in [SchemaId::Absent, SchemaId::Id("urn:a".into())] {
///     let o = Options::builder().coding(Coding::ByteAligned).schema_id(id).build()?;
///     let mut w = Writer::new(Vec::new(), WriterConfig::new(o, &grammars))?;
///     w.start_element("a")?;
///     w.end_element()?;
///     w.end_document()?;
///     input.extend(w.into_sink());
/// }
/// unsafe {
///     let mut cfg = core::mem::zeroed::<exi_reader_config>();
///     cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
///     cfg.sequence = EXI_SEQ_STREAMS;
///     let exig = set.to_exig()?;
///     assert_eq!(exi_grammar_set_from_exig(exig.as_ptr(), exig.len(), &mut cfg.grammars), EXI_OK);
///     let r = exi_reader_new(&cfg);
///     assert!(exi_reader_options(r).is_null());               // no header read yet
///     exi_reader_feed(r, input.as_ptr().cast(), input.len(), EXI_FEED_FINAL, core::ptr::null_mut());
///     let mut ev = core::mem::zeroed::<exi_event>();
///     let mut kinds = Vec::new();
///     loop {
///         match exi_reader_next(r, &mut ev) {
///             EXI_OK if ev.r#type == EXI_EV_SD => kinds.push((*exi_reader_options(r)).schema_id_kind),
///             EXI_OK => {}
///             s => { assert_eq!(s, EXI_END); break }
///         }
///     }
///     assert_eq!(kinds, [EXI_SCHEMA_ID_ABSENT, EXI_SCHEMA_ID_VALUE]);
///     let id = (*exi_reader_options(r)).schema_id;
///     assert_eq!(core::slice::from_raw_parts(id.ptr.cast::<u8>(), id.len + 1), b"urn:a\0");
///     exi_reader_free(r);
///     exi_grammar_set_release(cfg.grammars);
/// }
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_options(
    r: *const exi_reader,
) -> *const exi_options {
    guard(core::ptr::null(), || {
        // SAFETY: the caller guarantees a live reader.
        let Some(o) = (unsafe { reader(r as *mut exi_reader) }) else {
            return core::ptr::null();
        };
        let Some(now) = o.reader.options() else { return core::ptr::null() };
        if o.options.as_ref().is_none_or(|(was, _)| was != now) {
            let c = options_to(now, &mut o.options_keep);
            o.options = Some((now.clone(), c));
        }
        o.options.as_ref().map_or(core::ptr::null(), |(_, c)| c as *const _)
    })
}

/// The last error: its status; `*msg`, `*byte_offset`, `*event_index`
/// when not NULL.
///
/// # Safety
///
/// `r` is live; out pointers are NULL or writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let r = exi_reader_new(&reader_config(&byte));
///     assert_eq!(exi_reader_error(r, null_mut(), null_mut(), null_mut()), EXI_OK);  // none yet
///     exi_reader_feed(r, [0x80u8, 0x04].as_ptr().cast(), 2, EXI_FEED_FINAL, null_mut());
///     let mut ev = core::mem::zeroed::<exi_event>();
///     exi_reader_next(r, &mut ev);                                 // SD
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_E_GRAMMAR);      // a URI that does not exist
///     let (mut msg, mut byte_offset, mut event) = (null(), 0, 0);
///     assert_eq!(exi_reader_error(r, &mut msg, &mut byte_offset, &mut event), EXI_E_GRAMMAR);
///     assert!(!core::ffi::CStr::from_ptr(msg).to_bytes().is_empty());
///     assert_eq!(byte_offset, 1);
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_error(
    r: *const exi_reader,
    msg: *mut *const c_char,
    byte_offset: *mut u64,
    event_index: *mut u64,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees a live reader.
        let Some(o) = (unsafe { reader(r as *mut exi_reader) }) else {
            return EXI_E_ARG;
        };
        let Some(e) = &o.last else { return EXI_OK };
        // SAFETY: the caller guarantees the outputs are NULL or
        // writable.
        unsafe {
            if !msg.is_null() {
                msg.write(o.last_msg.as_ptr());
            }
            if !byte_offset.is_null() {
                byte_offset.write(e.position().byte);
            }
            if !event_index.is_null() {
                event_index.write(e.position().event);
            }
        }
        status_of(e.kind())
    })
}

/// Runs the handlers (see the module documentation).
///
/// # Safety
///
/// `r` is live; `h` points to an `exi_handlers` of at least `h->size`
/// bytes.
///
/// ```
/// use exi::capi::*;
/// extern "C" fn stop(_: *mut core::ffi::c_void, _: *const exi_name) -> core::ffi::c_int { 1 }
/// unsafe {
///     let mut cfg = core::mem::zeroed::<exi_reader_config>();
///     cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
///     cfg.format = EXI_FORMAT_XML;
///     let r = exi_reader_new(&cfg);
///     exi_reader_feed(r, b"<a/>".as_ptr().cast(), 4, EXI_FEED_FINAL, core::ptr::null_mut());
///     let mut h = core::mem::zeroed::<exi_handlers>();
///     h.size = core::mem::size_of::<exi_handlers>() as u32;
///     h.start_element = Some(stop);
///     assert_eq!(exi_reader_run(r, &h, core::ptr::null_mut()), EXI_E_ABORTED);
///     let mut ev = core::mem::zeroed::<exi_event>();
///     assert_eq!(exi_reader_next(r, &mut ev), EXI_E_STATE);    // poisoned
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_run(
    r: *mut exi_reader,
    h: *const exi_handlers,
    user: *mut c_void,
) -> exi_status {
    // SAFETY: the caller guarantees the handlers.
    let Some(h) = (unsafe { read_sized(h) }) else { return EXI_E_ARG };
    on_reader(r, |o| {
        loop {
            // SAFETY: plain data; zero is valid.
            let mut ev: exi_event = unsafe { core::mem::zeroed() };
            match o.next(&mut ev) {
                EXI_OK => {}
                s => return s,
            }
            let v = &ev.value;
            let rc = match ev.r#type {
                EXI_EV_SD => h.start_document.map(|f| f(user)),
                EXI_EV_ED => h.end_document.map(|f| f(user)),
                EXI_EV_SE => h.start_element.map(|f| f(user, &ev.name)),
                EXI_EV_EE => h.end_element.map(|f| f(user, &ev.name)),
                EXI_EV_AT => h.attribute.map(|f| f(user, &ev.name, v)),
                EXI_EV_CH => h.characters.map(|f| f(user, v)),
                EXI_EV_NS => h.namespace_decl.map(|f| f(user, ev.ns)),
                EXI_EV_CM => h.comment.map(|f| f(user, v.text)),
                EXI_EV_PI => h
                    .processing_instruction
                    .map(|f| f(user, ev.name.local, v.text)),
                EXI_EV_DT => h.doctype.map(|f| {
                    f(user, ev.name.local, ev.pubid, ev.sysid, v.text)
                }),
                EXI_EV_ER => {
                    h.entity_reference.map(|f| f(user, ev.name.local))
                }
                EXI_EV_SC => h.self_contained.map(|f| f(user)),
                _ => None,
            };
            if rc.is_some_and(|c| c != 0) {
                return o.fail(crate::Error::new(
                    crate::ErrorKind::Aborted,
                    "a handler stopped the run",
                ));
            }
        }
    })
}

// ---------- writer ----------

/// A new writer; NULL on a bad config.
///
/// # Safety
///
/// As `exi_reader_new`, for `exi_writer_config`.
///
/// ```
/// use exi::capi::*;
/// unsafe {
///     let mut o = core::mem::zeroed::<exi_options>();
///     exi_options_init(&mut o);
///     o.coding = EXI_CODING_BYTE_ALIGNED;
///     let mut cfg = core::mem::zeroed::<exi_writer_config>();
///     cfg.size = core::mem::size_of::<exi_writer_config>() as u32;
///     cfg.options = &o;
///     cfg.header = EXI_HDR_NO_OPTIONS;
///     let w = exi_writer_new(&cfg);
///     let s = |t: &str| exi_str { ptr: t.as_ptr().cast(), len: t.len() };
///     assert_eq!(exi_writer_start_element(w, s(""), s("a")), EXI_OK);
///     assert_eq!(exi_writer_end_element(w), EXI_OK);
///     assert_eq!(exi_writer_end_document(w), EXI_OK);
///     let (mut p, mut n) = (core::ptr::null(), 0);
///     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
///     assert_eq!(core::slice::from_raw_parts(p, n), [0x80, 0x01, 0x02, b'a', 0x00]);
///     assert_eq!(exi_writer_end_element(w), EXI_E_STATE);
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_new(
    cfg: *const exi_writer_config,
) -> *mut exi_writer {
    guard(core::ptr::null_mut(), || {
        // SAFETY: the caller guarantees the config and its pointers.
        let Some(c) = (unsafe { read_sized(cfg) }) else {
            return core::ptr::null_mut();
        };
        // SAFETY: as above.
        match unsafe { imp::writer_new(&c) } {
            Some(o) => Box::into_raw(Box::new(o)) as *mut exi_writer,
            None => core::ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `w` is NULL or a live writer.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let w = exi_writer_new(&writer_config(&byte));
///     exi_writer_free(w);
///     exi_writer_free(null_mut());                                 // no-op
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_free(w: *mut exi_writer) {
    // SAFETY: the caller guarantees NULL or a live writer.
    if unsafe { writer(w) }.is_some() {
        guard((), || {
            // SAFETY: from Box::into_raw in `exi_writer_new`.
            drop(unsafe { Box::from_raw(w as *mut WriterObj) })
        });
    }
}

/// Any event, as the reader returns them.
///
/// # Safety
///
/// `w` is live; `ev` points to a filled `exi_event`.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut cfg = writer_config(&byte);
///     cfg.flags = 1 << 30;                                    // not a writer flag
///     assert!(exi_writer_new(&cfg).is_null());
///     cfg.flags = 0;
///     let w = exi_writer_new(&cfg);
///     let s = |t: &str| exi_str { ptr: t.as_ptr().cast(), len: t.len() };
///     assert_eq!(exi_writer_start_element(w, s(""), s("at")), EXI_OK);
///     let mut ev = core::mem::zeroed::<exi_event>();
///     ev.r#type = EXI_EV_CH;
///     ev.value.r#type = EXI_V_DATETIME;
///     ev.value.u.datetime.kind = EXI_DATETIME_DATE;
///     ev.value.u.datetime.year = 2026;
///     ev.value.u.datetime.month = 11;
///     ev.value.u.datetime.day = 25;
///     assert_eq!(exi_writer_event(w, &ev), EXI_OK);
///     ev.value.u.datetime.has_tz = 1;
///     ev.value.u.datetime.tz_minutes = 1 << 20;               // beyond any zone
///     assert_eq!(exi_writer_event(w, &ev), EXI_E_ARG);
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_event(
    w: *mut exi_writer,
    ev: *const exi_event,
) -> exi_status {
    if ev.is_null() {
        return EXI_E_ARG;
    }
    on_writer(w, |o| {
        // SAFETY: the caller guarantees a filled event.
        let ev = unsafe { &*ev };
        let mut status = EXI_E_ARG;
        // SAFETY: as above.
        let done =
            unsafe { with_event(ev, |e| status = o.run(|w| w.event(e))) };
        if done.is_none() { EXI_E_ARG } else { status }
    })
}

/// # Safety
///
/// `w` is live; the strings are valid.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut cfg = writer_config(&byte);
///     cfg.format = EXI_FORMAT_XML;
///     let w = exi_writer_new(&cfg);
///     let xml = |w| { let (mut p, mut n) = (null(), 0); assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK); String::from_utf8(core::slice::from_raw_parts(p, n).to_vec()).unwrap() };
///     assert_eq!(exi_writer_start_element(w, s("urn:x"), s("a")), EXI_OK);
///     assert_eq!(exi_writer_end_element(w), EXI_OK);
///     assert_eq!(exi_writer_end_document(w), EXI_OK);
///     assert_eq!(xml(w), "<a xmlns=\"urn:x\"/>");
///     assert_eq!(exi_writer_start_element(w, s(""), exi_str { ptr: null(), len: 1 }), EXI_E_ARG);
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_start_element(
    w: *mut exi_writer,
    uri: exi_str,
    local: exi_str,
) -> exi_status {
    on_writer(w, |o| {
        // SAFETY: the caller guarantees the strings.
        let (Some(u), Some(l)) =
            (unsafe { str_of(uri) }, unsafe { str_of(local) })
        else {
            return EXI_E_ARG;
        };
        o.run(|w| w.event(&crate::Event::StartElement(crate::Name::new(u, l))))
    })
}

/// # Safety
///
/// `w` is live; the strings are valid.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut cfg = writer_config(&byte);
///     cfg.format = EXI_FORMAT_XML;
///     let w = exi_writer_new(&cfg);
///     let xml = |w| { let (mut p, mut n) = (null(), 0); assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK); String::from_utf8(core::slice::from_raw_parts(p, n).to_vec()).unwrap() };
///     exi_writer_start_element(w, s(""), s("a"));
///     assert_eq!(exi_writer_attribute(w, s(""), s("k"), s("1 < 2")), EXI_OK);
///     exi_writer_end_element(w);
///     exi_writer_end_document(w);
///     assert_eq!(xml(w), "<a k=\"1 &lt; 2\"/>");
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_attribute(
    w: *mut exi_writer,
    uri: exi_str,
    local: exi_str,
    text: exi_str,
) -> exi_status {
    on_writer(w, |o| {
        // SAFETY: the caller guarantees the strings.
        let (Some(u), Some(l), Some(t)) =
            (unsafe { str_of(uri) }, unsafe { str_of(local) }, unsafe {
                str_of(text)
            })
        else {
            return EXI_E_ARG;
        };
        o.run(|w| {
            w.event(&crate::Event::Attribute(
                crate::Name::new(u, l),
                crate::Value::Str(t),
            ))
        })
    })
}

/// # Safety
///
/// `w` is live; `text` is valid.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut cfg = writer_config(&byte);
///     cfg.format = EXI_FORMAT_XML;
///     let w = exi_writer_new(&cfg);
///     let xml = |w| { let (mut p, mut n) = (null(), 0); assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK); String::from_utf8(core::slice::from_raw_parts(p, n).to_vec()).unwrap() };
///     exi_writer_start_element(w, s(""), s("a"));
///     assert_eq!(exi_writer_text(w, s("x & y")), EXI_OK);
///     exi_writer_end_element(w);
///     exi_writer_end_document(w);
///     assert_eq!(xml(w), "<a>x &amp; y</a>");
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_text(
    w: *mut exi_writer,
    text: exi_str,
) -> exi_status {
    on_writer(w, |o| {
        // SAFETY: the caller guarantees the string.
        let Some(t) = (unsafe { str_of(text) }) else { return EXI_E_ARG };
        o.run(|w| w.event(&crate::Event::Characters(crate::Value::Str(t))))
    })
}

/// # Safety
///
/// `w` is live.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let w = exi_writer_new(&writer_config(&byte));
///     assert_eq!(exi_writer_end_element(w), EXI_E_STATE);          // nothing open
///     assert_eq!(exi_writer_start_element(w, s(""), s("a")), EXI_E_STATE);   // poisoned
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_end_element(
    w: *mut exi_writer,
) -> exi_status {
    on_writer(w, |o| {
        o.run(|w| w.event(&crate::Event::EndElement(crate::Name::local(""))))
    })
}

/// ED, padding, flush.
///
/// # Safety
///
/// `w` is live.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let w = exi_writer_new(&writer_config(&byte));
///     exi_writer_start_element(w, s(""), s("a"));
///     assert_eq!(exi_writer_end_document(w), EXI_E_STATE);         // <a> is still open
///     exi_writer_free(w);
///     let w = exi_writer_new(&writer_config(&byte));
///     exi_writer_start_element(w, s(""), s("a"));
///     exi_writer_end_element(w);
///     assert_eq!(exi_writer_end_document(w), EXI_OK);
///     let (mut p, mut n) = (null(), 0);
///     exi_writer_take(w, &mut p, &mut n);
///     assert_eq!(core::slice::from_raw_parts(p, n), [0x80, 0x01, 0x02, b'a', 0x00]);
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_end_document(
    w: *mut exi_writer,
) -> exi_status {
    on_writer(w, |o| o.run(|w| w.event(&crate::Event::EndDocument)))
}

/// `*pending_bits` (may be NULL) gets the bits not yet written (0 when
/// byte-aligned).
///
/// # Safety
///
/// `w` is live.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut pending = 0;
///     #[cfg(feature = "bit-packed")]
///     {
///         let mut bits = byte;
///         bits.coding = EXI_CODING_BIT_PACKED;
///         let w = exi_writer_new(&writer_config(&bits));
///         exi_writer_start_element(w, s(""), s("a"));
///         assert_eq!(exi_writer_flush(w, &mut pending), EXI_OK);
///         assert!(pending > 0 && pending < 8); // mid-octet
///         exi_writer_free(w);
///     }
///     let b = exi_writer_new(&writer_config(&byte));
///     exi_writer_start_element(b, s(""), s("a"));
///     assert_eq!(exi_writer_flush(b, &mut pending), EXI_OK);
///     assert_eq!(pending, 0);                                      // byte-aligned
///     exi_writer_free(b);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_flush(
    w: *mut exi_writer,
    pending_bits: *mut u32,
) -> exi_status {
    on_writer(w, |o| {
        let mut bits = 0;
        let s = o.run(|w| match w {
            AnyWriter::Exi(x) => x.flush().map(|b| bits = b as u32),
            AnyWriter::Xml(_) => Ok(()),
        });
        if !pending_bits.is_null() {
            // SAFETY: the caller guarantees `pending_bits` is writable.
            unsafe { pending_bits.write(bits) };
        }
        s
    })
}

/// `EXI_SEQ_FRAMES`: after a top-level element ends, the octets since
/// the previous frame. Valid until the next call on the writer.
///
/// # Safety
///
/// `w` is live; `data` and `len` are writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let mut frag = byte;
///     frag.flags = EXI_OPT_FRAGMENT;
///     let mut cfg = writer_config(&frag);
///     cfg.sequence = EXI_SEQ_FRAMES;
///     let w = exi_writer_new(&cfg);
///     let (mut p, mut n) = (null(), 0);
///     exi_writer_start_element(w, s(""), s("m"));
///     assert_eq!(exi_writer_end_frame(w, &mut p, &mut n), EXI_E_STATE);    // <m> is open
///     exi_writer_free(w);
///     let w = exi_writer_new(&cfg);
///     for expect in [&[0x80u8, 0x00, 0x01, 0x02, b'm', 0x00][..], &[0x00, 0x00]] {
///         exi_writer_start_element(w, s(""), s("m"));
///         exi_writer_end_element(w);
///         assert_eq!(exi_writer_end_frame(w, &mut p, &mut n), EXI_OK);
///         assert_eq!(core::slice::from_raw_parts(p, n), expect);
///     }
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_end_frame(
    w: *mut exi_writer,
    data: *mut *const u8,
    len: *mut usize,
) -> exi_status {
    if data.is_null() || len.is_null() {
        return EXI_E_ARG;
    }
    on_writer(w, |o| {
        let mut out: (*const u8, usize) = (core::ptr::null(), 0);
        let s = o.run(|w| match w {
            AnyWriter::Exi(x) => {
                x.end_frame().map(|f| out = (f.as_ptr(), f.len()))
            }
            AnyWriter::Xml(_) => Err(crate::Error::new(
                crate::ErrorKind::State,
                "frames are EXI only",
            )),
        });
        // SAFETY: the caller guarantees writable outputs.
        unsafe {
            data.write(out.0);
            len.write(out.1);
        }
        s
    })
}

/// With `write` NULL: the output accumulated so far, valid until the
/// next call; the buffer is emptied.
///
/// # Safety
///
/// `w` is live; `data` and `len` are writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     extern "C" fn sink(_: *mut core::ffi::c_void, _: *const u8, _: usize) -> exi_status { EXI_OK }
///     let mut cfg = writer_config(&byte);
///     cfg.write = Some(sink);
///     let w = exi_writer_new(&cfg);
///     let (mut p, mut n) = (null(), 0);
///     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_E_STATE); // output goes to `sink`
///     exi_writer_free(w);
///     let w = exi_writer_new(&writer_config(&byte));
///     exi_writer_start_element(w, s(""), s("a"));
///     exi_writer_flush(w, null_mut());
///     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
///     assert_eq!(n, 4);                                            // header and SE(a)
///     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
///     assert_eq!(n, 0);                                            // emptied
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_take(
    w: *mut exi_writer,
    data: *mut *const u8,
    len: *mut usize,
) -> exi_status {
    if data.is_null() || len.is_null() {
        return EXI_E_ARG;
    }
    on_writer(w, |o| {
        let WriterObj { writer, taken, .. } = o;
        match writer.sink() {
            CSink::Buffer(v) => {
                taken.clear();
                core::mem::swap(taken, v);
            }
            CSink::Callback(..) => return EXI_E_STATE,
        }
        // SAFETY: the caller guarantees writable outputs.
        unsafe {
            data.write(taken.as_ptr());
            len.write(taken.len());
        }
        EXI_OK
    })
}

/// The last error's status and message.
///
/// # Safety
///
/// `w` is live; `msg` is NULL or writable.
///
/// ```
/// use exi::capi::*;
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_api.rs")); } use fixture::*; #[allow(unused_variables)] let byte = byte();
/// unsafe {
///     let w = exi_writer_new(&writer_config(&byte));
///     assert_eq!(exi_writer_error(w, null_mut()), EXI_OK);         // none yet
///     exi_writer_end_element(w);
///     let mut msg = null();
///     assert_eq!(exi_writer_error(w, &mut msg), EXI_E_STATE);
///     assert!(!core::ffi::CStr::from_ptr(msg).to_bytes().is_empty());
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_error(
    w: *const exi_writer,
    msg: *mut *const c_char,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees a live writer.
        let Some(o) = (unsafe { writer(w as *mut exi_writer) }) else {
            return EXI_E_ARG;
        };
        let Some(e) = &o.last else { return EXI_OK };
        if !msg.is_null() {
            // SAFETY: the caller guarantees `msg` is writable.
            unsafe { msg.write(o.last_msg.as_ptr()) };
        }
        status_of(e.kind())
    })
}
