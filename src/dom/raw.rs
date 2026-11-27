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

use crate::OwnedValue;
use crate::strings::{Held, Local, Prefix, Uri};

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

/// What Rust keeps beside each node: the references that keep its
/// strings alive.
pub(crate) struct Extra<'g> {
    /// The node's slot in the document's table.
    pub slot: u32,
    pub uri: Option<Held<'g, Uri>>,
    pub local: Option<Held<'g, Local>>,
    /// The backing buffer of a PI or entity name, or a wanted prefix.
    pub name_buf: Option<Box<[u8]>>,
    pub content: Content,
}

/// Text backing the C pointer, or a boxed typed value used by EXI.
pub(crate) enum Content {
    None,
    Text(Box<[u8]>),
    Typed(Box<OwnedValue>),
}

impl Content {
    pub fn text(&self) -> &str {
        match self {
            Self::Text(text) => text_of(text),
            _ => "",
        }
    }
}

impl Extra<'_> {
    pub fn new(slot: u32) -> Self {
        Extra {
            slot,
            uri: None,
            local: None,
            name_buf: None,
            content: Content::None,
        }
    }
}

#[repr(C)]
pub(crate) struct NodeBox<'g> {
    pub raw: RawNode,
    pub x: Extra<'g>,
}

#[repr(C)]
pub(crate) struct AttrBox<'g> {
    pub raw: RawAttr,
    pub x: Extra<'g>,
}

#[repr(C)]
pub(crate) struct DtdBox<'g> {
    pub raw: RawDtd,
    pub x: Extra<'g>,
    /// Public ID, system ID and internal subset, NUL-terminated.
    pub ids: [Box<[u8]>; 3],
}

#[repr(C)]
pub(crate) struct NsBox<'g> {
    pub raw: RawNs,
    pub uri: Held<'g, Uri>,
    pub prefix: Option<Held<'g, Prefix>>,
}

/// A NUL-terminated copy of `s`.
pub(crate) fn cstr(s: &str) -> Box<[u8]> {
    let mut v = alloc::vec::Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v.into_boxed_slice()
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

/// The Rust side of a node of any kind but the document.
///
/// # Safety
///
/// `p` is a live node of a document with strings `'g`, not the document
/// itself.
pub(crate) unsafe fn extra<'a, 'g>(p: *mut RawCommon) -> &'a Extra<'g> {
    // SAFETY: the caller keeps the node alive for the borrow.
    unsafe { &*extra_ptr(p) }
}

/// Mutable metadata of a node.
///
/// # Safety
///
/// As `extra`, with exclusive access to the metadata for the borrow.
pub(crate) unsafe fn extra_mut<'a, 'g>(
    p: *mut RawCommon,
) -> &'a mut Extra<'g> {
    // SAFETY: the caller keeps the node alive and exclusively borrowed.
    unsafe { &mut *extra_ptr(p) }
}

/// # Safety
///
/// `p` is a live node of the indicated box type, excluding the
/// document.
unsafe fn extra_ptr<'g>(p: *mut RawCommon) -> *mut Extra<'g> {
    // SAFETY: each box is repr(C) with its public C struct first.
    unsafe {
        match (*p).r#type {
            EXI_ATTRIBUTE_NODE => &raw mut (*(p as *mut AttrBox<'g>)).x,
            EXI_DTD_NODE => &raw mut (*(p as *mut DtdBox<'g>)).x,
            _ => &raw mut (*(p as *mut NodeBox<'g>)).x,
        }
    }
}

/// Frees the box behind `p` (not its children).
///
/// # Safety
///
/// `p` is a live node, unlinked, never used again.
pub(crate) unsafe fn drop_box(p: *mut RawCommon) {
    // SAFETY: as `extra`; the box was made with `Box::into_raw`.
    unsafe {
        match (*p).r#type {
            EXI_ATTRIBUTE_NODE => drop(Box::from_raw(p as *mut AttrBox<'_>)),
            EXI_DTD_NODE => drop(Box::from_raw(p as *mut DtdBox<'_>)),
            _ => drop(Box::from_raw(p as *mut NodeBox<'_>)),
        }
    }
}

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
