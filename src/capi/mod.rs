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
//! `include/exi.h` is generated from this module by cbindgen
//! (`cbindgen.toml`) and committed; `tests/c_api.rs` checks it.
//! Names, numbers and layouts here are the ABI. Layer 2, the document
//! tree, is [`dom`], declared by the hand-written `include/exi_dom.h`.
//!
//! # Conventions
//!
//! | Rule | Detail |
//! |---|---|
//! | Status codes | `exi_status` (`int32_t`): `EXI_OK` 0, `EXI_END` 1, `EXI_NEED_INPUT` 2, negative codes one per `ErrorKind` (see [`crate::ErrorKind`]) |
//! | Strings | `exi_str { ptr, len }`, UTF-8. Strings libexi returns also have a NUL at `ptr[len]` that `len` does not count (D32). Strings passed in need no NUL. An `exi_str` with `len` 0 may have a NULL `ptr` |
//! | Objects | opaque pointers. `_free(NULL)` is a no-op. Every handle starts with a magic tag; debug builds check it on each call and return `EXI_E_ARG` for a wrong or freed handle |
//! | Config structs | start with `uint32_t size` = `sizeof` the struct the caller was compiled with. They only grow at the end; fields beyond `size` read as zero, and zero means the default. Constants are plain integers: an unknown value is `EXI_E_ARG` |
//! | Panics | every export catches panics and returns `EXI_E_INTERNAL` (or NULL); the object is poisoned. Callbacks are plain `extern "C"` and must not unwind |
//! | Poisoning | after a fatal error every call on the object except `_free` returns `EXI_E_STATE`. `EXI_E_UNENCODABLE` is not fatal (D23) |
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

/// cbindgen:ignore
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

/// Runs an export: a panic poisons the object and returns
/// `EXI_E_INTERNAL`.
fn on_reader(
    r: *mut exi_reader,
    f: impl FnOnce(&mut ReaderObj) -> exi_status,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the export's contract.
        match unsafe { reader(r) } {
            Some(o) => {
                let p: *mut ReaderObj = o;
                let s = std::panic::catch_unwind(
                    std::panic::AssertUnwindSafe(|| f(o)),
                );
                // SAFETY: still live: only `_free` ends it.
                s.unwrap_or_else(|_| {
                    unsafe { (*p).poisoned = true };
                    EXI_E_INTERNAL
                })
            }
            None => EXI_E_ARG,
        }
    })
}

fn on_writer(
    w: *mut exi_writer,
    f: impl FnOnce(&mut WriterObj) -> exi_status,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the export's contract.
        match unsafe { writer(w) } {
            Some(o) => {
                let p: *mut WriterObj = o;
                let s = std::panic::catch_unwind(
                    std::panic::AssertUnwindSafe(|| f(o)),
                );
                s.unwrap_or_else(|_| {
                    // SAFETY: still live: only `_free` ends it.
                    unsafe { (*p).poisoned = true };
                    EXI_E_INTERNAL
                })
            }
            None => EXI_E_ARG,
        }
    })
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
pub const EXI_ABI_MINOR: u32 = 11;

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
pub const EXI_RD_START_TAG: u32 = 1 << 1;
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
            Ok(strings) => {
                std::boxed::Box::into_raw(std::boxed::Box::new(StringsObj {
                    magic: MAGIC_STRINGS,
                    strings,
                })) as *mut exi_strings
            }
            Err(_) => core::ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `s` is NULL or from `exi_strings_new`, and no reader or writer still
/// uses it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_strings_free(s: *mut exi_strings) {
    let p = s as *mut StringsObj;
    // SAFETY: the caller guarantees NULL or a live handle.
    if let Some(o) = unsafe { p.as_mut() } {
        if o.magic == MAGIC_STRINGS {
            o.magic = 0;
            // SAFETY: from Box::into_raw in `exi_strings_new`.
            guard((), || drop(unsafe { std::boxed::Box::from_raw(p) }));
        }
    }
}

// ---------- grammar sets ----------

/// The schema-less grammar set: static; retain and release do nothing
/// to it.
#[unsafe(no_mangle)]
pub extern "C" fn exi_grammar_set_schemaless() -> *mut exi_grammar_set {
    imp::schemaless() as *mut exi_grammar_set
}

/// Adds a reference; returns `g`.
///
/// # Safety
///
/// `g` is a live grammar set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_retain(
    g: *mut exi_grammar_set,
) -> *mut exi_grammar_set {
    if !g.is_null() {
        // SAFETY: the caller guarantees a live set.
        unsafe { imp::retain(g as *mut GrammarSetObj) };
    }
    g
}

/// Drops a reference; the set is freed at zero. NULL is a no-op.
///
/// # Safety
///
/// `g` is NULL or a live grammar set the caller holds a reference to.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_release(g: *mut exi_grammar_set) {
    if !g.is_null() {
        // SAFETY: the caller guarantees a live set and a reference.
        unsafe { imp::release(g as *mut GrammarSetObj) };
    }
}

/// The fixed numbers of a declared name; `EXI_E_ARG` when it is not an
/// initial entry.
///
/// # Safety
///
/// `g` is live; `uri_id` and `local_id` are writable.
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
        let (Some(o), Some(u), Some(l)) = (
            unsafe { (g as *const GrammarSetObj).as_ref() },
            unsafe { str_of(uri) },
            unsafe { str_of(local) },
        ) else {
            return EXI_E_ARG;
        };
        if o.magic != MAGIC_GRAMMARS {
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
        // SAFETY: the caller guarantees `data` for `len` bytes.
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(data, len) }
        };
        match crate::grammar::GrammarSet::from_exig(bytes) {
            Ok(set) => {
                let obj = std::boxed::Box::new(GrammarSetObj {
                    magic: MAGIC_GRAMMARS,
                    refs: core::sync::atomic::AtomicUsize::new(1),
                    is_static: false,
                    set,
                });
                // SAFETY: the caller guarantees `out` is writable.
                unsafe {
                    out.write(
                        std::boxed::Box::into_raw(obj) as *mut exi_grammar_set
                    )
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_to_exig(
    g: *const exi_grammar_set,
    write: Option<extern "C" fn(*mut c_void, *const u8, usize) -> exi_status>,
    user: *mut c_void,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees `g`.
        let (Some(o), Some(write)) =
            (unsafe { (g as *const GrammarSetObj).as_ref() }, write)
        else {
            return EXI_E_ARG;
        };
        if o.magic != MAGIC_GRAMMARS {
            return EXI_E_ARG;
        }
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_grammar_set_digest(
    g: *const exi_grammar_set,
    out: *mut c_char,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller guarantees `g`.
        let Some(o) = (unsafe { (g as *const GrammarSetObj).as_ref() }) else {
            return EXI_E_ARG;
        };
        if o.magic != MAGIC_GRAMMARS || out.is_null() {
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
            Some(o) => std::boxed::Box::into_raw(std::boxed::Box::new(o))
                as *mut exi_reader,
            None => core::ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `r` is NULL or a live reader.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_free(r: *mut exi_reader) {
    // SAFETY: the caller guarantees NULL or a live reader.
    if unsafe { reader(r) }.is_some() {
        // SAFETY: from Box::into_raw in `exi_reader_new`.
        guard((), || {
            drop(unsafe { std::boxed::Box::from_raw(r as *mut ReaderObj) })
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
        // SAFETY: the caller guarantees `data` for `len` bytes.
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
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
/// the reader lives.
///
/// # Safety
///
/// `r` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_reader_options(
    r: *const exi_reader,
) -> *const exi_options {
    guard(core::ptr::null(), || {
        // SAFETY: the caller guarantees a live reader.
        let Some(o) = (unsafe { reader(r as *mut exi_reader) }) else {
            return core::ptr::null();
        };
        if o.options.is_none() {
            let mut keep = None;
            let c = o.reader.options().map(|opts| options_to(opts, &mut keep));
            o.options = c;
            o.options_keep = keep;
        }
        o.options
            .as_ref()
            .map_or(core::ptr::null(), |c| c as *const exi_options)
    })
}

/// The last error: its status; `*msg`, `*byte_offset`, `*event_index`
/// when not NULL.
///
/// # Safety
///
/// `r` is live; out pointers are NULL or writable.
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
            Some(o) => std::boxed::Box::into_raw(std::boxed::Box::new(o))
                as *mut exi_writer,
            None => core::ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `w` is NULL or a live writer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exi_writer_free(w: *mut exi_writer) {
    // SAFETY: the caller guarantees NULL or a live writer.
    if unsafe { writer(w) }.is_some() {
        // SAFETY: from Box::into_raw in `exi_writer_new`.
        guard((), || {
            drop(unsafe { std::boxed::Box::from_raw(w as *mut WriterObj) })
        });
    }
}

/// Any event, as the reader returns them.
///
/// # Safety
///
/// `w` is live; `ev` points to a filled `exi_event`.
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
