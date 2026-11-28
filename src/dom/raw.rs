// src/dom/raw.rs
//
// C-compatible document tree layout
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! These are the structs C programs see (`include/exi_dom.h`):
//! libxml2's `xmlDoc`, `xmlNode`, `xmlAttr`, `xmlNs` and `xmlDtd`, with
//! the same fields in the same order, so tree walks written for libxml2
//! (`for (cur = node->children; cur; cur = cur->next)`) work unchanged.
//! Each struct then has fields libxml2 does not: the name's global ids
//! (`uriId`, `localId`), for comparing names as integers.
//!
//! The first nine fields of `exiDoc`, `exiNode`, `exiAttr` and `exiDtd`
//! are the same ([`RawCommon`]), so a pointer to any of them can be
//! read as a `RawCommon`, as libxml2 code does with `xmlNodePtr`.
//!
//! Rust code uses [`Document`](super::Document) and [`NodeId`](super::NodeId) instead.
//! The structs are public for the C ABI and for programs that walk a
//! tree the same way: reading them is safe while the document is
//! borrowed; writing them is not supported.

#![allow(non_snake_case)]

use alloc::boxed::Box;
use core::ffi::{c_int, c_void};
use core::ptr;

use crate::strings::{Held, Prefix, Uri};
use crate::{Decimal, Float, OwnedValue, Value};

/// `xmlElementType`: the `type` field. The numbers are libxml2's.
pub type ExiElementType = c_int;
pub const EXI_ELEMENT_NODE: c_int = 1;
pub const EXI_ATTRIBUTE_NODE: c_int = 2;
pub const EXI_TEXT_NODE: c_int = 3;
pub const EXI_CDATA_SECTION_NODE: c_int = 4;
pub const EXI_ENTITY_REF_NODE: c_int = 5;
pub const EXI_PI_NODE: c_int = 7;
pub const EXI_COMMENT_NODE: c_int = 8;
pub const EXI_DOCUMENT_NODE: c_int = 9;
pub const EXI_DTD_NODE: c_int = 14;
pub const EXI_NAMESPACE_DECL: c_int = 18;

/// The fields every tree struct starts with.
#[repr(C)]
#[derive(Debug)]
pub struct RawCommon {
    pub _private: *mut c_void,
    pub r#type: ExiElementType,
    pub name: *const u8,
    pub children: *mut RawNode,
    pub last: *mut RawNode,
    pub parent: *mut RawNode,
    pub next: *mut RawNode,
    pub prev: *mut RawNode,
    pub doc: *mut RawDoc,
}

/// `exiNode` (libxml2 `xmlNode`): elements, text, comments, PIs, entity
/// references.
#[repr(C)]
#[derive(Debug)]
pub struct RawNode {
    pub _private: *mut c_void,
    pub r#type: ExiElementType,
    /// Elements: the local name. Text: `"text"`. Comments: `"comment"`.
    /// PIs: the target.
    pub name: *const u8,
    pub children: *mut RawNode,
    pub last: *mut RawNode,
    pub parent: *mut RawNode,
    pub next: *mut RawNode,
    pub prev: *mut RawNode,
    pub doc: *mut RawDoc,
    /// The element's namespace declaration, NULL for no namespace.
    pub ns: *mut RawNs,
    /// Text, comment and PI data, NUL-terminated. NULL for a typed
    /// value: read it with `exiNodeGetContent` or `exiNodeGetValue`.
    pub content: *mut u8,
    pub properties: *mut RawAttr,
    /// Namespaces declared on this element.
    pub nsDef: *mut RawNs,
    pub psvi: *mut c_void,
    /// The line of the start tag in XML text, at most 65535; 0 when
    /// unknown.
    pub line: u16,
    pub extra: u16,
    /// The name's ids in the document's global string tables;
    /// `u32::MAX` when it has none.
    pub uriId: u32,
    pub localId: u32,
}

/// `exiAttr` (libxml2 `xmlAttr`). The value is the text node(s) in
/// `children`.
#[repr(C)]
#[derive(Debug)]
pub struct RawAttr {
    pub _private: *mut c_void,
    pub r#type: ExiElementType,
    pub name: *const u8,
    pub children: *mut RawNode,
    pub last: *mut RawNode,
    pub parent: *mut RawNode,
    pub next: *mut RawAttr,
    pub prev: *mut RawAttr,
    pub doc: *mut RawDoc,
    pub ns: *mut RawNs,
    pub atype: c_int,
    pub psvi: *mut c_void,
    pub id: *mut c_void,
    pub uriId: u32,
    pub localId: u32,
}

/// `exiNs` (libxml2 `xmlNs`): one namespace declaration.
#[repr(C)]
#[derive(Debug)]
pub struct RawNs {
    pub next: *mut RawNs,
    pub r#type: ExiElementType,
    pub href: *const u8,
    /// NULL for the default namespace.
    pub prefix: *const u8,
    pub _private: *mut c_void,
    pub context: *mut RawDoc,
    pub uriId: u32,
    pub _pad: u32,
}

/// `exiDtd` (libxml2 `xmlDtd`): the document type declaration.
/// Declarations inside it are not parsed; `exiDtdInternalSubset` gives
/// the text.
#[repr(C)]
#[derive(Debug)]
pub struct RawDtd {
    pub _private: *mut c_void,
    pub r#type: ExiElementType,
    pub name: *const u8,
    pub children: *mut RawNode,
    pub last: *mut RawNode,
    pub parent: *mut RawDoc,
    pub next: *mut RawNode,
    pub prev: *mut RawNode,
    pub doc: *mut RawDoc,
    pub notations: *mut c_void,
    pub elements: *mut c_void,
    pub attributes: *mut c_void,
    pub entities: *mut c_void,
    pub ExternalID: *const u8,
    pub SystemID: *const u8,
    pub pentities: *mut c_void,
}

/// `exiDoc` (libxml2 `xmlDoc`).
#[repr(C)]
#[derive(Debug)]
pub struct RawDoc {
    pub _private: *mut c_void,
    pub r#type: ExiElementType,
    pub name: *mut u8,
    pub children: *mut RawNode,
    pub last: *mut RawNode,
    pub parent: *mut RawNode,
    pub next: *mut RawNode,
    pub prev: *mut RawNode,
    pub doc: *mut RawDoc,
    pub compression: c_int,
    pub standalone: c_int,
    pub intSubset: *mut RawDtd,
    pub extSubset: *mut RawDtd,
    /// The `xml` namespace, when an `xml:` attribute needed it.
    pub oldNs: *mut RawNs,
    pub version: *const u8,
    pub encoding: *const u8,
    pub ids: *mut c_void,
    pub refs: *mut c_void,
    pub URL: *const u8,
    pub charset: c_int,
    pub dict: *mut c_void,
    pub psvi: *mut c_void,
    pub parseFlags: c_int,
    pub properties: c_int,
}

pub(crate) const NO_ID: u32 = u32::MAX;
pub(crate) static TEXT_NAME: &[u8] = b"text\0";
pub(crate) static COMMENT_NAME: &[u8] = b"comment\0";
pub(crate) static VERSION: &[u8] = b"1.0\0";
pub(crate) static UTF8: &[u8] = b"UTF-8\0";

/// Longest UTF-8 text (excluding the NUL byte) stored inline in a node box.
pub(crate) const INLINE_MAX: usize = 21;

/// What Rust keeps beside each text, comment, PI or entity-ref node:
/// its slot and owned payload.
pub(crate) struct Extra {
    /// The node's slot in the document's table.
    pub slot: u32,
    pub payload: Payload,
}

/// Inline short text, inline scalar typed values, a heap text buffer,
/// a boxed compound typed value, an entity-ref name, or a PI `(target,
/// data)` pair.
pub(crate) enum Payload {
    None,
    /// Short NUL-terminated UTF-8 text stored inline in the node box
    /// (`len as usize <= INLINE_MAX`, `buf[len as usize] == 0`).
    Inline { len: u8, buf: [u8; INLINE_MAX + 1] },
    /// NUL-terminated UTF-8 text in the document's text arena
    /// (`ptr[len] == 0`), owned by the arena, not the node.
    Arena { ptr: *const u8, len: u32 },
    Text(CBuf),
    Bool(bool),
    BoolDigit(bool),
    Int(i64),
    Float(Float),
    Decimal {
        negative: bool,
        fraction_digits: u8,
        integral: u64,
        fraction: u64,
    },
    /// Compound or oversized typed EXI value.
    Typed(Box<OwnedValue>),
    /// Backing buffer for an entity-ref name (keeps `RawNode::name` alive).
    Name(#[allow(dead_code)] CBuf),
    /// Processing instruction `(target, data)`, both NUL-terminated.
    Pi(Box<PiText>),
}

/// A processing instruction's target and data.
pub(crate) type PiText = (CBuf, CBuf);

impl Extra {
    pub fn new(slot: u32) -> Self {
        Extra { slot, payload: Payload::None }
    }

    pub fn text(&self) -> &str {
        match &self.payload {
            Payload::Inline { len, buf } => {
                // SAFETY: written from a `&str` in `set_text`.
                unsafe { core::str::from_utf8_unchecked(&buf[..*len as usize]) }
            }
            Payload::Arena { ptr, len } => {
                // SAFETY: written from a `&str` in `set_text`; the arena
                // outlives every node that points into it.
                unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                        *ptr,
                        *len as usize,
                    ))
                }
            }
            Payload::Text(text) => text_of(text),
            Payload::Pi(pair) => text_of(&pair.1),
            _ => "",
        }
    }

    /// The node's value (for text/CDATA nodes): inline scalar values and
    /// text borrow directly without allocating.
    pub fn as_value(&self) -> Value<'_> {
        match &self.payload {
            Payload::Bool(b) => Value::Bool(*b),
            Payload::BoolDigit(b) => Value::BoolDigit(*b),
            Payload::Int(i) => Value::Int(*i),
            Payload::Float(f) => Value::Float(*f),
            Payload::Decimal {
                negative,
                fraction_digits,
                integral,
                fraction,
            } => Value::Decimal(Decimal {
                negative: *negative,
                integral: *integral,
                fraction: *fraction,
                fraction_digits: *fraction_digits,
            }),
            Payload::Typed(v) => v.as_value(),
            _ => Value::Str(self.text()),
        }
    }

    /// True when the payload owns a separate heap allocation that must be
    /// dropped before freeing the arena chunk.
    pub fn has_heap_payload(&self) -> bool {
        matches!(
            self.payload,
            Payload::Text(_)
                | Payload::Typed(_)
                | Payload::Name(_)
                | Payload::Pi(_)
        )
    }
}

/// Element box: `RawNode` plus slot and optional wanted `PrefixId`
/// (`NO_ID` when none). Names are held as reference-counted ids in
/// `raw.uriId` and `raw.localId`.
#[repr(C)]
pub(crate) struct ElemBox {
    pub raw: RawNode,
    pub slot: u32,
    pub want_prefix: u32,
}

#[repr(C)]
pub(crate) struct NodeBox {
    pub raw: RawNode,
    pub x: Extra,
}

/// Attribute box: `RawAttr` plus slot and optional wanted `PrefixId`.
#[repr(C)]
pub(crate) struct AttrBox {
    pub raw: RawAttr,
    pub slot: u32,
    pub want_prefix: u32,
}

#[repr(C)]
pub(crate) struct DtdBox {
    pub raw: RawDtd,
    pub slot: u32,
    pub name: CBuf,
    /// Public ID, system ID and internal subset, NUL-terminated.
    pub ids: [CBuf; 3],
}

#[repr(C)]
pub(crate) struct NsBox<'g> {
    pub raw: RawNs,
    pub uri: Held<'g, Uri>,
    pub prefix: Option<Held<'g, Prefix>>,
}

/// A heap byte buffer that the tree points into (`name`, `content`,
/// `href`). It is owned through a raw pointer, not a `Box`: moving a
/// `Box` asserts unique access to its bytes, which would invalidate the
/// tree's pointers into them every time the buffer moved into a payload
/// or struct.
pub(crate) struct CBuf(ptr::NonNull<[u8]>);

impl CBuf {
    pub fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr().cast()
    }
}

impl core::ops::Deref for CBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: the buffer is ours and alive until `drop`.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for CBuf {
    fn drop(&mut self) {
        // SAFETY: made from `Box::into_raw` in `cstr`, freed once here.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

/// A NUL-terminated copy of `s`.
pub(crate) fn cstr(s: &str) -> CBuf {
    let mut v = alloc::vec::Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    let b: Box<[u8]> = v.into_boxed_slice();
    // SAFETY: `Box::into_raw` is never null.
    CBuf(unsafe { ptr::NonNull::new_unchecked(Box::into_raw(b)) })
}

/// The text of a buffer made by [`cstr`].
pub(crate) fn text_of(b: &[u8]) -> &str {
    // SAFETY: every buffer passed here was made by `cstr` from a
    // `&str`, plus a NUL.
    unsafe { core::str::from_utf8_unchecked(&b[..b.len() - 1]) }
}

/// The NUL-terminated string at `p` (from a table or a `cstr` buffer).
///
/// # Safety
///
/// `p` points to valid UTF-8 ending in a NUL, alive for `'a`.
pub(crate) unsafe fn str_at<'a>(p: *const u8) -> &'a str {
    if p.is_null() {
        return "";
    }
    // SAFETY: the caller's contract.
    unsafe {
        let n = core::ffi::CStr::from_ptr(p.cast()).to_bytes();
        core::str::from_utf8_unchecked(n)
    }
}

impl RawNode {
    pub(crate) fn blank(kind: c_int, doc: *mut RawDoc) -> RawNode {
        RawNode {
            _private: ptr::null_mut(),
            r#type: kind,
            name: ptr::null(),
            children: ptr::null_mut(),
            last: ptr::null_mut(),
            parent: ptr::null_mut(),
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
            doc,
            ns: ptr::null_mut(),
            content: ptr::null_mut(),
            properties: ptr::null_mut(),
            nsDef: ptr::null_mut(),
            psvi: ptr::null_mut(),
            line: 0,
            extra: 0,
            uriId: NO_ID,
            localId: NO_ID,
        }
    }
}

impl RawAttr {
    pub(crate) fn blank(doc: *mut RawDoc) -> RawAttr {
        RawAttr {
            _private: ptr::null_mut(),
            r#type: EXI_ATTRIBUTE_NODE,
            name: ptr::null(),
            children: ptr::null_mut(),
            last: ptr::null_mut(),
            parent: ptr::null_mut(),
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
            doc,
            ns: ptr::null_mut(),
            atype: 0,
            psvi: ptr::null_mut(),
            id: ptr::null_mut(),
            uriId: NO_ID,
            localId: NO_ID,
        }
    }
}

impl RawDtd {
    pub(crate) fn blank(doc: *mut RawDoc) -> RawDtd {
        RawDtd {
            _private: ptr::null_mut(),
            r#type: EXI_DTD_NODE,
            name: ptr::null(),
            children: ptr::null_mut(),
            last: ptr::null_mut(),
            parent: ptr::null_mut(),
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
            doc,
            notations: ptr::null_mut(),
            elements: ptr::null_mut(),
            attributes: ptr::null_mut(),
            entities: ptr::null_mut(),
            ExternalID: ptr::null(),
            SystemID: ptr::null(),
            pentities: ptr::null_mut(),
        }
    }
}

impl RawDoc {
    pub(crate) fn blank() -> RawDoc {
        RawDoc {
            _private: ptr::null_mut(),
            r#type: EXI_DOCUMENT_NODE,
            name: ptr::null_mut(),
            children: ptr::null_mut(),
            last: ptr::null_mut(),
            parent: ptr::null_mut(),
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
            doc: ptr::null_mut(),
            compression: 0,
            standalone: -1,
            intSubset: ptr::null_mut(),
            extSubset: ptr::null_mut(),
            oldNs: ptr::null_mut(),
            version: VERSION.as_ptr(),
            encoding: UTF8.as_ptr(),
            ids: ptr::null_mut(),
            refs: ptr::null_mut(),
            URL: ptr::null(),
            charset: 1,
            dict: ptr::null_mut(),
            psvi: ptr::null_mut(),
            parseFlags: 0,
            properties: 0,
        }
    }
}

/// The slot number of any non-document node.
///
/// # Safety
///
/// `p` is a live node of a document, not the document itself.
pub(crate) unsafe fn node_slot(p: *mut RawCommon) -> u32 {
    // SAFETY: each box is repr(C) with its public C struct first.
    unsafe {
        match (*p).r#type {
            EXI_ELEMENT_NODE => (*(p as *const ElemBox)).slot,
            EXI_ATTRIBUTE_NODE => (*(p as *const AttrBox)).slot,
            EXI_DTD_NODE => (*(p as *const DtdBox)).slot,
            _ => (*(p as *const NodeBox)).x.slot,
        }
    }
}

/// Sets the slot number of any non-document node.
///
/// # Safety
///
/// As [`node_slot`], with exclusive access to the node.
pub(crate) unsafe fn set_node_slot(p: *mut RawCommon, slot: u32) {
    // SAFETY: each box is repr(C) with its public C struct first.
    unsafe {
        match (*p).r#type {
            EXI_ELEMENT_NODE => (*(p as *mut ElemBox)).slot = slot,
            EXI_ATTRIBUTE_NODE => (*(p as *mut AttrBox)).slot = slot,
            EXI_DTD_NODE => (*(p as *mut DtdBox)).slot = slot,
            _ => (*(p as *mut NodeBox)).x.slot = slot,
        }
    }
}

/// The wanted prefix id (`NO_ID` when none) of an element or attribute.
///
/// # Safety
///
/// `p` is a live element or attribute node.
pub(crate) unsafe fn want_prefix_id(p: *mut RawCommon) -> u32 {
    // SAFETY: the caller's contract.
    unsafe {
        match (*p).r#type {
            EXI_ELEMENT_NODE => (*(p as *const ElemBox)).want_prefix,
            EXI_ATTRIBUTE_NODE => (*(p as *const AttrBox)).want_prefix,
            _ => NO_ID,
        }
    }
}

/// Sets the wanted prefix id of an element or attribute, returning the
/// previous id (`NO_ID` when none).
///
/// # Safety
///
/// `p` is a live element or attribute node, exclusively borrowed.
pub(crate) unsafe fn swap_want_prefix_id(p: *mut RawCommon, id: u32) -> u32 {
    // SAFETY: the caller's contract.
    unsafe {
        match (*p).r#type {
            EXI_ELEMENT_NODE => {
                core::mem::replace(&mut (*(p as *mut ElemBox)).want_prefix, id)
            }
            EXI_ATTRIBUTE_NODE => {
                core::mem::replace(&mut (*(p as *mut AttrBox)).want_prefix, id)
            }
            _ => NO_ID,
        }
    }
}

/// The Rust side of a text, comment, PI or entity-ref node (`NodeBox`).
///
/// # Safety
///
/// `p` is a live `NodeBox` (not a document, element, attribute or DTD).
pub(crate) unsafe fn extra<'a>(p: *mut RawCommon) -> &'a Extra {
    // SAFETY: the caller keeps the node alive for the borrow.
    unsafe { &(*(p as *const NodeBox)).x }
}

/// Mutable metadata of a `NodeBox`.
///
/// # Safety
///
/// As [`extra`], with exclusive access to the metadata for the borrow.
pub(crate) unsafe fn extra_mut<'a>(p: *mut RawCommon) -> &'a mut Extra {
    // SAFETY: the caller keeps the node alive and exclusively borrowed.
    unsafe { &mut (*(p as *mut NodeBox)).x }
}

/// Drops any external heap allocation owned by `p` (idempotent:
/// replaces `Payload` with `Payload::None`, frees `DtdBox` for DTDs).
/// Does not free `p`'s arena slot or release document-retained string ids.
///
/// # Safety
///
/// `p` is a live node owned by the dropping document (for `EXI_DTD_NODE`,
/// `p` is never used again after this call).
pub(crate) unsafe fn drop_node_payload(p: *mut RawCommon) {
    // SAFETY: each box is `repr(C)` matching `(*p).r#type`.
    unsafe {
        match (*p).r#type {
            EXI_ELEMENT_NODE | EXI_ATTRIBUTE_NODE => {}
            EXI_DTD_NODE => drop(Box::from_raw(p as *mut DtdBox)),
            _ => {
                extra_mut(p).payload = Payload::None;
            }
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<Payload>() == 24);
    assert!(core::mem::size_of::<Extra>() == 32);
    assert!(
        core::mem::size_of::<ElemBox>() == core::mem::size_of::<RawNode>() + 8
    );
    assert!(
        core::mem::size_of::<AttrBox>() == core::mem::size_of::<RawAttr>() + 8
    );
};

// Layout checks: the common part is at the same offsets in every
// struct.
const _: () = {
    use core::mem::offset_of;
    macro_rules! same {
        ($t:ty) => {
            assert!(
                offset_of!($t, _private) == offset_of!(RawCommon, _private)
            );
            assert!(offset_of!($t, r#type) == offset_of!(RawCommon, r#type));
            assert!(offset_of!($t, name) == offset_of!(RawCommon, name));
            assert!(
                offset_of!($t, children) == offset_of!(RawCommon, children)
            );
            assert!(offset_of!($t, last) == offset_of!(RawCommon, last));
            assert!(offset_of!($t, parent) == offset_of!(RawCommon, parent));
            assert!(offset_of!($t, next) == offset_of!(RawCommon, next));
            assert!(offset_of!($t, prev) == offset_of!(RawCommon, prev));
            assert!(offset_of!($t, doc) == offset_of!(RawCommon, doc));
        };
    }
    same!(RawNode);
    same!(RawAttr);
    same!(RawDtd);
    same!(RawDoc);
};
