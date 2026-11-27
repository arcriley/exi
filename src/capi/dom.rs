// src/capi/dom.rs
//
// Libxml2-style C document tree API
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The C DOM: libxml2's tree API (`tree.h`, `parser.h`) under `exi`
//! names (feature `capi` with `dom`). `include/exi_dom.h` declares it;
//! `include/exi_libxml_compat.h` maps libxml2's names onto it.
//!
//! # Conventions, as libxml2
//!
//! | Rule | Detail |
//! |---|---|
//! | Strings | `exiChar*`: NUL-terminated UTF-8. Input that is not UTF-8 makes the call fail |
//! | Failure | NULL, or -1 for `int` results |
//! | Strings returned | malloc'd; free with `exiFree` |
//! | Nodes | owned by their document; `exiFreeNode` frees one (unlinking it first, which libxml2 leaves to the caller) |
//! | Threads | one thread at a time per document, and per string tables |
//!
//! # Where libexi differs from libxml2
//!
//! | | libexi |
//! |---|---|
//! | String tables | Documents made without `exiNewDocWithStrings` share their thread's tables, so names compare as integers across them: `uriId`/`localId` in each node |
//! | Nodes made without a document | `exiNewNode`, `exiNewText`, `exiNewComment`, `exiNewPI` and `exiNewProp(NULL, …)` make nodes in a holding document of the thread. Adding one to a document moves it there, keeping its pointer |
//! | Unlinked nodes | an unlinked element's `ns` is NULL until it is placed again: its declaration may have been freed with its old parent |
//! | `exiNewChild` | `content` is text, as `xmlNewTextChild`; no entity references are parsed |
//! | `exiNewNs` | `exiNewNs(el, href, NULL)` returns NULL when `el` has no namespace (a default declaration would move `el` into that namespace); libxml2 allows it |
//! | Typed values | text nodes from schema-informed streams may have `content` NULL: `exiNodeGetContent` and `exiNodeGetValue` give the value |
//! | Encodings | UTF-8 only; the `encoding` arguments are ignored |
//! | `exiCopyNode` | `extended` 0 copies as 2 (with properties and namespaces) |
//! | `format` arguments | ignored: no indentation is added |

#![allow(non_snake_case)]

use core::ffi::{c_char, c_int, c_long, c_ulong, c_void};
use core::mem::ManuallyDrop;
use core::ptr;
use std::boxed::Box;
use std::string::{String, ToString};
use std::thread_local;
use std::vec::Vec;

use super::imp::{Scratch, guard, value_from};
use super::{
    EXI_E_ARG, EXI_E_INTERNAL, EXI_E_STATE, EXI_OK, exi_reader, exi_status,
    exi_strings, exi_value, exi_writer,
};
use crate::dom::raw::*;
use crate::dom::{Document, DocumentBuilder, NodeId, NodeKind};
use crate::strings::{GlobalConfig, GlobalStrings};
use crate::{Error, Event, EventSink, Name, Result, Step};

unsafe extern "C" {
    fn malloc(n: usize) -> *mut c_void;
    fn free(p: *mut c_void);
}

pub type exiChar = u8;
pub type exiDocPtr = *mut RawDoc;
pub type exiNodePtr = *mut RawNode;
pub type exiAttrPtr = *mut RawAttr;
pub type exiNsPtr = *mut RawNs;
pub type exiDtdPtr = *mut RawDtd;

/// `XML_PARSE_NOBLANKS`: leave out whitespace-only text.
pub const EXI_PARSE_NOBLANKS: c_int = 1 << 8;
/// `XML_PARSE_NODICT`: accepted and ignored; libexi's names are always
/// shared.
pub const EXI_PARSE_NODICT: c_int = 1 << 12;

// ----- thread defaults -----

thread_local! {
    /// The thread's string tables, for documents made without explicit ones. Never freed:
    /// documents of the thread may live until its end.
    static DEFAULT: &'static GlobalStrings = Box::leak(Box::new(GlobalStrings::new(GlobalConfig::new()).expect("default string tables")));
    static NURSERY: Nursery = Nursery(Document::new(default_strings()).into_raw());
}

struct Nursery(*mut RawDoc);

impl Drop for Nursery {
    fn drop(&mut self) {
        // SAFETY: made by `into_raw` for this thread; freed once, at
        // thread exit.
        drop(unsafe { Document::<'static>::from_raw(self.0) });
    }
}

fn default_strings() -> &'static GlobalStrings {
    DEFAULT.with(|s| *s)
}

fn nursery() -> *mut RawDoc {
    NURSERY.with(|n| n.0)
}

// ----- helpers -----

/// The document behind a document pointer.
///
/// # Safety
///
/// `d` is NULL or a live document from this API.
unsafe fn doc<'a>(d: *const RawDoc) -> Option<ManuallyDrop<Document<'a>>> {
    // SAFETY: the caller's contract; the type field guards against
    // stray pointers.
    unsafe {
        (!d.is_null() && (*d).r#type == EXI_DOCUMENT_NODE)
            .then(|| Document::borrow_raw(d as *mut RawDoc))
    }
}

/// A node's document and id.
///
/// # Safety
///
/// `n` is NULL or a live node from this API.
unsafe fn node<'a>(
    n: *const RawCommon,
) -> Option<(ManuallyDrop<Document<'a>>, NodeId)> {
    if n.is_null() {
        return None;
    }
    // SAFETY: the caller's contract.
    unsafe {
        let d = if (*n).r#type == EXI_DOCUMENT_NODE {
            n as *const RawDoc
        } else {
            (*n).doc as *const RawDoc
        };
        let d = doc(d)?;
        let id = d.live_node_id(n.cast_mut())?;
        Some((d, id))
    }
}

/// A C string as `&str`; `None` for NULL or bytes that are not UTF-8.
///
/// # Safety
///
/// `p` is NULL or NUL-terminated.
unsafe fn cs<'a>(p: *const exiChar) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller's contract.
    unsafe { core::ffi::CStr::from_ptr(p.cast()).to_str().ok() }
}

/// # Safety
///
/// `p` is NULL or valid for `len` bytes.
unsafe fn cs_len<'a>(p: *const exiChar, len: c_int) -> Option<&'a str> {
    if p.is_null() || len < 0 {
        return None;
    }
    // SAFETY: the caller's contract.
    core::str::from_utf8(unsafe {
        core::slice::from_raw_parts(p, len as usize)
    })
    .ok()
}

/// A malloc'd NUL-terminated copy, for the caller to `exiFree`.
fn out_str(s: &str) -> *mut exiChar {
    // SAFETY: a fresh allocation of `len + 1` bytes, filled before it
    // is returned.
    unsafe {
        let p = malloc(s.len() + 1) as *mut u8;
        if p.is_null() {
            return p;
        }
        ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
        *p.add(s.len()) = 0;
        p
    }
}

fn ptr_of(d: &Document<'_>, n: Option<NodeId>) -> *mut RawNode {
    n.map_or(ptr::null_mut(), |n| d.raw(n) as *mut RawNode)
}

/// The namespace an `exiNs` names: URI and prefix.
///
/// # Safety
///
/// `ns` is NULL or live.
unsafe fn ns_name<'a>(ns: *const RawNs) -> (&'a str, Option<&'a str>) {
    if ns.is_null() {
        return ("", None);
    }
    // SAFETY: the caller's contract.
    unsafe { (cs((*ns).href).unwrap_or(""), cs((*ns).prefix)) }
}

/// Makes `n` a node of `d`'s document, moving it there if it is another
/// document's.
fn bring(d: &mut Document<'_>, n: *mut RawCommon) -> Option<NodeId> {
    // SAFETY: `n` is a live node of some document of this API.
    let (mut from, id) = unsafe { node(n)? };
    if from.as_raw() == d.as_raw() {
        Some(id)
    } else {
        d.adopt(&mut from, id).ok()
    }
}

// ----- documents -----

/// `xmlNewDoc`. The document uses its thread's string tables.
///
/// # Safety
///
/// `version` is NULL or NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiNewDoc(s!("1.0"));
///     assert_eq!((*d).r#type, EXI_DOCUMENT_NODE);
///     assert!(exiDocGetRootElement(d).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDoc(_version: *const exiChar) -> exiDocPtr {
    guard(ptr::null_mut(), || Document::new(default_strings()).into_raw())
}

/// A new document whose names go in `strings` (an `exi_strings` that
/// outlives it), so its name ids match readers and writers using the
/// same tables.
///
/// # Safety
///
/// `strings` is a live `exi_strings`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     use exi::capi::{exi_strings_free, exi_strings_new};
///     let strings = exi_strings_new(null());
///     let d = exiNewDocWithStrings(s!("1.0"), strings);
///     let a = exiNewDocNode(d, null_mut(), s!("a"), null());
///     exiDocSetRootElement(d, a);
///     let (mut uri, mut local) = (0, 0);
///     assert_eq!(exiNameLookup(d, s!(""), s!("a"), &mut uri, &mut local), 1);
///     assert_eq!(((*a).uriId, (*a).localId), (uri, local));      // ids from `strings`
///     assert!(exiNewDocWithStrings(s!("1.0"), null_mut()).is_null());
///     exiFreeDoc(d);
///     exi_strings_free(strings);                                  // after the document
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDocWithStrings(
    _version: *const exiChar,
    strings: *mut exi_strings,
) -> exiDocPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        match unsafe { strings_of(strings) } {
            Some(s) => Document::new(s).into_raw(),
            None => ptr::null_mut(),
        }
    })
}

/// # Safety
///
/// `s` is NULL or a live handle.
unsafe fn strings_of<'a>(s: *mut exi_strings) -> Option<&'a GlobalStrings> {
    let o = s as *mut super::imp::StringsObj;
    // SAFETY: the caller's contract.
    unsafe {
        o.as_ref()
            .filter(|o| o.magic == super::imp::MAGIC_STRINGS)
            .map(|o| &o.strings)
    }
}

/// `xmlFreeDoc`: the document and every node in it, linked or not.
///
/// # Safety
///
/// `d` is NULL or a live document, not used again.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a><b/></a>"));
///     let loose = exiNewDocNode(d, null_mut(), s!("c"), null());   // never linked
///     assert!(!loose.is_null());
///     exiFreeDoc(d);                                               // frees both
///     exiFreeDoc(null_mut());                                      // no-op
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiFreeDoc(d: exiDocPtr) {
    // SAFETY: the caller's contract; `doc` checks the node type.
    if d == nursery() || unsafe { doc(d) }.is_none() {
        return;
    }
    // SAFETY: as above.
    guard((), || drop(unsafe { Document::<'static>::from_raw(d) }));
}

/// `xmlDocGetRootElement`.
///
/// # Safety
///
/// `d` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<!--first--><a/>"));
///     assert_eq!(name(exiDocGetRootElement(d)), "a");
///     assert!(exiDocGetRootElement(null()).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocGetRootElement(d: *const RawDoc) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some(d) = (unsafe { doc(d) }) else { return ptr::null_mut() };
        ptr_of(&d, d.root())
    })
}

/// `xmlDocSetRootElement`: returns the old root, unlinked, or NULL.
///
/// # Safety
///
/// `d` and `root` are live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let b = exiNewDocNode(d, null_mut(), s!("b"), null());
///     let old = exiDocSetRootElement(d, b);
///     assert_eq!(name(old), "a");
///     assert!((*old).parent.is_null());                            // unlinked
///     assert_eq!(exiDocGetRootElement(d), b);
///     exiFreeNode(old);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocSetRootElement(
    d: exiDocPtr,
    root: exiNodePtr,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some(mut d) = (unsafe { doc(d) }) else { return ptr::null_mut() };
        let Some(r) = bring(&mut d, root as *mut RawCommon) else {
            return ptr::null_mut();
        };
        match d.set_root(r) {
            Ok(old) => ptr_of(&d, old),
            Err(_) => ptr::null_mut(),
        }
    })
}

/// `xmlCopyDoc`: with `recursive` 0, the document without its children.
///
/// # Safety
///
/// `d` is live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a><b/></a>"));
///     let (full, bare) = (exiCopyDoc(d, 1), exiCopyDoc(d, 0));
///     assert_eq!(kids(exiDocGetRootElement(full)), ["b"]);
///     assert!(exiDocGetRootElement(bare).is_null());               // no children
///     for x in [d, full, bare] { exiFreeDoc(x); }
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiCopyDoc(
    d: exiDocPtr,
    recursive: c_int,
) -> exiDocPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some(src) = (unsafe { doc(d) }) else { return ptr::null_mut() };
        let mut out = Document::new(src.strings());
        if let Some(dt) = src.doctype() {
            out.set_doctype(dt);
        }
        if recursive != 0 {
            let top = src.document_node();
            for c in src.children(top) {
                if src.kind(c) == NodeKind::Doctype {
                    continue;
                }
                match out
                    .import(&src, c, true)
                    .and_then(|n| out.append_child(out.document_node(), n))
                {
                    Ok(()) => {}
                    Err(_) => return ptr::null_mut(),
                }
            }
        }
        out.into_raw()
    })
}

// ----- making nodes -----

fn element_in(
    d: &mut Document<'_>,
    uri: &str,
    prefix: Option<&str>,
    local: &str,
    content: Option<&str>,
) -> Result<NodeId> {
    let mut name = Name::new(uri, local);
    if let Some(p) = prefix {
        name = name.with_prefix(p);
    }
    let e = d.new_element(name)?;
    if let Some(t) = content {
        let t = d.new_text(t);
        d.append_child(e, t)?;
    }
    Ok(e)
}

/// `xmlNewDocNode`: an element in `ns` (NULL: no namespace), with
/// `content` as text.
///
/// # Safety
///
/// `d` is live; `ns` NULL or live; strings NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiNewDoc(s!("1.0"));
///     let r = exiNewDocNode(d, null_mut(), s!("r"), null());
///     exiDocSetRootElement(d, r);
///     let ns = exiNewNs(r, s!("urn:x"), s!("x"));
///     let e = exiNewDocNode(d, ns, s!("e"), s!("some text"));
///     exiAddChild(r, e);
///     assert_eq!((*e).ns, ns);
///     assert_eq!(text(exiNodeGetContent(e)), "some text");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDocNode(
    d: exiDocPtr,
    ns: exiNsPtr,
    name: *const exiChar,
    content: *const exiChar,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        unsafe {
            let (Some(mut d), Some(local)) = (doc(d), cs(name)) else {
                return ptr::null_mut();
            };
            let (uri, prefix) = ns_name(ns);
            match element_in(&mut d, uri, prefix, local, cs(content)) {
                Ok(e) => d.raw(e) as exiNodePtr,
                Err(_) => ptr::null_mut(),
            }
        }
    })
}

/// `xmlNewNode`: an element with no document yet (see "Where libexi
/// differs" in the module documentation).
///
/// # Safety
///
/// As `exiNewDocNode`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let n = exiNewNode(null_mut(), s!("a"));                     // no document yet
///     let d = exiNewDoc(s!("1.0"));
///     exiDocSetRootElement(d, n);                                  // moved, same pointer
///     assert_eq!(exiDocGetRootElement(d), n);
///     assert_eq!((*n).doc, d);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewNode(
    ns: exiNsPtr,
    name: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract; the nursery is live.
    unsafe { exiNewDocNode(nursery(), ns, name, ptr::null()) }
}

/// `xmlNewChild` / `xmlNewTextChild`: a new last child of `parent`.
/// With `ns` NULL the child takes `parent`'s namespace, as libxml2
/// does.
///
/// # Safety
///
/// `parent` is live; `ns` NULL or live; strings NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r xmlns='urn:x'/>"));
///     let r = exiDocGetRootElement(d);
///     let c = exiNewChild(r, null_mut(), s!("c"), s!("a < b"));
///     assert_eq!((*c).ns, (*r).ns);                                // the parent's namespace
///     assert_eq!(text(exiNodeGetContent(c)), "a < b");             // text, not markup
///     assert_eq!(kids(r), ["c"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewChild(
    parent: exiNodePtr,
    ns: exiNsPtr,
    name: *const exiChar,
    content: *const exiChar,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        unsafe {
            let (Some((mut d, p)), Some(local)) =
                (node(parent as *const RawCommon), cs(name))
            else {
                return ptr::null_mut();
            };
            let ns = if ns.is_null() && (*parent).r#type == EXI_ELEMENT_NODE {
                (*parent).ns
            } else {
                ns
            };
            let (uri, prefix) = ns_name(ns);
            let made = element_in(&mut d, uri, prefix, local, cs(content))
                .and_then(|e| d.append_child(p, e).map(|()| e));
            match made {
                Ok(e) => d.raw(e) as exiNodePtr,
                Err(_) => ptr::null_mut(),
            }
        }
    })
}

/// `xmlNewTextChild`: the same as `exiNewChild` here.
///
/// # Safety
///
/// As `exiNewChild`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r/>"));
///     let r = exiDocGetRootElement(d);
///     let c = exiNewTextChild(r, null_mut(), s!("c"), s!("<b/>"));
///     assert_eq!(text(exiNodeGetContent(c)), "<b/>");
///     assert_eq!(exiChildElementCount(c), 0);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewTextChild(
    parent: exiNodePtr,
    ns: exiNsPtr,
    name: *const exiChar,
    content: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    unsafe { exiNewChild(parent, ns, name, content) }
}

fn leaf(
    d: *const RawDoc,
    f: impl FnOnce(&mut Document<'_>) -> Option<NodeId>,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        let d = if d.is_null() { nursery() } else { d as *mut RawDoc };
        // SAFETY: a live document from the caller, or the nursery.
        let Some(mut d) = (unsafe { doc(d) }) else { return ptr::null_mut() };
        let n = f(&mut d);
        ptr_of(&d, n)
    })
}

/// `xmlNewDocText`.
///
/// # Safety
///
/// `d` NULL or live; `content` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let a = exiDocGetRootElement(d);
///     let t = exiNewDocText(d, s!("hi"));
///     assert_eq!(exiNodeIsText(t), 1);
///     exiAddChild(a, t);
///     assert_eq!(text(exiNodeGetContent(a)), "hi");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDocText(
    d: *const RawDoc,
    content: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    let s = unsafe { cs(content) };
    leaf(d, |d| s.map(|s| d.new_text(s)))
}

/// `xmlNewDocTextLen`.
///
/// # Safety
///
/// `d` NULL or live; `content` valid for `len` bytes.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiNewDoc(s!("1.0"));
///     let t = exiNewDocTextLen(d, s!("hello"), 4);
///     assert_eq!(text(exiNodeGetContent(t)), "hell");
///     assert!(exiNewDocTextLen(d, s!("hello"), -1).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDocTextLen(
    d: exiDocPtr,
    content: *const exiChar,
    len: c_int,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    let s = unsafe { cs_len(content, len) };
    leaf(d, |d| s.map(|s| d.new_text(s)))
}

/// `xmlNewText`: a text node with no document yet.
///
/// # Safety
///
/// `content` is NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let t = exiNewText(s!("hi"));                                // no document yet
///     let d = exiParseDoc(s!("<a/>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(exiAddChild(a, t), t);
///     assert_eq!((*t).doc, d);
///     assert_eq!(text(exiNodeGetContent(a)), "hi");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewText(content: *const exiChar) -> exiNodePtr {
    // SAFETY: the caller's contract.
    unsafe { exiNewDocText(ptr::null(), content) }
}

/// `xmlNewTextLen`.
///
/// # Safety
///
/// `content` is valid for `len` bytes.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let t = exiNewTextLen(s!("hi there"), 2);
///     assert_eq!(text(exiNodeGetContent(t)), "hi");
///     exiFreeNode(t);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewTextLen(
    content: *const exiChar,
    len: c_int,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    unsafe { exiNewDocTextLen(ptr::null_mut(), content, len) }
}

/// `xmlNewDocComment`.
///
/// # Safety
///
/// `d` NULL or live; `content` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let c = exiNewDocComment(d, s!(" note "));
///     assert_eq!((*c).r#type, EXI_COMMENT_NODE);
///     exiAddChild(exiDocGetRootElement(d), c);
///     assert_eq!(text(exiNodeGetContent(c)), " note ");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDocComment(
    d: exiDocPtr,
    content: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    let s = unsafe { cs(content) };
    leaf(d, |d| s.map(|s| d.new_comment(s)))
}

/// `xmlNewComment`.
///
/// # Safety
///
/// `content` is NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let c = exiNewComment(s!("note"));
///     assert_eq!((*c).r#type, EXI_COMMENT_NODE);
///     assert_eq!(exiNodeIsText(c), 0);
///     exiFreeNode(c);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewComment(content: *const exiChar) -> exiNodePtr {
    // SAFETY: the caller's contract.
    unsafe { exiNewDocComment(ptr::null_mut(), content) }
}

/// `xmlNewDocPI`.
///
/// # Safety
///
/// `d` NULL or live; strings NUL-terminated (`content` may be NULL).
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let pi = exiNewDocPI(d, s!("style"), s!("href='x.css'"));
///     assert_eq!((*pi).r#type, EXI_PI_NODE);
///     assert_eq!(name(pi), "style");
///     assert_eq!(text(exiNodeGetContent(pi)), "href='x.css'");
///     assert!(!exiNewDocPI(d, s!("empty"), null()).is_null());    // content may be NULL
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewDocPI(
    d: exiDocPtr,
    name: *const exiChar,
    content: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    let (n, c) = unsafe { (cs(name), cs(content).unwrap_or("")) };
    leaf(d, |d| n.map(|n| d.new_pi(n, c)))
}

/// `xmlNewPI`.
///
/// # Safety
///
/// As `exiNewDocPI`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let pi = exiNewPI(s!("target"), s!("data"));
///     assert_eq!((*pi).r#type, EXI_PI_NODE);
///     assert_eq!(name(pi), "target");
///     exiFreeNode(pi);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewPI(
    name: *const exiChar,
    content: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    unsafe { exiNewDocPI(ptr::null_mut(), name, content) }
}

/// `xmlNewReference`: `&name;`, or `name` given as `&name;`.
///
/// # Safety
///
/// `d` NULL or live; `name` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let r = exiNewReference(d, s!("&nbsp;"));                    // or "nbsp"
///     assert_eq!((*r).r#type, EXI_ENTITY_REF_NODE);
///     assert_eq!(name(r), "nbsp");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewReference(
    d: *const RawDoc,
    name: *const exiChar,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    let n = unsafe { cs(name) }
        .map(|n| n.trim_start_matches('&').trim_end_matches(';'));
    leaf(d, |d| n.map(|n| d.new_entity_ref(n)))
}

// ----- linking -----

/// The text of a string text node, if `n` is one.
fn plain_text(d: &Document<'_>, n: NodeId) -> Option<String> {
    match (d.kind(n), d.value(n)) {
        (NodeKind::Text, Some(crate::Value::Str(s))) => Some(s.into()),
        _ => None,
    }
}

/// `xmlAddChild`: appends `cur` to `parent`. Text is merged into a text
/// `parent` or a text last child, and `cur` freed, as libxml2 does; the
/// node that holds the text is returned. An attribute goes on
/// `parent`'s attributes, replacing one of the same name.
///
/// # Safety
///
/// Both NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>x</a>"));
///     let a = exiDocGetRootElement(d);
///     let held = exiAddChild(a, exiNewDocText(d, s!("y")));        // merged, and freed
///     assert_eq!(held, (*a).children);
///     assert_eq!(text(exiNodeGetContent(a)), "xy");
///     let b = exiNewDocNode(d, null_mut(), s!("b"), null());
///     assert_eq!(exiAddChild(a, b), b);
///     assert_eq!((*a).last, b);
///     assert!(exiAddChild(a, a).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiAddChild(
    parent: exiNodePtr,
    cur: exiNodePtr,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        if parent.is_null() || cur.is_null() || parent == cur {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract.
        let Some((mut d, p)) = (unsafe { node(parent as *const RawCommon) })
        else {
            return ptr::null_mut();
        };
        let Some(c) = bring(&mut d, cur as *mut RawCommon) else {
            return ptr::null_mut();
        };
        if let Some(t) = plain_text(&d, c) {
            if plain_text(&d, p).is_some() {
                let _ = d.add_content(p, &t);
                d.remove(c);
                return d.raw(p) as exiNodePtr;
            }
            if let Some(last) =
                d.last_child(p).filter(|&l| plain_text(&d, l).is_some())
            {
                let _ = d.add_content(last, &t);
                d.remove(c);
                return d.raw(last) as exiNodePtr;
            }
        }
        let ok = if d.kind(c) == NodeKind::Attribute {
            d.attach_attribute(p, c)
        } else {
            d.append_child(p, c)
        };
        if ok.is_ok() { cur } else { ptr::null_mut() }
    })
}

/// `xmlAddChildList`: `cur` and its following siblings. Returns the
/// last added.
///
/// # Safety
///
/// As `exiAddChild`; `cur` heads a list of unlinked siblings.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><from><a/><b/></from><to/></r>"));
///     let r = exiDocGetRootElement(d);
///     let (from, to) = ((*r).children, (*r).last);
///     let last = exiAddChildList(to, (*from).children);
///     assert_eq!(name(last), "b");
///     assert_eq!(kids(to), ["a", "b"]);
///     assert!(kids(from).is_empty());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiAddChildList(
    parent: exiNodePtr,
    cur: exiNodePtr,
) -> exiNodePtr {
    let mut last = ptr::null_mut();
    let mut c = cur;
    while !c.is_null() {
        // SAFETY: the caller's contract.
        let next = unsafe { (*c).next };
        // SAFETY: as above.
        last = unsafe { exiAddChild(parent, c) };
        if last.is_null() {
            return ptr::null_mut();
        }
        c = next;
    }
    last
}

fn add_sibling(cur: exiNodePtr, elem: exiNodePtr, after: bool) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        if cur.is_null() || elem.is_null() || cur == elem {
            return ptr::null_mut();
        }
        // SAFETY: live nodes from the caller.
        let Some((mut d, c)) = (unsafe { node(cur as *const RawCommon) })
        else {
            return ptr::null_mut();
        };
        let Some(e) = bring(&mut d, elem as *mut RawCommon) else {
            return ptr::null_mut();
        };
        if let (Some(t), Some(_)) = (plain_text(&d, e), plain_text(&d, c)) {
            let _ = if after {
                d.add_content(c, &t)
            } else {
                let mut s = t;
                s.push_str(&d.content(c));
                d.set_content(c, &s)
            };
            d.remove(e);
            return cur;
        }
        let ok =
            if after { d.insert_after(c, e) } else { d.insert_before(c, e) };
        if ok.is_ok() { elem } else { ptr::null_mut() }
    })
}

/// `xmlAddNextSibling`.
///
/// # Safety
///
/// Both NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/><c/></r>"));
///     let r = exiDocGetRootElement(d);
///     let b = exiNewDocNode(d, null_mut(), s!("b"), null());
///     assert_eq!(exiAddNextSibling((*r).children, b), b);
///     assert_eq!(kids(r), ["a", "b", "c"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiAddNextSibling(
    cur: exiNodePtr,
    elem: exiNodePtr,
) -> exiNodePtr {
    add_sibling(cur, elem, true)
}

/// `xmlAddPrevSibling`.
///
/// # Safety
///
/// Both NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><b/></r>"));
///     let r = exiDocGetRootElement(d);
///     let a = exiNewDocNode(d, null_mut(), s!("a"), null());
///     assert_eq!(exiAddPrevSibling((*r).children, a), a);
///     assert_eq!(kids(r), ["a", "b"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiAddPrevSibling(
    cur: exiNodePtr,
    elem: exiNodePtr,
) -> exiNodePtr {
    add_sibling(cur, elem, false)
}

/// `xmlAddSibling`: after the last sibling of `cur`.
///
/// # Safety
///
/// Both NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/><b/></r>"));
///     let r = exiDocGetRootElement(d);
///     let z = exiNewDocNode(d, null_mut(), s!("z"), null());
///     exiAddSibling((*r).children, z);                             // after the last, b
///     assert_eq!(kids(r), ["a", "b", "z"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiAddSibling(
    cur: exiNodePtr,
    elem: exiNodePtr,
) -> exiNodePtr {
    let mut last = cur;
    // SAFETY: walking live siblings.
    unsafe {
        while !last.is_null() && !(*last).next.is_null() {
            last = (*last).next;
        }
    }
    add_sibling(last, elem, true)
}

/// `xmlUnlinkNode`. The node stays its document's until freed.
///
/// # Safety
///
/// `cur` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/><b/></r>"));
///     let r = exiDocGetRootElement(d);
///     let a = (*r).children;
///     exiUnlinkNode(a);
///     assert_eq!(kids(r), ["b"]);
///     assert!((*a).parent.is_null() && (*a).next.is_null());
///     assert_eq!((*a).doc, d);                                     // still the document's
///     exiAddChild(r, a);                                           // and can go back
///     assert_eq!(kids(r), ["b", "a"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiUnlinkNode(cur: exiNodePtr) {
    guard((), || {
        // SAFETY: the caller's contract.
        if let Some((mut d, n)) = unsafe { node(cur as *const RawCommon) } {
            d.unlink(n);
        }
    })
}

/// `xmlFreeNode`: frees `cur` and everything inside it, unlinking it
/// first. A document is freed as `exiFreeDoc`.
///
/// # Safety
///
/// `cur` is NULL or live, and not used again.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a><deep/></a><b/></r>"));
///     let r = exiDocGetRootElement(d);
///     exiFreeNode((*r).children);                                  // unlinked first
///     assert_eq!(kids(r), ["b"]);
///     exiFreeNode(null_mut());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiFreeNode(cur: exiNodePtr) {
    if cur.is_null() {
        return;
    }
    // SAFETY: the caller's contract.
    if unsafe { (*cur).r#type } == EXI_DOCUMENT_NODE {
        // SAFETY: as above.
        return unsafe { exiFreeDoc(cur as exiDocPtr) };
    }
    guard((), || {
        // SAFETY: the caller's contract.
        if let Some((mut d, n)) = unsafe { node(cur as *const RawCommon) } {
            d.remove(n);
        }
    })
}

/// `xmlFreeNodeList`: `cur` and its following siblings.
///
/// # Safety
///
/// As `exiFreeNode`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/><b/><c/></r>"));
///     let r = exiDocGetRootElement(d);
///     exiFreeNodeList((*(*r).children).next);                      // b and c
///     assert_eq!(kids(r), ["a"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiFreeNodeList(cur: exiNodePtr) {
    let mut c = cur;
    while !c.is_null() {
        // SAFETY: the caller's contract.
        let next = unsafe { (*c).next };
        // SAFETY: as above.
        unsafe { exiFreeNode(c) };
        c = next;
    }
}

/// `xmlReplaceNode`: puts `cur` where `old` is; returns `old`,
/// unlinked. With `cur` NULL, unlinks `old`.
///
/// # Safety
///
/// Both NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/><b/><c/></r>"));
///     let r = exiDocGetRootElement(d);
///     let b = (*(*r).children).next;
///     let x = exiNewDocNode(d, null_mut(), s!("x"), null());
///     assert_eq!(exiReplaceNode(b, x), b);
///     assert_eq!(kids(r), ["a", "x", "c"]);
///     assert!((*b).parent.is_null());
///     exiReplaceNode(x, null_mut());                               // only unlinks
///     assert_eq!(kids(r), ["a", "c"]);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiReplaceNode(
    old: exiNodePtr,
    cur: exiNodePtr,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some((mut d, o)) = (unsafe { node(old as *const RawCommon) })
        else {
            return ptr::null_mut();
        };
        if cur.is_null() {
            d.unlink(o);
            return old;
        }
        let Some(c) = bring(&mut d, cur as *mut RawCommon) else {
            return ptr::null_mut();
        };
        if d.replace(o, c).is_ok() { old } else { ptr::null_mut() }
    })
}

/// `xmlCopyNode`: `extended` 1 copies everything inside; 0 and 2 copy
/// the node with its attributes and namespace declarations.
///
/// # Safety
///
/// `n` is live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a k='v'><b/></a>"));
///     let a = exiDocGetRootElement(d);
///     let (deep, shallow) = (exiCopyNode(a, 1), exiCopyNode(a, 2));
///     assert_eq!(kids(deep), ["b"]);
///     assert!(kids(shallow).is_empty());
///     assert_eq!(text(exiGetProp(shallow, s!("k"))), "v");
///     assert!((*deep).parent.is_null());                           // unlinked, in `d`
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiCopyNode(
    n: exiNodePtr,
    extended: c_int,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some((mut d, id)) = (unsafe { node(n as *const RawCommon) })
        else {
            return ptr::null_mut();
        };
        match d.copy(id, extended == 1) {
            Ok(c) => d.raw(c) as exiNodePtr,
            Err(_) => ptr::null_mut(),
        }
    })
}

/// `xmlDocCopyNode`: a copy in document `d`.
///
/// # Safety
///
/// `n` and `d` are live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let (d, other) = (exiParseDoc(s!("<a><b/></a>")), exiNewDoc(s!("1.0")));
///     let copy = exiDocCopyNode(exiDocGetRootElement(d), other, 1);
///     assert_eq!((*copy).doc, other);
///     exiDocSetRootElement(other, copy);
///     assert_eq!(kids(exiDocGetRootElement(other)), ["b"]);
///     exiFreeDoc(d);
///     exiFreeDoc(other);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocCopyNode(
    n: exiNodePtr,
    d: exiDocPtr,
    extended: c_int,
) -> exiNodePtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let (Some((src, id)), Some(mut dst)) =
            (unsafe { node(n as *const RawCommon) }, unsafe { doc(d) })
        else {
            return ptr::null_mut();
        };
        let made = if ptr::eq(src.as_raw(), dst.as_raw()) {
            dst.copy(id, extended == 1)
        } else {
            dst.import(&src, id, extended == 1)
        };
        match made {
            Ok(c) => dst.raw(c) as exiNodePtr,
            Err(_) => ptr::null_mut(),
        }
    })
}

// ----- content -----

/// `xmlNodeGetContent`: malloc'd text, typed values written as text.
///
/// # Safety
///
/// `n` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>x<b>y</b><!--not text-->z</a>"));
///     assert_eq!(text(exiNodeGetContent(exiDocGetRootElement(d))), "xyz");
///     assert!(exiNodeGetContent(null()).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeGetContent(n: *const RawNode) -> *mut exiChar {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some((d, id)) = (unsafe { node(n as *const RawCommon) }) else {
            return ptr::null_mut();
        };
        out_str(&d.content(id))
    })
}

fn set_content(n: exiNodePtr, s: Option<&str>) -> c_int {
    guard(-1, || {
        // SAFETY: a live node from the caller.
        let Some((mut d, id)) = (unsafe { node(n as *const RawCommon) })
        else {
            return -1;
        };
        if d.set_content(id, s.unwrap_or("")).is_ok() { 0 } else { -1 }
    })
}

/// `xmlNodeSetContent`: 0, or -1.
///
/// # Safety
///
/// `n` is live; `content` NULL or NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>old<b/></a>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(exiNodeSetContent(a, s!("new")), 0);
///     assert_eq!(text(exiNodeGetContent(a)), "new");
///     assert_eq!(exiChildElementCount(a), 0);                      // children replaced
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeSetContent(
    n: exiNodePtr,
    content: *const exiChar,
) -> c_int {
    // SAFETY: the caller's contract.
    set_content(n, unsafe { cs(content) })
}

/// `xmlNodeSetContentLen`.
///
/// # Safety
///
/// `n` is live; `content` valid for `len` bytes.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>old</a>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(exiNodeSetContentLen(a, s!("newer"), 3), 0);
///     assert_eq!(text(exiNodeGetContent(a)), "new");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeSetContentLen(
    n: exiNodePtr,
    content: *const exiChar,
    len: c_int,
) -> c_int {
    // SAFETY: the caller's contract.
    set_content(n, unsafe { cs_len(content, len) })
}

fn add_content(n: exiNodePtr, s: Option<&str>) -> c_int {
    guard(-1, || {
        // SAFETY: a live node from the caller.
        let (Some((mut d, id)), Some(s)) =
            (unsafe { node(n as *const RawCommon) }, s)
        else {
            return -1;
        };
        if d.add_content(id, s).is_ok() { 0 } else { -1 }
    })
}

/// `xmlNodeAddContent`.
///
/// # Safety
///
/// `n` is live; `content` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>one</a>"));
///     let a = exiDocGetRootElement(d);
///     exiNodeAddContent(a, s!(", two"));
///     assert_eq!(text(exiNodeGetContent(a)), "one, two");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeAddContent(
    n: exiNodePtr,
    content: *const exiChar,
) -> c_int {
    // SAFETY: the caller's contract.
    add_content(n, unsafe { cs(content) })
}

/// `xmlNodeAddContentLen`.
///
/// # Safety
///
/// `n` is live; `content` valid for `len` bytes.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>one</a>"));
///     let a = exiDocGetRootElement(d);
///     exiNodeAddContentLen(a, s!("+2 more"), 2);
///     assert_eq!(text(exiNodeGetContent(a)), "one+2");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeAddContentLen(
    n: exiNodePtr,
    content: *const exiChar,
    len: c_int,
) -> c_int {
    // SAFETY: the caller's contract.
    add_content(n, unsafe { cs_len(content, len) })
}

/// `xmlNodeListGetString`: the text of `list` and its following
/// siblings; entity references as `&name;` unless `inLine`. NULL for an
/// empty result.
///
/// # Safety
///
/// `list` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>x</a>"));
///     let a = exiDocGetRootElement(d);
///     exiAddChild(a, exiNewReference(d, s!("e")));
///     exiAddChild(a, exiNewDocText(d, s!("y")));
///     assert_eq!(text(exiNodeListGetString(d, (*a).children, 0)), "x&e;y");
///     assert_eq!(text(exiNodeListGetString(d, (*a).children, 1)), "xy");
///     assert!(exiNodeListGetString(d, null(), 0).is_null());       // empty
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeListGetString(
    _d: exiDocPtr,
    list: *const RawNode,
    inLine: c_int,
) -> *mut exiChar {
    guard(ptr::null_mut(), || {
        let mut out = String::new();
        let mut c = list;
        while !c.is_null() {
            // SAFETY: walking the caller's live list.
            unsafe {
                if let Some((d, id)) = node(c as *const RawCommon) {
                    match d.kind(id) {
                        NodeKind::Text => out.push_str(&d.content(id)),
                        NodeKind::EntityReference if inLine == 0 => {
                            out.push('&');
                            out.push_str(d.node_name(id));
                            out.push(';');
                        }
                        _ => {}
                    }
                }
                c = (*c).next;
            }
        }
        if out.is_empty() { ptr::null_mut() } else { out_str(&out) }
    })
}

/// `xmlNodeIsText`.
///
/// # Safety
///
/// `n` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>t</a>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!((exiNodeIsText((*a).children), exiNodeIsText(a), exiNodeIsText(null())), (1, 0, 0));
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeIsText(n: *const RawNode) -> c_int {
    // SAFETY: the caller's contract.
    (!n.is_null() && unsafe { (*n).r#type } == EXI_TEXT_NODE) as c_int
}

/// `xmlIsBlankNode`.
///
/// # Safety
///
/// `n` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a> <b/>x</a>"));
///     let a = exiDocGetRootElement(d);
///     let blank = (*a).children;
///     assert_eq!(exiIsBlankNode(blank), 1);
///     assert_eq!(exiIsBlankNode((*a).last), 0);                    // "x"
///     assert_eq!(exiIsBlankNode(a), 0);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiIsBlankNode(n: *const RawNode) -> c_int {
    guard(0, || {
        // SAFETY: the caller's contract.
        unsafe { node(n as *const RawCommon) }
            .is_some_and(|(d, id)| d.is_blank(id)) as c_int
    })
}

/// `xmlNodeSetName`.
///
/// # Safety
///
/// `n` is live; `name` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<x:a xmlns:x='urn:x'/>"));
///     let a = exiDocGetRootElement(d);
///     let ns = (*a).ns;
///     exiNodeSetName(a, s!("z"));
///     assert_eq!(name(a), "z");
///     assert_eq!((*a).ns, ns);                                     // the namespace stays
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeSetName(n: exiNodePtr, name: *const exiChar) {
    guard((), || {
        // SAFETY: the caller's contract.
        let (Some((mut d, id)), Some(local)) =
            (unsafe { node(n as *const RawCommon) }, unsafe { cs(name) })
        else {
            return;
        };
        let (uri, prefix) = match d.name(id) {
            Some(nm) => {
                (nm.uri().to_string(), nm.prefix().map(str::to_string))
            }
            None => (String::new(), None),
        };
        let mut nm = Name::new(&uri, local);
        if let Some(p) = &prefix {
            nm = nm.with_prefix(p);
        }
        let _ = d.rename(id, nm);
    })
}

/// `xmlGetLineNo`: the line of a node read from XML text, or -1.
///
/// # Safety
///
/// `n` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>\n\n<b/></a>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(exiGetLineNo(a), 1);
///     assert_eq!(exiGetLineNo((*a).last), 3);
///     assert_eq!(exiGetLineNo(exiNewDocNode(d, null_mut(), s!("new"), null())), -1);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiGetLineNo(n: *const RawNode) -> c_long {
    // SAFETY: the caller's contract.
    match unsafe { n.as_ref() } {
        Some(n) if n.line != 0 => c_long::from(n.line),
        _ => -1,
    }
}

/// `xmlGetNodePath`: malloc'd.
///
/// # Safety
///
/// `n` is NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/><a/></r>"));
///     let r = exiDocGetRootElement(d);
///     assert_eq!(text(exiGetNodePath(r)), "/r");
///     assert_eq!(text(exiGetNodePath((*r).last)), "/r/a[2]");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiGetNodePath(n: *const RawNode) -> *mut exiChar {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some((d, id)) = (unsafe { node(n as *const RawCommon) }) else {
            return ptr::null_mut();
        };
        out_str(&d.path(id))
    })
}

/// A text node's or attribute's value, typed as it came: `out` is valid
/// until the next `exiNodeGetValue` on the same document or a change to
/// the node. `EXI_E_ARG` for other nodes.
///
/// # Safety
///
/// `n` is live; `out` points to an `exi_value`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     use exi::capi::{EXI_E_ARG, EXI_OK, EXI_V_STRING, exi_value};
///     let d = exiParseDoc(s!("<a>5</a>"));
///     let a = exiDocGetRootElement(d);
///     let mut v = core::mem::zeroed::<exi_value>();
///     assert_eq!(exiNodeGetValue((*a).children, &mut v), EXI_OK);
///     assert_eq!(v.r#type, EXI_V_STRING);                          // schema-less: text
///     assert_eq!(core::slice::from_raw_parts(v.text.ptr.cast::<u8>(), v.text.len), b"5");
///     assert_eq!(exiNodeGetValue(a, &mut v), EXI_E_ARG);           // not a text node
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeGetValue(
    n: *const RawNode,
    out: *mut exi_value,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller's contract.
        let Some((mut d, id)) = (unsafe { node(n as *const RawCommon) })
        else {
            return EXI_E_ARG;
        };
        if out.is_null() {
            return EXI_E_ARG;
        }
        let mut sc = match d
            .scratch()
            .take()
            .and_then(|b| b.downcast::<Scratch>().ok())
        {
            Some(s) => s,
            None => Box::new(Scratch::default()),
        };
        sc.clear();
        let r = match d.value(id) {
            Some(v) => {
                let c = sc.value(v, false);
                // SAFETY: the caller's pointer.
                unsafe { *out = c };
                EXI_OK
            }
            None => EXI_E_ARG,
        };
        *d.scratch() = Some(sc);
        r
    })
}

/// Sets a text node's, attribute's or element's content to a typed
/// value.
///
/// # Safety
///
/// `n` is live; `v` points to a filled `exi_value`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     use exi::capi::{EXI_OK, EXI_V_INT, exi_value};
///     let d = exiParseDoc(s!("<a>old</a>"));
///     let a = exiDocGetRootElement(d);
///     let mut v = core::mem::zeroed::<exi_value>();
///     v.r#type = EXI_V_INT;
///     v.u.integer = -42;
///     assert_eq!(exiNodeSetValue((*a).children, &v), EXI_OK);
///     assert_eq!(text(exiNodeGetContent(a)), "-42");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeSetValue(
    n: exiNodePtr,
    v: *const exi_value,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller's contract.
        let Some((mut d, id)) = (unsafe { node(n as *const RawCommon) })
        else {
            return EXI_E_ARG;
        };
        // SAFETY: as above.
        let Some(v) = (unsafe { v.as_ref().and_then(|v| value_from(v)) })
        else {
            return EXI_E_ARG;
        };
        if d.set_content_value(id, v).is_ok() { EXI_OK } else { EXI_E_ARG }
    })
}

// ----- attributes -----

/// The element and a name from (`ns`, `name`).
fn attr_name<'a>(ns: *const RawNs, name: &'a str) -> Name<'a> {
    // SAFETY: the caller's live namespace or NULL.
    let (uri, prefix) = unsafe { ns_name(ns) };
    let n = Name::new(uri, name);
    match prefix {
        Some(p) => n.with_prefix(p),
        None => n,
    }
}

/// `name` as `prefix:local` resolved at `el`, as `xmlSetProp` does;
/// otherwise a name in no namespace.
fn qname_at<'a>(
    d: &mut Document<'_>,
    el: NodeId,
    name: &'a str,
) -> (*mut RawNs, &'a str) {
    if let Some((p, local)) = name.split_once(':') {
        let ns = d.ns_raw(el, Some(p));
        if !ns.is_null() {
            return (ns, local);
        }
    }
    (ptr::null_mut(), name)
}

fn set_prop(
    n: exiNodePtr,
    ns: *const RawNs,
    name: *const exiChar,
    value: *const exiChar,
    resolve: bool,
) -> exiAttrPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's live node or NULL, and strings.
        let Some(local) = (unsafe { cs(name) }) else {
            return ptr::null_mut();
        };
        // SAFETY: as above.
        let value = unsafe { cs(value) }.unwrap_or("");
        if n.is_null() {
            // A detached attribute, in the nursery.
            // SAFETY: the nursery is live.
            let Some(mut d) = (unsafe { doc(nursery()) }) else {
                return ptr::null_mut();
            };
            return match d.new_attribute(attr_name(ns, local), value) {
                Ok(a) => d.raw(a) as exiAttrPtr,
                Err(_) => ptr::null_mut(),
            };
        }
        // SAFETY: as above.
        let Some((mut d, el)) = (unsafe { node(n as *const RawCommon) })
        else {
            return ptr::null_mut();
        };
        let (ns, local) = if resolve && ns.is_null() {
            qname_at(&mut d, el, local)
        } else {
            (ns as *mut RawNs, local)
        };
        match d.set_attribute(el, attr_name(ns, local), value) {
            Ok(a) => d.raw(a) as exiAttrPtr,
            Err(_) => ptr::null_mut(),
        }
    })
}

/// `xmlNewProp`: an attribute on `node` (one of the same name gets the
/// new value), or with `node` NULL a detached one.
///
/// # Safety
///
/// `node` NULL or live; strings NUL-terminated (`value` may be NULL).
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let a = exiDocGetRootElement(d);
///     let k = exiNewProp(a, s!("k"), s!("1"));
///     assert_eq!((*k).r#type, EXI_ATTRIBUTE_NODE);
///     assert_eq!(exiNewProp(a, s!("k"), s!("2")), exiHasProp(a, s!("k")));  // one k
///     assert_eq!(text(exiGetProp(a, s!("k"))), "2");
///     let loose = exiNewProp(null_mut(), s!("j"), s!("3"));        // detached
///     exiAddChild(a, loose.cast());                                // onto `a`
///     assert_eq!(text(exiGetProp(a, s!("j"))), "3");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewProp(
    n: exiNodePtr,
    name: *const exiChar,
    value: *const exiChar,
) -> exiAttrPtr {
    set_prop(n, ptr::null(), name, value, false)
}

/// `xmlNewNsProp`.
///
/// # Safety
///
/// As `exiNewProp`; `ns` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let a = exiDocGetRootElement(d);
///     let x = exiNewNs(a, s!("urn:x"), s!("x"));
///     let k = exiNewNsProp(a, x, s!("k"), s!("v"));
///     assert_eq!((*k).ns, x);
///     assert_eq!(text(exiGetNsProp(a, s!("k"), s!("urn:x"))), "v");
///     assert!(exiGetNoNsProp(a, s!("k")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewNsProp(
    n: exiNodePtr,
    ns: exiNsPtr,
    name: *const exiChar,
    value: *const exiChar,
) -> exiAttrPtr {
    set_prop(n, ns, name, value, false)
}

/// `xmlSetProp`: `name` may be `prefix:local`, resolved at `node`.
///
/// # Safety
///
/// `node` live; strings NUL-terminated (`value` may be NULL).
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x'/>"));
///     let a = exiDocGetRootElement(d);
///     exiSetProp(a, s!("x:k"), s!("in x"));                        // the prefix resolved at `a`
///     exiSetProp(a, s!("k"), s!("in none"));
///     assert_eq!(text(exiGetNsProp(a, s!("k"), s!("urn:x"))), "in x");
///     assert_eq!(text(exiGetNoNsProp(a, s!("k"))), "in none");
///     assert!(exiSetProp(null_mut(), s!("k"), s!("v")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSetProp(
    n: exiNodePtr,
    name: *const exiChar,
    value: *const exiChar,
) -> exiAttrPtr {
    if n.is_null() {
        return ptr::null_mut();
    }
    set_prop(n, ptr::null(), name, value, true)
}

/// `xmlSetNsProp`.
///
/// # Safety
///
/// As `exiSetProp`; `ns` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let a = exiDocGetRootElement(d);
///     let x = exiNewNs(a, s!("urn:x"), s!("x"));
///     exiSetNsProp(a, x, s!("k"), s!("1"));
///     exiSetNsProp(a, x, s!("k"), s!("2"));                        // replaces
///     assert_eq!(text(exiGetNsProp(a, s!("k"), s!("urn:x"))), "2");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSetNsProp(
    n: exiNodePtr,
    ns: exiNsPtr,
    name: *const exiChar,
    value: *const exiChar,
) -> exiAttrPtr {
    if n.is_null() {
        return ptr::null_mut();
    }
    set_prop(n, ns, name, value, false)
}

/// The attribute of `n` named `local`: in namespace `uri`, in none
/// (`Some("")`), or in any (`None`, as `xmlHasProp`).
fn find_prop(n: *const RawNode, local: &str, uri: Option<&str>) -> exiAttrPtr {
    // SAFETY: walking a live element's attributes.
    unsafe {
        let Some((d, el)) = node(n as *const RawCommon) else {
            return ptr::null_mut();
        };
        for a in d.attributes(el) {
            if let Some(nm) = d.name(a) {
                if nm.local_name() == local
                    && uri.is_none_or(|u| u == nm.uri())
                {
                    return d.raw(a) as exiAttrPtr;
                }
            }
        }
    }
    ptr::null_mut()
}

/// `xmlHasProp`: the attribute named `name` in any namespace.
///
/// # Safety
///
/// `n` NULL or live; `name` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x' x:k='1'/>"));
///     let a = exiDocGetRootElement(d);
///     let k = exiHasProp(a, s!("k"));                              // any namespace
///     assert_eq!((*k).r#type, EXI_ATTRIBUTE_NODE);
///     assert!(exiHasProp(a, s!("missing")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiHasProp(
    n: *const RawNode,
    name: *const exiChar,
) -> exiAttrPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        unsafe { cs(name) }.map_or(ptr::null_mut(), |l| find_prop(n, l, None))
    })
}

/// `xmlHasNsProp`: `nameSpace` NULL for no namespace.
///
/// # Safety
///
/// As `exiHasProp`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x' x:k='1' j='2'/>"));
///     let a = exiDocGetRootElement(d);
///     assert!(!exiHasNsProp(a, s!("k"), s!("urn:x")).is_null());
///     assert!(exiHasNsProp(a, s!("k"), null()).is_null());         // not in no namespace
///     assert!(!exiHasNsProp(a, s!("j"), null()).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiHasNsProp(
    n: *const RawNode,
    name: *const exiChar,
    nameSpace: *const exiChar,
) -> exiAttrPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let (Some(l), uri) = (unsafe { cs(name) }, unsafe { cs(nameSpace) })
        else {
            return ptr::null_mut();
        };
        find_prop(n, l, Some(uri.unwrap_or("")))
    })
}

fn prop_value(a: exiAttrPtr) -> *mut exiChar {
    if a.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: a live attribute found above.
    match unsafe { node(a as *const RawCommon) } {
        Some((d, id)) => out_str(&d.content(id)),
        None => ptr::null_mut(),
    }
}

/// `xmlGetProp`: malloc'd, or NULL. Any namespace.
///
/// # Safety
///
/// As `exiHasProp`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a k='v &amp; w'/>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(text(exiGetProp(a, s!("k"))), "v & w");
///     assert!(exiGetProp(a, s!("j")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiGetProp(
    n: *const RawNode,
    name: *const exiChar,
) -> *mut exiChar {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        prop_value(unsafe { exiHasProp(n, name) })
    })
}

/// `xmlGetNoNsProp`.
///
/// # Safety
///
/// As `exiHasProp`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x' x:k='in x' k='in none'/>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(text(exiGetNoNsProp(a, s!("k"))), "in none");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiGetNoNsProp(
    n: *const RawNode,
    name: *const exiChar,
) -> *mut exiChar {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        prop_value(unsafe { exiHasNsProp(n, name, ptr::null()) })
    })
}

/// `xmlGetNsProp`.
///
/// # Safety
///
/// As `exiHasProp`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x' x:k='in x' k='in none'/>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(text(exiGetNsProp(a, s!("k"), s!("urn:x"))), "in x");
///     assert!(exiGetNsProp(a, s!("k"), s!("urn:y")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiGetNsProp(
    n: *const RawNode,
    name: *const exiChar,
    nameSpace: *const exiChar,
) -> *mut exiChar {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        prop_value(unsafe { exiHasNsProp(n, name, nameSpace) })
    })
}

/// `xmlRemoveProp`: unlinks and frees; 0, or -1.
///
/// # Safety
///
/// `a` NULL or live, not used again.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a k='v'/>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(exiRemoveProp(exiHasProp(a, s!("k"))), 0);
///     assert!(exiHasProp(a, s!("k")).is_null());
///     assert_eq!(exiRemoveProp(null_mut()), -1);
///     assert_eq!(exiRemoveProp(a.cast()), -1);                     // not an attribute
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiRemoveProp(a: exiAttrPtr) -> c_int {
    guard(-1, || {
        // SAFETY: the caller's contract.
        match unsafe { node(a as *const RawCommon) } {
            Some((mut d, id)) if d.kind(id) == NodeKind::Attribute => {
                d.remove(id);
                0
            }
            _ => -1,
        }
    })
}

/// `xmlFreeProp`.
///
/// # Safety
///
/// As `exiRemoveProp`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a k='v'/>"));
///     let a = exiDocGetRootElement(d);
///     exiFreeProp(exiHasProp(a, s!("k")));
///     assert!((*a).properties.is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiFreeProp(a: exiAttrPtr) {
    // SAFETY: the caller's contract.
    unsafe { exiRemoveProp(a) };
}

/// `xmlUnsetProp`: the attribute in no namespace. 0, or -1 if there was
/// none.
///
/// # Safety
///
/// As `exiHasProp`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a k='v'/>"));
///     let a = exiDocGetRootElement(d);
///     assert_eq!(exiUnsetProp(a, s!("k")), 0);
///     assert_eq!(exiUnsetProp(a, s!("k")), -1);                    // none left
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiUnsetProp(
    n: exiNodePtr,
    name: *const exiChar,
) -> c_int {
    // SAFETY: the caller's contract.
    unsafe { exiRemoveProp(exiHasNsProp(n, name, ptr::null())) }
}

/// `xmlUnsetNsProp`.
///
/// # Safety
///
/// As `exiHasProp`; `ns` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x' x:k='1' k='2'/>"));
///     let a = exiDocGetRootElement(d);
///     let x = exiSearchNs(d, a, s!("x"));
///     assert_eq!(exiUnsetNsProp(a, x, s!("k")), 0);
///     assert_eq!(text(exiGetProp(a, s!("k"))), "2");               // the one in no namespace
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiUnsetNsProp(
    n: exiNodePtr,
    ns: exiNsPtr,
    name: *const exiChar,
) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        let href = if ns.is_null() { ptr::null() } else { (*ns).href };
        exiRemoveProp(exiHasNsProp(n, name, href))
    }
}

// ----- namespaces -----

/// `xmlNewNs`: declares `prefix` (NULL: the default namespace) for
/// `href` on `node`.
///
/// # Safety
///
/// `n` live; strings NUL-terminated (`prefix` may be NULL).
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a/>"));
///     let a = exiDocGetRootElement(d);
///     let x = exiNewNs(a, s!("urn:x"), s!("x"));
///     assert_eq!(core::ffi::CStr::from_ptr((*x).href.cast()).to_str().unwrap(), "urn:x");
///     assert_eq!((*a).nsDef, x);                                   // declared on `a`
///     // A default namespace would move `a`, which is in none, into it: refused.
///     assert!(exiNewNs(a, s!("urn:d"), null()).is_null());
///     let d2 = exiParseDoc(s!("<b xmlns='urn:d'/>"));
///     let dflt = exiNewNs(exiDocGetRootElement(d2), s!("urn:d"), null());
///     assert!((*dflt).prefix.is_null());
///     exiFreeDoc(d);
///     exiFreeDoc(d2);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNewNs(
    n: exiNodePtr,
    href: *const exiChar,
    prefix: *const exiChar,
) -> exiNsPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let (Some((mut d, el)), Some(uri)) =
            (unsafe { node(n as *const RawCommon) }, unsafe { cs(href) })
        else {
            return ptr::null_mut();
        };
        // SAFETY: as above.
        d.declare_ns_raw(el, unsafe { cs(prefix) }, uri)
            .unwrap_or(ptr::null_mut())
    })
}

/// `xmlSearchNs`: the declaration `prefix` (NULL: default) is bound to
/// at `node`.
///
/// # Safety
///
/// `n` live; `prefix` NULL or NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns='urn:d' xmlns:x='urn:x'><b/></a>"));
///     let b = (*exiDocGetRootElement(d)).children;
///     let x = exiSearchNs(d, b, s!("x"));                          // found on the parent
///     assert_eq!(core::ffi::CStr::from_ptr((*x).href.cast()).to_str().unwrap(), "urn:x");
///     assert!(!exiSearchNs(d, b, null()).is_null());               // the default
///     assert!(exiSearchNs(d, b, s!("y")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSearchNs(
    _d: exiDocPtr,
    n: exiNodePtr,
    prefix: *const exiChar,
) -> exiNsPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some((mut d, id)) = (unsafe { node(n as *const RawCommon) })
        else {
            return ptr::null_mut();
        };
        // SAFETY: as above.
        d.ns_raw(id, unsafe { cs(prefix) })
    })
}

/// `xmlSearchNsByHref`.
///
/// # Safety
///
/// `n` live; `href` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x'><b/></a>"));
///     let b = (*exiDocGetRootElement(d)).children;
///     let x = exiSearchNsByHref(d, b, s!("urn:x"));
///     assert_eq!(core::ffi::CStr::from_ptr((*x).prefix.cast()).to_str().unwrap(), "x");
///     assert!(exiSearchNsByHref(d, b, s!("urn:y")).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSearchNsByHref(
    _d: exiDocPtr,
    n: exiNodePtr,
    href: *const exiChar,
) -> exiNsPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let (Some((mut d, id)), Some(uri)) =
            (unsafe { node(n as *const RawCommon) }, unsafe { cs(href) })
        else {
            return ptr::null_mut();
        };
        d.ns_raw_by_href(id, uri)
    })
}

/// `xmlSetNs`: puts an element or attribute in `ns`'s namespace (NULL:
/// none).
///
/// # Safety
///
/// `n` live; `ns` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x'><b/></a>"));
///     let a = exiDocGetRootElement(d);
///     let b = (*a).children;
///     let x = exiSearchNs(d, a, s!("x"));
///     exiSetNs(b, x);
///     assert_eq!((*b).ns, x);
///     exiSetNs(b, null_mut());                                     // no namespace
///     assert!((*b).ns.is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSetNs(n: exiNodePtr, ns: exiNsPtr) {
    guard((), || {
        // SAFETY: the caller's contract.
        let Some((mut d, id)) = (unsafe { node(n as *const RawCommon) })
        else {
            return;
        };
        let Some(local) = d.name(id).map(|nm| nm.local_name().to_string())
        else {
            return;
        };
        let _ = d.rename(id, attr_name(ns, &local));
    })
}

/// `xmlGetNsList`: the declarations in scope at `node`, nearest first,
/// one per prefix, as a malloc'd NULL-terminated array (free with
/// `exiFree`); NULL if none.
///
/// # Safety
///
/// `n` is live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns:x='urn:x' xmlns:y='urn:y'><b xmlns:x='urn:x2'/></a>"));
///     let b = (*exiDocGetRootElement(d)).children;
///     let list = exiGetNsList(d, b);
///     let mut hrefs = Vec::new();
///     let mut i = 0;
///     while !(*list.add(i)).is_null() {
///         hrefs.push(core::ffi::CStr::from_ptr((**list.add(i)).href.cast()).to_str().unwrap());
///         i += 1;
///     }
///     assert_eq!(hrefs, ["urn:x2", "urn:y"]);                     // nearest first, one per prefix
///     exiFree(list.cast());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiGetNsList(
    _d: *const RawDoc,
    n: *const RawNode,
) -> *mut exiNsPtr {
    guard(ptr::null_mut(), || {
        let mut found: Vec<exiNsPtr> = Vec::new();
        let mut e = n;
        // SAFETY: walking live ancestors and their declarations.
        unsafe {
            while !e.is_null() && (*e).r#type == EXI_ELEMENT_NODE {
                let mut ns = (*e).nsDef;
                while !ns.is_null() {
                    if !found
                        .iter()
                        .any(|&o| cs((*o).prefix) == cs((*ns).prefix))
                    {
                        found.push(ns);
                    }
                    ns = (*ns).next;
                }
                e = (*e).parent;
            }
            if found.is_empty() {
                return ptr::null_mut();
            }
            let out =
                malloc((found.len() + 1) * core::mem::size_of::<exiNsPtr>())
                    as *mut exiNsPtr;
            if !out.is_null() {
                for (i, &ns) in found.iter().enumerate() {
                    *out.add(i) = ns;
                }
                *out.add(found.len()) = ptr::null_mut();
            }
            out
        }
    })
}

// ----- element walks (on the structs, no lookups) -----

/// # Safety
///
/// `c` NULL or live.
unsafe fn elem_from(mut c: *mut RawNode, forward: bool) -> exiNodePtr {
    // SAFETY: the caller's contract.
    unsafe {
        while !c.is_null() && (*c).r#type != EXI_ELEMENT_NODE {
            c = if forward { (*c).next } else { (*c).prev };
        }
    }
    c
}

/// `xmlFirstElementChild`.
///
/// # Safety
///
/// `n` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r>t<a/>u<b/>v</r>"));
///     let r = exiDocGetRootElement(d);
///     assert_eq!(name(exiFirstElementChild(r)), "a");              // past the text
///     assert!(exiFirstElementChild(exiFirstElementChild(r)).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiFirstElementChild(n: exiNodePtr) -> exiNodePtr {
    // SAFETY: the caller's contract.
    match unsafe { n.as_ref() } {
        // SAFETY: as above.
        Some(n) => unsafe { elem_from(n.children, true) },
        None => ptr::null_mut(),
    }
}

/// `xmlLastElementChild`.
///
/// # Safety
///
/// `n` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r>t<a/>u<b/>v</r>"));
///     let r = exiDocGetRootElement(d);
///     assert_eq!(name(exiLastElementChild(r)), "b");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiLastElementChild(n: exiNodePtr) -> exiNodePtr {
    // SAFETY: the caller's contract.
    match unsafe { n.as_ref() } {
        // SAFETY: as above.
        Some(n) => unsafe { elem_from(n.last, false) },
        None => ptr::null_mut(),
    }
}

/// `xmlNextElementSibling`.
///
/// # Safety
///
/// `n` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/>u<b/></r>"));
///     let r = exiDocGetRootElement(d);
///     let b = exiNextElementSibling(exiFirstElementChild(r));
///     assert_eq!(name(b), "b");
///     assert!(exiNextElementSibling(b).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNextElementSibling(n: exiNodePtr) -> exiNodePtr {
    // SAFETY: the caller's contract.
    match unsafe { n.as_ref() } {
        // SAFETY: as above.
        Some(n) => unsafe { elem_from(n.next, true) },
        None => ptr::null_mut(),
    }
}

/// `xmlPreviousElementSibling`.
///
/// # Safety
///
/// `n` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r><a/>u<b/></r>"));
///     let r = exiDocGetRootElement(d);
///     assert_eq!(name(exiPreviousElementSibling(exiLastElementChild(r))), "a");
///     assert!(exiPreviousElementSibling(null_mut()).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiPreviousElementSibling(
    n: exiNodePtr,
) -> exiNodePtr {
    // SAFETY: the caller's contract.
    match unsafe { n.as_ref() } {
        // SAFETY: as above.
        Some(n) => unsafe { elem_from(n.prev, false) },
        None => ptr::null_mut(),
    }
}

/// `xmlChildElementCount`.
///
/// # Safety
///
/// `n` NULL or live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r>t<a/><!--c--><b/></r>"));
///     assert_eq!(exiChildElementCount(exiDocGetRootElement(d)), 2);
///     assert_eq!(exiChildElementCount(null_mut()), 0);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiChildElementCount(n: exiNodePtr) -> c_ulong {
    let mut k = 0;
    // SAFETY: the caller's contract.
    unsafe {
        let mut c = if n.is_null() {
            ptr::null_mut()
        } else {
            elem_from((*n).children, true)
        };
        while !c.is_null() {
            k += 1;
            c = elem_from((*c).next, true);
        }
    }
    k
}

/// The global ids of a name, if the document's tables hold it: compare
/// with a node's `uriId` and `localId`. 1 if found, else 0.
///
/// # Safety
///
/// `d` live; strings NUL-terminated; the outputs point to `uint32_t`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a xmlns='urn:x'/>"));
///     let a = exiDocGetRootElement(d);
///     let (mut uri, mut local) = (0, 0);
///     assert_eq!(exiNameLookup(d, s!("urn:x"), s!("a"), &mut uri, &mut local), 1);
///     assert_eq!(((*a).uriId, (*a).localId), (uri, local));        // compare names as numbers
///     assert_eq!(exiNameLookup(d, s!("urn:x"), s!("never-seen"), &mut uri, &mut local), 0);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNameLookup(
    d: *const RawDoc,
    uri: *const exiChar,
    local: *const exiChar,
    uriId: *mut u32,
    localId: *mut u32,
) -> c_int {
    guard(0, || {
        // SAFETY: the caller's contract.
        unsafe {
            let (Some(d), Some(l)) = (doc(d), cs(local)) else { return 0 };
            match d.lookup_name(cs(uri).unwrap_or(""), l) {
                Some(g) => {
                    if !uriId.is_null() {
                        *uriId = g.uri.index();
                    }
                    if !localId.is_null() {
                        *localId = g.local.index();
                    }
                    1
                }
                None => 0,
            }
        }
    })
}

// ----- reading and writing -----

fn read_xml(bytes: &[u8], url: Option<&str>, options: c_int) -> exiDocPtr {
    let s = default_strings();
    let b = DocumentBuilder::new(s)
        .ignore_blanks(options & EXI_PARSE_NOBLANKS != 0);
    match Document::parse_xml_with(s, bytes, b) {
        Ok(mut d) => {
            if let Some(u) = url {
                d.set_url(u);
            }
            d.into_raw()
        }
        Err(_) => ptr::null_mut(),
    }
}

/// `xmlReadMemory`: XML text. `options`: `EXI_PARSE_NOBLANKS`; others
/// are ignored.
///
/// # Safety
///
/// `buffer` valid for `size` bytes; `URL` NULL or NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let xml = b"<a> <b/> </a>";
///     let d = exiReadMemory(xml.as_ptr().cast(), xml.len() as _, c"mem:a".as_ptr(), null(), EXI_PARSE_NOBLANKS);
///     let a = exiDocGetRootElement(d);
///     assert_eq!(name((*a).children), "b");                        // no blank text
///     assert_eq!(core::ffi::CStr::from_ptr((*d).URL.cast()).to_str().unwrap(), "mem:a");
///     assert!(exiReadMemory(b"<a>".as_ptr().cast(), 3, null(), null(), 0).is_null());   // not well-formed
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiReadMemory(
    buffer: *const c_char,
    size: c_int,
    URL: *const c_char,
    _encoding: *const c_char,
    options: c_int,
) -> exiDocPtr {
    guard(ptr::null_mut(), || {
        if buffer.is_null() || size < 0 {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract.
        let bytes = unsafe {
            core::slice::from_raw_parts(buffer as *const u8, size as usize)
        };
        // SAFETY: as above.
        read_xml(bytes, unsafe { cs(URL as *const exiChar) }, options)
    })
}

/// `xmlReadDoc`: a NUL-terminated document.
///
/// # Safety
///
/// `cur` NUL-terminated; `URL` NULL or NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiReadDoc(s!("<a> <b/></a>"), null(), null(), 0);
///     assert_eq!(exiIsBlankNode((*exiDocGetRootElement(d)).children), 1);   // blanks kept
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiReadDoc(
    cur: *const exiChar,
    URL: *const c_char,
    _encoding: *const c_char,
    options: c_int,
) -> exiDocPtr {
    guard(ptr::null_mut(), || {
        if cur.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract.
        let bytes =
            unsafe { core::ffi::CStr::from_ptr(cur.cast()).to_bytes() };
        // SAFETY: as above.
        read_xml(bytes, unsafe { cs(URL as *const exiChar) }, options)
    })
}

/// `xmlReadFile`.
///
/// # Safety
///
/// `filename` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let path = std::env::temp_dir().join(format!("exi-read-file-{}.xml", std::process::id()));
///     std::fs::write(&path, "<a/>").unwrap();
///     let p = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
///     let d = exiReadFile(p.as_ptr(), null(), 0);
///     assert_eq!(name(exiDocGetRootElement(d)), "a");
///     exiFreeDoc(d);
///     std::fs::remove_file(&path).unwrap();
///     assert!(exiReadFile(p.as_ptr(), null(), 0).is_null());       // gone
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiReadFile(
    filename: *const c_char,
    _encoding: *const c_char,
    options: c_int,
) -> exiDocPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some(f) = (unsafe { cs(filename as *const exiChar) }) else {
            return ptr::null_mut();
        };
        match std::fs::read(f) {
            Ok(b) => read_xml(&b, Some(f), options),
            Err(_) => ptr::null_mut(),
        }
    })
}

/// `xmlParseMemory`.
///
/// # Safety
///
/// `buffer` valid for `size` bytes.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let xml = b"<a/>trailing";
///     assert!(exiParseMemory(xml.as_ptr().cast(), xml.len() as _).is_null());
///     let d = exiParseMemory(xml.as_ptr().cast(), 4);              // the first 4 octets
///     assert_eq!(name(exiDocGetRootElement(d)), "a");
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiParseMemory(
    buffer: *const c_char,
    size: c_int,
) -> exiDocPtr {
    // SAFETY: the caller's contract.
    unsafe { exiReadMemory(buffer, size, ptr::null(), ptr::null(), 0) }
}

/// `xmlParseDoc`.
///
/// # Safety
///
/// `cur` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a><b/></a>"));
///     assert_eq!(kids(exiDocGetRootElement(d)), ["b"]);
///     assert!(exiParseDoc(null()).is_null());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiParseDoc(cur: *const exiChar) -> exiDocPtr {
    // SAFETY: the caller's contract.
    unsafe { exiReadDoc(cur, ptr::null(), ptr::null(), 0) }
}

/// `xmlParseFile`.
///
/// # Safety
///
/// `filename` NUL-terminated.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let path = std::env::temp_dir().join(format!("exi-parse-file-{}.xml", std::process::id()));
///     std::fs::write(&path, "<a><b/></a>").unwrap();
///     let p = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
///     let d = exiParseFile(p.as_ptr());
///     assert_eq!(kids(exiDocGetRootElement(d)), ["b"]);
///     exiFreeDoc(d);
///     std::fs::remove_file(&path).unwrap();
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiParseFile(filename: *const c_char) -> exiDocPtr {
    // SAFETY: the caller's contract.
    unsafe { exiReadFile(filename, ptr::null(), 0) }
}

/// A document from every event of a layer 1 reader: EXI, or XML text
/// with `EXI_FORMAT_XML`. The reader must have all its input
/// (`EXI_FEED_FINAL`). Names go in `strings` (NULL: the thread's
/// tables); give the reader's own `exi_strings` for name ids that match
/// the reader's. NULL on error: `exi_reader_error` tells why.
///
/// # Safety
///
/// `r` is live; `strings` NULL or live and outliving the document.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     use exi::capi::*;
///     let mut cfg = core::mem::zeroed::<exi_reader_config>();
///     cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
///     cfg.format = EXI_FORMAT_XML;                                 // or EXI
///     let r = exi_reader_new(&cfg);
///     let xml = b"<a><b/></a>";
///     exi_reader_feed(r, xml.as_ptr().cast(), xml.len(), EXI_FEED_FINAL, null_mut());
///     let d = exiDocReadFrom(r, null_mut());
///     assert_eq!(kids(exiDocGetRootElement(d)), ["b"]);
///     exiFreeDoc(d);
///     exi_reader_free(r);
///     // Without all its input, the reader cannot give a whole document.
///     let r = exi_reader_new(&cfg);
///     exi_reader_feed(r, xml.as_ptr().cast(), 4, 0, null_mut());
///     assert!(exiDocReadFrom(r, null_mut()).is_null());
///     assert_eq!(exi_reader_error(r, null_mut(), null_mut(), null_mut()), EXI_E_TRUNCATED);
///     exi_reader_free(r);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocReadFrom(
    r: *mut exi_reader,
    strings: *mut exi_strings,
) -> exiDocPtr {
    guard(ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let Some(o) = (unsafe { super::reader(r) }) else {
            return ptr::null_mut();
        };
        if o.poisoned {
            return ptr::null_mut();
        }
        let s = if strings.is_null() {
            default_strings()
        } else {
            // SAFETY: the caller's contract.
            match unsafe { strings_of(strings) } {
                Some(s) => s,
                None => return ptr::null_mut(),
            }
        };
        let mut b = DocumentBuilder::new(s);
        let res: Result<()> = (|| loop {
            match o.reader.next()? {
                Step::Event(e, m) => b.event_at(&e, Some(&m))?,
                Step::NeedInput => {
                    return Err(Error::new(
                        crate::ErrorKind::Truncated,
                        "the reader needs more input",
                    ));
                }
                Step::End => return Ok(()),
            }
        })();
        match res.and_then(|()| b.finish()) {
            Ok(d) => d.into_raw(),
            Err(e) => {
                o.fail(e);
                ptr::null_mut()
            }
        }
    })
}

struct ToWriter<'a>(&'a mut super::imp::AnyWriter);

impl EventSink for ToWriter<'_> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        self.0.event(ev)
    }
}

/// Writes the whole document to a layer 1 writer (EXI, or XML text with
/// `EXI_FORMAT_XML`), ending it. Flush the writer after.
///
/// # Safety
///
/// `d` and `w` are live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     use exi::capi::*;
///     let mut o = core::mem::zeroed::<exi_options>();
///     exi_options_init(&mut o);
///     o.coding = EXI_CODING_BYTE_ALIGNED;
///     let mut cfg = core::mem::zeroed::<exi_writer_config>();
///     cfg.size = core::mem::size_of::<exi_writer_config>() as u32;
///     cfg.options = &o;
///     cfg.header = EXI_HDR_NO_OPTIONS;
///     let w = exi_writer_new(&cfg);
///     let (mut p, mut n) = (null(), 0);
///     let d = exiParseDoc(s!("<a/>"));
///     assert_eq!(exiDocWriteTo(d, w), EXI_OK);                     // ends the document
///     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
///     assert_eq!(core::slice::from_raw_parts(p, n), [0x80, 0x01, 0x02, b'a', 0x00]);
///     assert_eq!(exiDocWriteTo(d, null_mut()), EXI_E_ARG);
///     exiFreeDoc(d);
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocWriteTo(
    d: *const RawDoc,
    w: *mut exi_writer,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller's contract.
        let Some(d) = (unsafe { doc(d) }) else { return EXI_E_ARG };
        write_with(w, |s| d.write(s))
    })
}

/// Writes one element and everything in it, with the declarations it
/// needs: a stanza for an open stream.
///
/// # Safety
///
/// `n` and `w` are live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     use exi::capi::*;
///     let mut o = core::mem::zeroed::<exi_options>();
///     exi_options_init(&mut o);
///     o.coding = EXI_CODING_BYTE_ALIGNED;
///     let mut cfg = core::mem::zeroed::<exi_writer_config>();
///     cfg.size = core::mem::size_of::<exi_writer_config>() as u32;
///     cfg.options = &o;
///     cfg.header = EXI_HDR_NO_OPTIONS;
///     let w = exi_writer_new(&cfg);
///     let (mut p, mut n) = (null(), 0);
///     let d = exiParseDoc(s!("<stream><m/></stream>"));
///     let m = (*exiDocGetRootElement(d)).children;
///     assert_eq!(exiNodeWriteTo(m, w), EXI_OK);                    // one element, as a document
///     assert_eq!(exi_writer_end_document(w), EXI_OK);
///     assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
///     assert_eq!(core::slice::from_raw_parts(p, n), [0x80, 0x01, 0x02, b'm', 0x00]);
///     exiFreeDoc(d);
///     exi_writer_free(w);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeWriteTo(
    n: *const RawNode,
    w: *mut exi_writer,
) -> exi_status {
    guard(EXI_E_INTERNAL, || {
        // SAFETY: the caller's contract.
        let Some((d, id)) = (unsafe { node(n as *const RawCommon) }) else {
            return EXI_E_ARG;
        };
        write_with(w, |s| d.write_subtree(id, s))
    })
}

fn write_with(
    w: *mut exi_writer,
    f: impl FnOnce(&mut ToWriter<'_>) -> Result<()>,
) -> exi_status {
    // SAFETY: a live writer from the caller.
    let Some(o) = (unsafe { super::writer(w) }) else { return EXI_E_ARG };
    if o.poisoned {
        return EXI_E_STATE;
    }
    o.run(|aw| f(&mut ToWriter(aw)))
}

/// The document as libxml2 dumps it: the declaration, then each
/// top-level node on a line of its own.
fn dump(d: &Document<'_>) -> Option<Vec<u8>> {
    use crate::xml::{PrefixPolicy, XmlWriter, XmlWriterConfig};
    let mut out = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_vec();
    for n in d.children(d.document_node()) {
        let mut w = XmlWriter::new(
            Vec::new(),
            XmlWriterConfig::new().prefixes(PrefixPolicy::FromEvents),
        );
        d.write_subtree(n, &mut w).ok()?;
        out.extend_from_slice(&w.into_sink());
        out.push(b'\n');
    }
    Some(out)
}

/// `xmlDocDumpMemory`: XML text with a declaration, malloc'd (free with
/// `exiFree`).
///
/// # Safety
///
/// `d` live; `mem` and `size` point to writable storage.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<!--c--><a k='v'><b/></a>"));
///     let (mut mem, mut size) = (null_mut(), 0);
///     exiDocDumpMemory(d, &mut mem, &mut size);
///     let xml = text(mem);
///     assert_eq!(xml, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!--c-->\n<a k=\"v\"><b/></a>\n");
///     assert_eq!(size as usize, xml.len());
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocDumpMemory(
    d: *const RawDoc,
    mem: *mut *mut exiChar,
    size: *mut c_int,
) {
    guard((), || {
        if mem.is_null() {
            return;
        }
        // SAFETY: the caller's contract.
        unsafe {
            *mem = ptr::null_mut();
            if !size.is_null() {
                *size = 0;
            }
            let Some(d) = doc(d) else { return };
            let Some(b) = dump(&d) else { return };
            let Ok(n) = c_int::try_from(b.len()) else { return };
            // The text is UTF-8: the XML writer writes only that.
            *mem = out_str(core::str::from_utf8_unchecked(&b));
            if !size.is_null() && !(*mem).is_null() {
                *size = n;
            }
        }
    })
}

/// `xmlDocDumpFormatMemory`: `format` is ignored.
///
/// # Safety
///
/// As `exiDocDumpMemory`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a><b/></a>"));
///     let (mut mem, mut size) = (null_mut(), 0);
///     exiDocDumpFormatMemory(d, &mut mem, &mut size, 1);           // no indentation is added
///     assert!(text(mem).ends_with("<a><b/></a>\n"));
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiDocDumpFormatMemory(
    d: *const RawDoc,
    mem: *mut *mut exiChar,
    size: *mut c_int,
    _format: c_int,
) {
    // SAFETY: the caller's contract.
    unsafe { exiDocDumpMemory(d, mem, size) }
}

/// One element as XML text, malloc'd: the subtree with the declarations
/// it needs.
///
/// # Safety
///
/// As `exiDocDumpMemory`, for a node.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<r xmlns:x='urn:x'><x:m>hi</x:m></r>"));
///     let m = (*exiDocGetRootElement(d)).children;
///     let (mut mem, mut size) = (null_mut(), 0);
///     exiNodeDumpMemory(m, &mut mem, &mut size);
///     assert_eq!(text(mem), "<x:m xmlns:x=\"urn:x\">hi</x:m>");      // with what it needs
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiNodeDumpMemory(
    n: *const RawNode,
    mem: *mut *mut exiChar,
    size: *mut c_int,
) {
    guard((), || {
        if mem.is_null() {
            return;
        }
        // SAFETY: the caller's contract.
        unsafe {
            *mem = ptr::null_mut();
            let Some((d, id)) = node(n as *const RawCommon) else { return };
            let mut w = crate::xml::XmlWriter::new(
                Vec::new(),
                crate::xml::XmlWriterConfig::new()
                    .prefixes(crate::xml::PrefixPolicy::FromEvents),
            );
            if d.write_subtree(id, &mut w).is_err() {
                return;
            }
            let b = w.into_sink();
            let Ok(n) = c_int::try_from(b.len()) else { return };
            *mem = out_str(core::str::from_utf8_unchecked(&b));
            if !size.is_null() && !(*mem).is_null() {
                *size = n;
            }
        }
    })
}

/// `xmlSaveFile`: the octets written, or -1.
///
/// # Safety
///
/// `filename` NUL-terminated; `d` live.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let path = std::env::temp_dir().join(format!("exi-save-file-{}.xml", std::process::id()));
///     let p = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
///     let d = exiParseDoc(s!("<a/>"));
///     let n = exiSaveFile(p.as_ptr(), d);
///     let saved = std::fs::read_to_string(&path).unwrap();
///     assert_eq!(n as usize, saved.len());
///     assert!(saved.ends_with("<a/>\n"));
///     std::fs::remove_file(&path).unwrap();
///     assert_eq!(exiSaveFile(p.as_ptr(), null()), -1);
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSaveFile(
    filename: *const c_char,
    d: *const RawDoc,
) -> c_int {
    guard(-1, || {
        // SAFETY: the caller's contract.
        let (Some(f), Some(d)) =
            (unsafe { cs(filename as *const exiChar) }, unsafe { doc(d) })
        else {
            return -1;
        };
        match dump(&d) {
            Some(b) if std::fs::write(f, &b).is_ok() => {
                c_int::try_from(b.len()).unwrap_or(-1)
            }
            _ => -1,
        }
    })
}

/// `xmlSaveFormatFile`: `format` is ignored.
///
/// # Safety
///
/// As `exiSaveFile`.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let path = std::env::temp_dir().join(format!("exi-save-format-{}.xml", std::process::id()));
///     let p = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
///     let d = exiParseDoc(s!("<a/>"));
///     assert!(exiSaveFormatFile(p.as_ptr(), d, 1) > 0);
///     std::fs::remove_file(&path).unwrap();
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiSaveFormatFile(
    filename: *const c_char,
    d: *const RawDoc,
    _format: c_int,
) -> c_int {
    // SAFETY: the caller's contract.
    unsafe { exiSaveFile(filename, d) }
}

/// `xmlFree`: frees a string or array this API returned.
///
/// # Safety
///
/// `p` is NULL or from this API, freed once.
///
/// ```
/// # #[allow(dead_code, unused_imports)] mod fixture { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/doctest/c_dom.rs")); } use fixture::*;
/// unsafe {
///     let d = exiParseDoc(s!("<a>t</a>"));
///     let p = exiNodeGetContent(exiDocGetRootElement(d));          // malloc'd
///     exiFree(p.cast());
///     exiFree(null_mut());                                         // no-op
///     exiFreeDoc(d);
/// }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exiFree(p: *mut c_void) {
    // SAFETY: the caller's contract; the memory came from malloc.
    unsafe { free(p) }
}
