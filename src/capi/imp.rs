// src/capi/imp.rs
//
// C handles and Rust type conversions
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Ownership and lifetimes
//!
//! A reader or writer block owns, and drops in this order: the Rust
//! reader or writer, the `Grammars` (the grammar set attached to string
//! tables), then its private `GlobalStrings` when the config gave none,
//! and finally its reference to the grammar set. The Rust types borrow
//! each other with lifetimes; inside the block those lifetimes
//! are erased to `'static`. That is sound because every borrowed part
//! lives in its own heap allocation that the block creates, never
//! moves, and frees only after everything borrowing it has been
//! dropped. The parts are held as raw pointers (not `Box`) so that
//! moving the block never asserts uniqueness over memory that is
//! borrowed.

use std::boxed::Box;
use std::ffi::CString;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::vec::Vec;

use core::ffi::{c_char, c_void};

use super::*;
use crate::coding::Sink;
use crate::event::{Doctype, Event, NsDecl};
use crate::grammar::{GrammarSet, Grammars};
use crate::name::Name;
use crate::reader::{Reader, ReaderConfig, Sequence};
use crate::strings::{GlobalConfig, GlobalStrings};
use crate::value::{
    BinaryKind, DateTime, DateTimeKind, Decimal, Float, OwnedValue, Value,
};
use crate::writer::{Writer, WriterConfig};
use crate::xml::{
    EntityPolicy, PrefixPolicy, XmlConfig, XmlReader, XmlWriter,
    XmlWriterConfig,
};
use crate::{
    Coding, Error, ErrorKind, Header, Limits, Options, Preserve, SchemaId,
    Step,
};

pub(super) const MAGIC_STRINGS: u32 = 0x4558_4953; // "EXIS"
pub(super) const MAGIC_GRAMMARS: u32 = 0x4558_4947; // "EXIG"
pub(super) const MAGIC_READER: u32 = 0x4558_4952; // "EXIR"
pub(super) const MAGIC_WRITER: u32 = 0x4558_4957; // "EXIW"
const MAGIC_FREED: u32 = 0xDEAD_0000;

pub(super) fn status_of(kind: ErrorKind) -> exi_status {
    match kind {
        ErrorKind::Arg => EXI_E_ARG,
        ErrorKind::State => EXI_E_STATE,
        ErrorKind::Io => EXI_E_IO,
        ErrorKind::Header => EXI_E_HEADER,
        ErrorKind::Grammar => EXI_E_GRAMMAR,
        ErrorKind::Value => EXI_E_VALUE,
        ErrorKind::Truncated => EXI_E_TRUNCATED,
        ErrorKind::Limit => EXI_E_LIMIT,
        ErrorKind::Schema => EXI_E_SCHEMA,
        ErrorKind::Unsupported => EXI_E_UNSUPPORTED,
        ErrorKind::Xml => EXI_E_XML,
        ErrorKind::BufferFull => EXI_E_BUFFER_FULL,
        ErrorKind::Unencodable => EXI_E_UNENCODABLE,
        ErrorKind::NoMem => EXI_E_NOMEM,
        ErrorKind::Aborted => EXI_E_ABORTED,
        ErrorKind::Internal => EXI_E_INTERNAL,
    }
}

/// Runs `f`, turning a panic into `on_panic`.
pub(super) fn guard<T>(on_panic: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or(on_panic)
}

/// A C string view as `&str`; `None` when it is not UTF-8.
///
/// # Safety
///
/// `s.ptr` is valid for `s.len` bytes, or `s.len` is 0.
pub(super) unsafe fn str_of<'a>(s: exi_str) -> Option<&'a str> {
    if s.len == 0 {
        return Some("");
    }
    if s.ptr.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    let bytes =
        unsafe { core::slice::from_raw_parts(s.ptr.cast::<u8>(), s.len) };
    core::str::from_utf8(bytes).ok()
}

/// Copies a config struct the caller compiled with `size` bytes into a
/// zeroed full one.
///
/// # Safety
///
/// `p` points to at least `size` readable bytes, where `size` is its
/// first `u32`.
pub(super) unsafe fn read_sized<T: Copy>(p: *const T) -> Option<T> {
    if p.is_null() {
        return None;
    }
    // SAFETY: every config struct starts with `uint32_t size`.
    let size = unsafe { *(p as *const u32) } as usize;
    // SAFETY: all fields are integers, pointers or Options of fn
    // pointers: zero is valid.
    let mut out: T = unsafe { core::mem::zeroed() };
    let n = size.min(core::mem::size_of::<T>());
    // SAFETY: `p` has `size` readable bytes; `out` has
    // `size_of::<T>()`.
    unsafe {
        core::ptr::copy_nonoverlapping(
            p as *const u8,
            &mut out as *mut T as *mut u8,
            n,
        )
    };
    Some(out)
}

// ----- options and limits -----

pub(super) fn default_options() -> exi_options {
    exi_options {
        size: core::mem::size_of::<exi_options>() as u32,
        coding: EXI_CODING_BIT_PACKED,
        flags: 0,
        preserve: 0,
        block_size: 1_000_000,
        value_max_length: EXI_ABSENT,
        value_partition_capacity: EXI_ABSENT,
        schema_id_kind: EXI_SCHEMA_ID_ABSENT,
        _pad: 0,
        schema_id: exi_str { ptr: core::ptr::null(), len: 0 },
    }
}

/// # Safety
///
/// The schema id string of `o` is valid.
pub(super) unsafe fn options_from(
    o: &exi_options,
) -> core::result::Result<Options, &'static core::ffi::CStr> {
    let coding = match o.coding {
        EXI_CODING_BIT_PACKED => Coding::BitPacked,
        EXI_CODING_BYTE_ALIGNED => Coding::ByteAligned,
        EXI_CODING_PRE_COMPRESSION => Coding::PreCompression,
        EXI_CODING_COMPRESSION => Coding::Compression,
        _ => return Err(c"unknown coding constant"),
    };
    if o.flags & !(EXI_OPT_STRICT | EXI_OPT_FRAGMENT | EXI_OPT_SELF_CONTAINED)
        != 0
        || o.preserve & !0x1F != 0
    {
        return Err(c"unknown flag bits");
    }
    let mut p = Preserve::empty();
    for (bit, f) in [
        (EXI_PRESERVE_DTD, Preserve::DTD),
        (EXI_PRESERVE_PREFIXES, Preserve::PREFIXES),
        (EXI_PRESERVE_LEXICAL, Preserve::LEXICAL_VALUES),
        (EXI_PRESERVE_COMMENTS, Preserve::COMMENTS),
        (EXI_PRESERVE_PIS, Preserve::PIS),
    ] {
        if o.preserve & bit != 0 {
            p = p | f;
        }
    }
    let opt_u32 = |v: u64| -> core::result::Result<
        Option<u32>,
        &'static core::ffi::CStr,
    > {
        match v {
            EXI_ABSENT => Ok(None),
            v => u32::try_from(v)
                .map(Some)
                .map_err(|_| c"a value beyond unsignedInt"),
        }
    };
    let block = match o.block_size {
        0 => 1_000_000,
        v => u32::try_from(v).map_err(|_| c"blockSize beyond unsignedInt")?,
    };
    let schema_id = match o.schema_id_kind {
        EXI_SCHEMA_ID_ABSENT => SchemaId::Absent,
        EXI_SCHEMA_ID_NIL => SchemaId::Nil,
        EXI_SCHEMA_ID_BUILTIN => SchemaId::BuiltinTypes,
        EXI_SCHEMA_ID_VALUE => {
            // SAFETY: the caller guarantees the string.
            let s = unsafe { str_of(o.schema_id) }
                .ok_or(c"schemaId is not UTF-8")?;
            if s.is_empty() {
                SchemaId::BuiltinTypes
            } else {
                SchemaId::Id(s.into())
            }
        }
        _ => return Err(c"unknown schemaId kind"),
    };
    Options::builder()
        .coding(coding)
        .strict(o.flags & EXI_OPT_STRICT != 0)
        .fragment(o.flags & EXI_OPT_FRAGMENT != 0)
        .self_contained(o.flags & EXI_OPT_SELF_CONTAINED != 0)
        .preserve(p)
        .block_size(block)
        .value_max_length(opt_u32(o.value_max_length)?)
        .value_partition_capacity(opt_u32(o.value_partition_capacity)?)
        .schema_id(schema_id)
        .build()
        .map_err(|_| c"options that cannot be combined: strict with preserved DTDs, prefixes, comments, PIs or selfContained; selfContained with compression; or blockSize 0")
}

/// The C form of `o`; the schema id text is kept in `keep`.
pub(super) fn options_to(
    o: &Options,
    keep: &mut Option<CString>,
) -> exi_options {
    let mut c = default_options();
    c.coding = match o.coding() {
        Coding::BitPacked => EXI_CODING_BIT_PACKED,
        Coding::ByteAligned => EXI_CODING_BYTE_ALIGNED,
        Coding::PreCompression => EXI_CODING_PRE_COMPRESSION,
        Coding::Compression => EXI_CODING_COMPRESSION,
    };
    c.flags = (o.strict() as u32 * EXI_OPT_STRICT)
        | (o.fragment() as u32 * EXI_OPT_FRAGMENT)
        | (o.self_contained() as u32 * EXI_OPT_SELF_CONTAINED);
    c.preserve = o.preserve().bits() as u32;
    c.block_size = o.block_size() as u64;
    c.value_max_length = o.value_max_length().map_or(EXI_ABSENT, |v| v as u64);
    c.value_partition_capacity =
        o.value_partition_capacity().map_or(EXI_ABSENT, |v| v as u64);
    match o.schema_id() {
        SchemaId::Absent => c.schema_id_kind = EXI_SCHEMA_ID_ABSENT,
        SchemaId::Nil => c.schema_id_kind = EXI_SCHEMA_ID_NIL,
        SchemaId::BuiltinTypes => c.schema_id_kind = EXI_SCHEMA_ID_BUILTIN,
        SchemaId::Id(s) => {
            c.schema_id_kind = EXI_SCHEMA_ID_VALUE;
            let cs = CString::new(s.as_bytes()).unwrap_or_default();
            c.schema_id = exi_str { ptr: cs.as_ptr(), len: s.len() };
            *keep = Some(cs);
        }
    }
    c
}

pub(super) fn default_limits() -> exi_limits {
    let l = Limits::new();
    exi_limits {
        size: core::mem::size_of::<exi_limits>() as u32,
        max_depth: l.get_max_depth(),
        max_attributes: l.get_max_attributes(),
        max_integer_octets: l.get_max_integer_octets(),
        max_names: l.get_max_names(),
        max_list_items: l.get_max_list_items(),
        max_options_bytes: l.get_max_options_bytes(),
        max_entity_depth: l.get_max_entity_depth(),
        max_value_chars: l.get_max_value_chars(),
        max_binary_bytes: l.get_max_binary_bytes(),
        max_string_table_bytes: l.get_max_string_table_bytes(),
        max_learned_productions: l.get_max_learned_productions(),
        max_expanded_bytes: l.get_max_expanded_bytes(),
    }
}

/// Zero fields mean the default.
pub(super) fn limits_from(c: &exi_limits) -> Limits {
    let mut l = Limits::new();
    macro_rules! set {
        ($($f:ident),*) => { $( if c.$f != 0 { l = l.$f(c.$f); } )* };
    }
    set!(
        max_depth,
        max_attributes,
        max_integer_octets,
        max_names,
        max_list_items,
        max_options_bytes,
        max_entity_depth,
        max_value_chars,
        max_binary_bytes,
        max_string_table_bytes,
        max_learned_productions,
        max_expanded_bytes
    );
    l
}

// ----- string tables and grammar sets -----

pub(super) struct StringsObj {
    pub magic: u32,
    pub strings: GlobalStrings,
}

pub(super) struct GrammarSetObj {
    pub magic: u32,
    pub refs: AtomicUsize,
    pub is_static: bool,
    pub set: GrammarSet,
}

// SAFETY: a GrammarSet is immutable and Send + Sync; the count is
// atomic.
unsafe impl Sync for GrammarSetObj {}

pub(super) fn schemaless() -> *mut GrammarSetObj {
    static SET: OnceLock<GrammarSetObj> = OnceLock::new();
    let g = SET.get_or_init(|| GrammarSetObj {
        magic: MAGIC_GRAMMARS,
        refs: AtomicUsize::new(1),
        is_static: true,
        set: GrammarSet::schemaless(),
    });
    g as *const GrammarSetObj as *mut GrammarSetObj
}

/// # Safety
///
/// `g` is a live grammar set.
pub(super) unsafe fn retain(g: *mut GrammarSetObj) {
    // SAFETY: the caller guarantees `g` is live.
    let r = unsafe { &*g };
    if !r.is_static {
        r.refs.fetch_add(1, Ordering::Relaxed);
    }
}

/// # Safety
///
/// `g` is a live grammar set the caller holds a reference to.
pub(super) unsafe fn release(g: *mut GrammarSetObj) {
    // SAFETY: the caller guarantees `g` is live.
    let r = unsafe { &*g };
    if r.is_static {
        return;
    }
    if r.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        // SAFETY: the last reference: nothing else uses it; it came
        // from Box::into_raw.
        drop(unsafe { Box::from_raw(g) });
    }
}

/// The parts a reader or writer block owns besides the Rust reader or
/// writer.
pub(super) struct Parts {
    /// From `Box::into_raw`, or borrowed from an `exi_strings` handle.
    strings: *mut GlobalStrings,
    owns_strings: bool,
    set: *mut GrammarSetObj,
    grammars: *mut Grammars<'static>,
}

impl Parts {
    /// # Safety
    ///
    /// `strings` is NULL or a live `exi_strings` handle; `set` is NULL
    /// or a live grammar set. Both outlive the block.
    pub unsafe fn new(
        strings: *mut exi_strings,
        set: *mut exi_grammar_set,
    ) -> Option<Parts> {
        let (strings, owns) = if strings.is_null() {
            (
                Box::into_raw(Box::new(
                    GlobalStrings::new(GlobalConfig::new()).ok()?,
                )),
                true,
            )
        } else {
            let s = strings as *mut StringsObj;
            // SAFETY: the caller guarantees a live handle.
            if unsafe { (*s).magic } != MAGIC_STRINGS {
                return None;
            }
            // SAFETY: as above.
            (unsafe { &mut (*s).strings } as *mut GlobalStrings, false)
        };
        let set = if set.is_null() {
            schemaless()
        } else {
            set as *mut GrammarSetObj
        };
        // SAFETY: the caller guarantees a live set; the static one is
        // always live.
        if unsafe { (*set).magic } != MAGIC_GRAMMARS {
            if owns {
                // SAFETY: created above.
                drop(unsafe { Box::from_raw(strings) });
            }
            return None;
        }
        // SAFETY: as above.
        unsafe { retain(set) };
        // SAFETY: both pointees live until `Drop for Parts` frees them,
        // after the reader or writer that borrows them (the block drops
        // that first).
        let (s_ref, g_ref): (&'static GlobalStrings, &'static GrammarSet) =
            unsafe { (&*strings, &(*set).set) };
        match g_ref.attach(s_ref) {
            Ok(g) => Some(Parts {
                strings,
                owns_strings: owns,
                set,
                grammars: Box::into_raw(Box::new(g)),
            }),
            Err(_) => {
                // SAFETY: undo what was done above.
                unsafe { release(set) };
                if owns {
                    drop(unsafe { Box::from_raw(strings) });
                }
                None
            }
        }
    }

    pub fn grammars(&self) -> &'static Grammars<'static> {
        // SAFETY: see the module documentation.
        unsafe { &*self.grammars }
    }
}

impl Drop for Parts {
    fn drop(&mut self) {
        // SAFETY: created in `new`; the reader or writer using them is
        // already dropped.
        unsafe {
            drop(Box::from_raw(self.grammars));
            if self.owns_strings {
                drop(Box::from_raw(self.strings));
            }
            release(self.set);
        }
    }
}

// ----- readers -----

macro_rules! any_reader {
    ($($v:ident $x:ident $n:literal),*) => {
        // The ring is inline; the whole block is on the heap, so the size spread costs nothing.
        #[allow(clippy::large_enum_variant)]
        pub(super) enum AnyReader {
            $( $v(Reader<'static, $n>), $x(XmlReader<'static, $n>), )*
        }
        macro_rules! with_reader {
            ($r:expr, $b:ident => $e:expr) => { match $r { $( AnyReader::$v($b) => $e, AnyReader::$x($b) => $e, )* } };
        }
        impl AnyReader {
            pub fn new(size: u32, xml: bool, cfg: ReaderConfig<'static>, xcfg: XmlConfig, g: &'static Grammars<'static>) -> Option<crate::Result<AnyReader>> {
                Some(match (size, xml) {
                    $( ($n, false) => Reader::new(cfg).map(AnyReader::$v), ($n, true) => Ok(AnyReader::$x(XmlReader::new(xcfg, g))), )*
                    _ => return None,
                })
            }
            pub fn feed(&mut self, d: &[u8]) -> usize { with_reader!(self, r => r.feed(d)) }
            pub fn buffer(&mut self) -> &mut [u8] { with_reader!(self, r => r.buffer()) }
            pub fn commit(&mut self, n: usize) { with_reader!(self, r => r.commit(n)) }
            pub fn end_input(&mut self) { with_reader!(self, r => r.end_input()) }
            pub fn next(&mut self) -> crate::Result<Step<'_>> { with_reader!(self, r => r.next()) }
            pub fn end_frame(&mut self) {
                match self { $( AnyReader::$v(r) => r.end_frame(), )* _ => {} }
            }
            pub fn options(&self) -> Option<&Options> {
                match self { $( AnyReader::$v(r) => r.options(), )* _ => None }
            }
        }
    };
}

any_reader!(E64 X64 64, E256 X256 256, E1K X1K 1024, E4K X4K 4096, E16K X16K 16384, E64K X64K 65536);

/// Storage for one event's C view, valid until the next call.
#[derive(Default)]
pub(super) struct Scratch {
    strings: Vec<Box<[u8]>>,
    values: Vec<Box<[exi_value]>>,
    bytes: Vec<Box<[u8]>>,
    ns: Option<Box<exi_nsdecl>>,
}

impl Scratch {
    pub fn clear(&mut self) {
        self.strings.clear();
        self.values.clear();
        self.bytes.clear();
        self.ns = None;
    }

    /// A NUL-terminated copy.
    pub fn str(&mut self, s: &str) -> exi_str {
        let mut b = Vec::with_capacity(s.len() + 1);
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        let b = b.into_boxed_slice();
        let ptr = b.as_ptr().cast::<c_char>();
        self.strings.push(b);
        exi_str { ptr, len: s.len() }
    }

    pub fn name(&mut self, n: Name<'_>) -> exi_name {
        let (uri_id, local_id) =
            n.id().map_or((EXI_NO_ID, EXI_NO_ID), |i| (i.uri, i.local));
        let (global_uri, global_local) =
            n.global().map_or((EXI_NO_ID, EXI_NO_ID), |g| {
                (g.uri.index(), g.local.index())
            });
        exi_name {
            uri: self.str(n.uri()),
            local: self.str(n.local_name()),
            prefix: self.str(n.prefix().unwrap_or("")),
            uri_id,
            local_id,
            global_uri,
            global_local,
        }
    }

    pub fn value(&mut self, v: Value<'_>, lexical: bool) -> exi_value {
        // SAFETY: every union member is plain data; zero is a valid
        // value of each.
        let mut out: exi_value = unsafe { core::mem::zeroed() };
        if lexical {
            out.flags |= EXI_VF_LEXICAL;
        }
        match v {
            Value::Str(s) => {
                out.r#type = EXI_V_STRING;
                out.text = self.str(s);
            }
            Value::Bool(b) | Value::BoolDigit(b) => {
                out.r#type = EXI_V_BOOL;
                out.u.boolean = b as i32;
                if matches!(v, Value::BoolDigit(_)) {
                    out.flags |= EXI_VF_BOOL_DIGIT;
                }
            }
            Value::Int(i) => {
                out.r#type = EXI_V_INT;
                out.u.integer = i;
            }
            Value::BigInt(s) => {
                out.r#type = EXI_V_INT;
                out.flags |= EXI_VF_TEXT_ONLY;
                out.text = self.str(s);
            }
            Value::Decimal(d) => {
                out.r#type = EXI_V_DECIMAL;
                out.u.dec = exi_decimal {
                    negative: d.negative as u32,
                    fraction_digits: d.fraction_digits as u32,
                    integral: d.integral,
                    fraction: d.fraction,
                };
            }
            Value::BigDecimal(s) => {
                out.r#type = EXI_V_DECIMAL;
                out.flags |= EXI_VF_TEXT_ONLY;
                out.text = self.str(s);
            }
            Value::Float(f) => {
                out.r#type = EXI_V_FLOAT;
                out.u.flt = exi_float {
                    mantissa: f.mantissa,
                    exponent: f.exponent as i32,
                    _pad: 0,
                };
            }
            Value::Binary(b, kind) => {
                out.r#type = EXI_V_BINARY;
                if kind == BinaryKind::Hex {
                    out.flags |= EXI_VF_HEX;
                }
                let copy: Box<[u8]> = b.into();
                out.u.binary =
                    exi_bytes { ptr: copy.as_ptr(), len: copy.len() };
                self.bytes.push(copy);
            }
            Value::DateTime(d) => {
                out.r#type = EXI_V_DATETIME;
                out.u.datetime = datetime_to(&d);
            }
            Value::QName(n) => {
                out.r#type = EXI_V_QNAME;
                out.u.qname = self.name(n);
            }
            Value::List(items) => {
                out.r#type = EXI_V_LIST;
                let list: Box<[exi_value]> =
                    items.iter().map(|x| self.value(x, false)).collect();
                out.u.list =
                    exi_list { items: list.as_ptr(), count: list.len() };
                self.values.push(list);
            }
            Value::Enum { ordinal, text } => {
                out.r#type = EXI_V_ENUM;
                out.u.enum_ordinal = ordinal;
                out.text = self.str(text);
            }
        }
        out
    }
}

const KINDS: [DateTimeKind; 8] = [
    DateTimeKind::GYear,
    DateTimeKind::GYearMonth,
    DateTimeKind::Date,
    DateTimeKind::DateTime,
    DateTimeKind::GMonth,
    DateTimeKind::GMonthDay,
    DateTimeKind::GDay,
    DateTimeKind::Time,
];

fn datetime_to(d: &DateTime) -> exi_datetime {
    exi_datetime {
        kind: KINDS
            .iter()
            .position(|k| *k == d.kind)
            .map_or(0, |i| i as u32 + 1),
        tz_minutes: d.tz_minutes.unwrap_or(0) as i32,
        year: d.year,
        month: d.month,
        day: d.day,
        hour: d.hour,
        minute: d.minute,
        second: d.second,
        has_tz: d.tz_minutes.is_some() as u8,
        has_frac: d.fraction.is_some() as u8,
        _pad: 0,
        frac_digits: d.fraction.map_or(0, |(_, n)| n as u32),
        frac: d.fraction.map_or(0, |(v, _)| v),
    }
}

fn datetime_from(c: &exi_datetime) -> Option<DateTime> {
    let kind = *KINDS.get((c.kind as usize).checked_sub(1)?)?;
    Some(DateTime {
        kind,
        year: c.year,
        month: c.month,
        day: c.day,
        hour: c.hour,
        minute: c.minute,
        second: c.second,
        fraction: (c.has_frac != 0).then_some((c.frac, c.frac_digits as u8)),
        tz_minutes: (c.has_tz != 0).then_some(c.tz_minutes as i16),
    })
}

/// Fills the C view of `ev`.
pub(super) fn event_to(
    sc: &mut Scratch,
    ev: &Event<'_>,
    depth: u32,
    lexical: bool,
) -> exi_event {
    // SAFETY: plain data; zero is valid for every field.
    let mut out: exi_event = unsafe { core::mem::zeroed() };
    out.depth = depth;
    let text = |sc: &mut Scratch, s: &str| {
        // SAFETY: as above.
        let mut v: exi_value = unsafe { core::mem::zeroed() };
        v.r#type = EXI_V_STRING;
        v.text = sc.str(s);
        v
    };
    match *ev {
        Event::StartDocument => out.r#type = EXI_EV_SD,
        Event::EndDocument => out.r#type = EXI_EV_ED,
        Event::StartElement(n)
        | Event::StartTag(crate::StartTag { name: n, .. }) => {
            out.r#type = EXI_EV_SE;
            out.name = sc.name(n);
        }
        Event::EndElement(n) => {
            out.r#type = EXI_EV_EE;
            out.name = sc.name(n);
        }
        Event::Attribute(n, v) => {
            out.r#type = EXI_EV_AT;
            out.name = sc.name(n);
            out.value = sc.value(v, lexical);
        }
        Event::Characters(v) => {
            out.r#type = EXI_EV_CH;
            out.value = sc.value(v, lexical);
        }
        Event::Namespace(ns) => {
            out.r#type = EXI_EV_NS;
            let d = Box::new(exi_nsdecl {
                prefix: sc.str(ns.prefix),
                uri: sc.str(ns.uri),
                local_element_ns: ns.local_element_ns as u32,
                _pad: 0,
            });
            out.ns = &*d;
            out.ns_count = 1;
            sc.ns = Some(d);
        }
        Event::Comment(c) => {
            out.r#type = EXI_EV_CM;
            out.value = text(sc, c);
        }
        Event::ProcessingInstruction { target, data } => {
            out.r#type = EXI_EV_PI;
            out.name.local = sc.str(target);
            out.value = text(sc, data);
        }
        Event::Doctype(d) => {
            out.r#type = EXI_EV_DT;
            out.name.local = sc.str(d.name);
            out.pubid = sc.str(d.public_id);
            out.sysid = sc.str(d.system_id);
            out.value = text(sc, d.text);
        }
        Event::EntityReference(n) => {
            out.r#type = EXI_EV_ER;
            out.name.local = sc.str(n);
        }
        Event::SelfContained => out.r#type = EXI_EV_SC,
    }
    out
}

pub(super) struct ReaderObj {
    pub magic: u32,
    /// Dropped before `parts`, which it borrows.
    pub reader: core::mem::ManuallyDrop<AnyReader>,
    pub parts: core::mem::ManuallyDrop<Parts>,
    pub poisoned: bool,
    pub last: Option<Error>,
    pub last_msg: CString,
    pub scratch: Scratch,
    pub options: Option<exi_options>,
    pub options_keep: Option<CString>,
}

impl Drop for ReaderObj {
    fn drop(&mut self) {
        self.magic = MAGIC_FREED;
        // SAFETY: dropped once, reader first.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut self.reader);
            core::mem::ManuallyDrop::drop(&mut self.parts);
        }
    }
}

impl ReaderObj {
    pub fn fail(&mut self, e: Error) -> exi_status {
        self.last_msg = CString::new(e.detail()).unwrap_or_default();
        let s = status_of(e.kind());
        self.last = Some(e);
        self.poisoned = true;
        s
    }

    /// The next event into `out`.
    pub fn next(&mut self, out: &mut exi_event) -> exi_status {
        if self.poisoned {
            return EXI_E_STATE;
        }
        self.scratch.clear();
        let step = self.reader.next();
        match step {
            Ok(Step::Event(ev, meta)) => {
                *out = event_to(
                    &mut self.scratch,
                    &ev,
                    meta.depth,
                    meta.flags.contains(crate::ValueFlags::LEXICAL),
                );
                EXI_OK
            }
            Ok(Step::NeedInput) => EXI_NEED_INPUT,
            Ok(Step::End) => EXI_END,
            Err(e) => self.fail(e),
        }
    }
}

// ----- writers -----

/// Where a C writer's output goes.
pub(super) enum CSink {
    /// The callback, its user pointer, and the status it last failed
    /// with.
    Callback(
        extern "C" fn(*mut c_void, *const u8, usize) -> exi_status,
        *mut c_void,
        Option<exi_status>,
    ),
    Buffer(Vec<u8>),
}

impl Sink for CSink {
    fn write(&mut self, octets: &[u8]) -> crate::Result<()> {
        match self {
            CSink::Callback(f, user, failed) => {
                match f(*user, octets.as_ptr(), octets.len()) {
                    EXI_OK => Ok(()),
                    s => {
                        *failed = Some(s);
                        Err(Error::new(
                            ErrorKind::Io,
                            "the write callback failed",
                        ))
                    }
                }
            }
            CSink::Buffer(v) => {
                v.extend_from_slice(octets);
                Ok(())
            }
        }
    }
}

#[allow(clippy::large_enum_variant)] // one per block, on the heap
pub(super) enum AnyWriter {
    Exi(Writer<'static, CSink>),
    Xml(XmlWriter<CSink>),
}

impl AnyWriter {
    pub fn event(&mut self, ev: &Event<'_>) -> crate::Result<()> {
        use crate::EventSink;
        match self {
            AnyWriter::Exi(w) => w.event(ev),
            AnyWriter::Xml(w) => w.event(ev),
        }
    }
    pub fn sink(&mut self) -> &mut CSink {
        match self {
            AnyWriter::Exi(w) => w.sink_mut(),
            AnyWriter::Xml(w) => w.sink_mut(),
        }
    }
}

pub(super) struct WriterObj {
    pub magic: u32,
    pub writer: core::mem::ManuallyDrop<AnyWriter>,
    pub parts: core::mem::ManuallyDrop<Parts>,
    pub poisoned: bool,
    pub last: Option<Error>,
    pub last_msg: CString,
    pub taken: Vec<u8>,
}

impl Drop for WriterObj {
    fn drop(&mut self) {
        self.magic = MAGIC_FREED;
        // SAFETY: dropped once, writer first.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut self.writer);
            core::mem::ManuallyDrop::drop(&mut self.parts);
        }
    }
}

impl WriterObj {
    pub fn run(
        &mut self,
        f: impl FnOnce(&mut AnyWriter) -> crate::Result<()>,
    ) -> exi_status {
        if self.poisoned {
            return EXI_E_STATE;
        }
        match f(&mut self.writer) {
            Ok(()) => EXI_OK,
            Err(e) => {
                self.last_msg = CString::new(e.detail()).unwrap_or_default();
                let mut s = status_of(e.kind());
                // A failed callback stops the writer with the
                // callback's own status.
                if let CSink::Callback(_, _, Some(cs)) = self.writer.sink() {
                    s = *cs;
                }
                if e.is_fatal() {
                    self.poisoned = true;
                }
                self.last = Some(e);
                s
            }
        }
    }
}

/// Builds a writer block.
///
/// # Safety
///
/// As `exi_writer_new`.
pub(super) unsafe fn writer_new(c: &exi_writer_config) -> Option<WriterObj> {
    if c.sequence > EXI_SEQ_FRAMES
        || c.format > EXI_FORMAT_XML
        || c.header & !(EXI_HDR_COOKIE | EXI_HDR_NO_OPTIONS) != 0
    {
        return None;
    }
    // SAFETY: the caller guarantees the pointers.
    let parts = unsafe { Parts::new(c.strings, c.grammars) }?;
    let g = parts.grammars();
    let sink = match c.write {
        Some(f) => CSink::Callback(f, c.write_user, None),
        None => CSink::Buffer(Vec::new()),
    };
    let writer = if c.format == EXI_FORMAT_XML {
        AnyWriter::Xml(XmlWriter::new(
            sink,
            XmlWriterConfig::new()
                .declaration(c.flags & EXI_WR_XML_DECLARATION != 0)
                .prefixes(PrefixPolicy::Generate),
        ))
    } else {
        // SAFETY: the caller guarantees the pointers.
        let o =
            unsafe { read_sized(c.options) }.unwrap_or_else(default_options);
        // SAFETY: as above.
        let opts = unsafe { options_from(&o) }.ok()?;
        // SAFETY: as above.
        let limits = unsafe { read_sized(c.limits) }
            .map_or_else(Limits::new, |l| limits_from(&l));
        let cookie = c.header & EXI_HDR_COOKIE != 0;
        let header = if c.header & EXI_HDR_NO_OPTIONS != 0 {
            Header::Bare { cookie }
        } else {
            Header::WithOptions { cookie }
        };
        let seq = [Sequence::Single, Sequence::Streams, Sequence::Frames]
            [c.sequence as usize];
        let cfg = WriterConfig::new(opts, g)
            .header(header)
            .sequence(seq)
            .limits(limits)
            .canonical(c.flags & EXI_WR_CANONICAL != 0)
            .utc_time(c.flags & EXI_WR_UTC_TIME != 0);
        AnyWriter::Exi(Writer::new(sink, cfg).ok()?)
    };
    Some(WriterObj {
        magic: MAGIC_WRITER,
        writer: core::mem::ManuallyDrop::new(writer),
        parts: core::mem::ManuallyDrop::new(parts),
        poisoned: false,
        last: None,
        last_msg: CString::default(),
        taken: Vec::new(),
    })
}

/// Builds a reader block.
///
/// # Safety
///
/// As `exi_reader_new`.
pub(super) unsafe fn reader_new(c: &exi_reader_config) -> Option<ReaderObj> {
    if c.sequence > EXI_SEQ_FRAMES
        || c.format > EXI_FORMAT_XML
        || c.flags
            & !(EXI_RD_START_TAG
                | EXI_RD_XML_DOCTYPE
                | EXI_RD_XML_PREDEFINED_ONLY)
            != 0
    {
        return None;
    }
    let size =
        if c.input_buffer_size == 0 { 4096 } else { c.input_buffer_size };
    if ![64, 256, 1024, 4096, 16384, 65536].contains(&size) {
        return None;
    }
    // SAFETY: the caller guarantees the pointers.
    let limits = unsafe { read_sized(c.limits) }
        .map_or_else(Limits::new, |l| limits_from(&l));
    // SAFETY: as above.
    let options = match unsafe { read_sized(c.options) } {
        // SAFETY: as above.
        Some(o) => Some(unsafe { options_from(&o) }.ok()?),
        None => None,
    };
    // SAFETY: as above.
    let parts = unsafe { Parts::new(c.strings, c.grammars) }?;
    let g = parts.grammars();
    let seq = [Sequence::Single, Sequence::Streams, Sequence::Frames]
        [c.sequence as usize];
    let mut cfg = ReaderConfig::new(g)
        .sequence(seq)
        .limits(limits)
        .start_tag(c.flags & EXI_RD_START_TAG != 0);
    if let Some(o) = options {
        cfg = cfg.options(o);
    }
    let entities = if c.flags & EXI_RD_XML_PREDEFINED_ONLY != 0 {
        EntityPolicy::PredefinedOnly
    } else {
        EntityPolicy::Internal
    };
    let xcfg = XmlConfig::new()
        .limits(limits)
        .entities(entities)
        .report_doctype(c.flags & EXI_RD_XML_DOCTYPE != 0);
    let reader =
        AnyReader::new(size, c.format == EXI_FORMAT_XML, cfg, xcfg, g)?
            .ok()?;
    Some(ReaderObj {
        magic: MAGIC_READER,
        reader: core::mem::ManuallyDrop::new(reader),
        parts: core::mem::ManuallyDrop::new(parts),
        poisoned: false,
        last: None,
        last_msg: CString::default(),
        scratch: Scratch::default(),
        options: None,
        options_keep: None,
    })
}

/// An event from its C form.
///
/// # Safety
///
/// Every string and pointer of `ev` its type uses is valid.
pub(super) unsafe fn with_event<T>(
    ev: &exi_event,
    f: impl FnOnce(&Event<'_>) -> T,
) -> Option<T> {
    // SAFETY: the caller guarantees the strings.
    let s = |x: exi_str| unsafe { str_of(x) };
    let name = |n: &exi_name| -> Option<Name<'_>> {
        let nm = Name::new(s(n.uri)?, s(n.local)?);
        let p = s(n.prefix)?;
        Some(if p.is_empty() { nm } else { nm.with_prefix(p) })
    };
    let mut owned: Option<OwnedValue> = None;
    // SAFETY: as above.
    let value =
        |v: &exi_value, owned: &mut Option<OwnedValue>| -> Option<()> {
            *owned = Some(unsafe { value_from(v) }?);
            Some(())
        };
    let e = match ev.r#type {
        EXI_EV_SD => Event::StartDocument,
        EXI_EV_ED => Event::EndDocument,
        EXI_EV_SE => Event::StartElement(name(&ev.name)?),
        EXI_EV_EE => Event::EndElement(name(&ev.name)?),
        EXI_EV_AT | EXI_EV_CH => {
            value(&ev.value, &mut owned)?;
            let v = owned.as_ref().expect("set above").as_value();
            let e = if ev.r#type == EXI_EV_AT {
                Event::Attribute(name(&ev.name)?, v)
            } else {
                Event::Characters(v)
            };
            return Some(f(&e));
        }
        EXI_EV_NS => {
            if ev.ns.is_null() || ev.ns_count != 1 {
                return None;
            }
            // SAFETY: the caller guarantees `ns`.
            let d = unsafe { &*ev.ns };
            Event::Namespace(NsDecl {
                prefix: s(d.prefix)?,
                uri: s(d.uri)?,
                local_element_ns: d.local_element_ns != 0,
            })
        }
        EXI_EV_CM => Event::Comment(s(ev.value.text)?),
        EXI_EV_PI => Event::ProcessingInstruction {
            target: s(ev.name.local)?,
            data: s(ev.value.text)?,
        },
        EXI_EV_DT => Event::Doctype(Doctype {
            name: s(ev.name.local)?,
            public_id: s(ev.pubid)?,
            system_id: s(ev.sysid)?,
            text: s(ev.value.text)?,
        }),
        EXI_EV_ER => Event::EntityReference(s(ev.name.local)?),
        EXI_EV_SC => Event::SelfContained,
        _ => return None,
    };
    Some(f(&e))
}

/// # Safety
///
/// The pointers and strings `v`'s type uses are valid.
pub(super) unsafe fn value_from(v: &exi_value) -> Option<OwnedValue> {
    // SAFETY: the caller guarantees the strings.
    let text = || unsafe { str_of(v.text) };
    let text_only = v.flags & EXI_VF_TEXT_ONLY != 0;
    // SAFETY: `type` selects the member the caller set.
    Some(unsafe {
        match v.r#type {
            EXI_V_NONE | EXI_V_STRING => OwnedValue::Str(text()?.into()),
            EXI_V_BOOL if v.flags & EXI_VF_BOOL_DIGIT != 0 => {
                OwnedValue::BoolDigit(v.u.boolean != 0)
            }
            EXI_V_BOOL => OwnedValue::Bool(v.u.boolean != 0),
            EXI_V_INT if text_only => OwnedValue::BigInt(text()?.into()),
            EXI_V_INT => OwnedValue::Int(v.u.integer),
            EXI_V_DECIMAL if text_only => {
                OwnedValue::BigDecimal(text()?.into())
            }
            EXI_V_DECIMAL => {
                let d = v.u.dec;
                OwnedValue::Decimal(Decimal::new(
                    d.negative != 0,
                    d.integral,
                    d.fraction,
                    u8::try_from(d.fraction_digits).ok()?,
                ))
            }
            EXI_V_FLOAT => OwnedValue::Float(Float {
                mantissa: v.u.flt.mantissa,
                exponent: i16::try_from(v.u.flt.exponent).ok()?,
            }),
            EXI_V_BINARY => {
                let b = v.u.binary;
                let data: &[u8] = if b.len == 0 {
                    &[]
                } else {
                    core::slice::from_raw_parts(b.ptr, b.len)
                };
                OwnedValue::Binary(
                    data.into(),
                    if v.flags & EXI_VF_HEX != 0 {
                        BinaryKind::Hex
                    } else {
                        BinaryKind::Base64
                    },
                )
            }
            EXI_V_DATETIME => {
                OwnedValue::DateTime(datetime_from(&v.u.datetime)?)
            }
            EXI_V_QNAME => {
                let n = v.u.qname;
                let on =
                    crate::OwnedName::new(str_of(n.uri)?, str_of(n.local)?);
                let p = str_of(n.prefix)?;
                OwnedValue::QName(if p.is_empty() {
                    on
                } else {
                    on.with_prefix(p)
                })
            }
            EXI_V_LIST => {
                let l = v.u.list;
                let items: &[exi_value] = if l.count == 0 {
                    &[]
                } else {
                    core::slice::from_raw_parts(l.items, l.count)
                };
                OwnedValue::List(
                    items
                        .iter()
                        .map(|x| value_from(x))
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            EXI_V_ENUM => OwnedValue::Enum {
                ordinal: v.u.enum_ordinal,
                text: text()?.into(),
            },
            _ => return None,
        }
    })
}

/// A static message for a status.
pub(super) fn status_text(s: exi_status) -> &'static core::ffi::CStr {
    match s {
        EXI_OK => c"ok",
        EXI_END => c"end",
        EXI_NEED_INPUT => c"need input",
        EXI_E_ARG => c"invalid argument",
        EXI_E_STATE => c"call not valid in this state",
        EXI_E_NOMEM => c"out of memory",
        EXI_E_IO => c"input or output failed",
        EXI_E_HEADER => c"malformed EXI header",
        EXI_E_GRAMMAR => c"stream does not match the grammar",
        EXI_E_VALUE => c"malformed value",
        EXI_E_TRUNCATED => c"input ended inside an event",
        EXI_E_LIMIT => c"a limit was exceeded",
        EXI_E_SCHEMA => c"schema mismatch",
        EXI_E_UNSUPPORTED => c"not supported by this build",
        EXI_E_XML => c"XML is not well-formed",
        EXI_E_INTERNAL => c"internal error",
        EXI_E_ABORTED => c"stopped by a handler",
        EXI_E_BUFFER_FULL => c"output buffer full",
        EXI_E_UNENCODABLE => c"event fits no production",
        _ => c"unknown status",
    }
}
