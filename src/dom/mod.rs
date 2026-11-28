// src/dom/mod.rs
//
// Document tree API
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! A [`Document`] holds a tree of nodes in libxml2's layout: elements,
//! attributes, text, comments, processing instructions, entity
//! references and a DOCTYPE. The operations are libxml2's tree
//! functions (`tree.h`), under Rust names:
//!
//! | libxml2 | here |
//! |---|---|
//! | `xmlNewDoc` | [`Document::new`] |
//! | `xmlDocGetRootElement`, `xmlDocSetRootElement` | [`root`](Document::root), [`set_root`](Document::set_root) |
//! | `xmlNewDocNode`, `xmlNewDocText`, `xmlNewDocComment`, `xmlNewDocPI` | [`new_element`](Document::new_element), [`new_text`](Document::new_text), [`new_comment`](Document::new_comment), [`new_pi`](Document::new_pi) |
//! | `xmlNewChild`, `xmlNewTextChild` | [`new_child`](Document::new_child) |
//! | `xmlAddChild`, `xmlAddPrevSibling`, `xmlAddNextSibling` | [`append_child`](Document::append_child), [`insert_before`](Document::insert_before), [`insert_after`](Document::insert_after) |
//! | `xmlUnlinkNode`, `xmlFreeNode`, `xmlReplaceNode` | [`unlink`](Document::unlink), [`remove`](Document::remove), [`replace`](Document::replace) |
//! | `xmlCopyNode`, `xmlDocCopyNode` | [`copy`](Document::copy), [`import`](Document::import) |
//! | `xmlNodeGetContent`, `xmlNodeSetContent`, `xmlNodeAddContent` | [`content`](Document::content), [`set_content`](Document::set_content), [`add_content`](Document::add_content) |
//! | `xmlSetProp`, `xmlSetNsProp`, `xmlGetProp`, `xmlGetNsProp`, `xmlHasNsProp`, `xmlUnsetNsProp` | [`set_attribute`](Document::set_attribute), [`attribute`](Document::attribute), [`attribute_node`](Document::attribute_node), [`remove_attribute`](Document::remove_attribute) |
//! | `xmlNewNs`, `xmlSearchNs`, `xmlSearchNsByHref` | [`declare_ns`](Document::declare_ns), [`search_ns`](Document::search_ns), [`search_ns_by_href`](Document::search_ns_by_href) |
//! | `xmlFirstElementChild`, `xmlNextElementSibling`, `xmlChildElementCount` … | [`first_element_child`](Document::first_element_child), [`next_element_sibling`](Document::next_element_sibling), [`child_element_count`](Document::child_element_count) … |
//! | `xmlGetNodePath`, `xmlIsBlankNode` | [`path`](Document::path), [`is_blank`](Document::is_blank) |
//!
//! C programs get the same tree as libxml2's structs (`exiNode`,
//! `exiAttr` … in `include/exi_dom.h`), so code that walks
//! `node->children` and `cur->next` ports with a change of prefix
//! ([`raw`]).
//!
//! # Where libexi differs from libxml2
//!
//! | | libxml2 | libexi |
//! |---|---|---|
//! | Names | strings | strings, and the name's ids in the global string tables: compare names as integers with [`global_name`](Document::global_name) and [`lookup_name`](Document::lookup_name) |
//! | Values | text | typed values from schema-informed streams stay typed ([`value`](Document::value)); text is made when asked for ([`content`](Document::content)) |
//! | Nodes | created without a document | created in a document; moved between documents with [`import`](Document::import) |
//! | Text | `xmlAddChild` merges adjacent text nodes | Rust: never merged. C: merged, as libxml2 does |
//! | Input and output | the parser and serializer | any layer 1 source and sink: XML text, EXI, or the application's own events ([`Document::read`], [`Document::write`]) |
//! | Handles in Rust | pointers | [`NodeId`]s, checked: an id of a freed node panics instead of reading freed memory |
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "xml")] {
//! use exi::dom::{Document, NodeKind};
//! use exi::strings::{GlobalConfig, GlobalStrings};
//!
//! let strings = GlobalStrings::new(GlobalConfig::new())?;
//! let mut doc = Document::parse_xml(&strings, br#"<feed xmlns="http://www.w3.org/2005/Atom"><title>News</title><entry id="1"/></feed>"#)?;
//! let feed = doc.root().unwrap();
//! let title = doc.first_element_child(feed).unwrap();
//! assert_eq!(doc.content(title), "News");
//!
//! let entry = doc.new_child(feed, ("http://www.w3.org/2005/Atom", "entry"), None)?;
//! doc.set_attribute(entry, "id", "2")?;
//! assert_eq!(doc.child_element_count(feed), 3);
//! assert_eq!(doc.attribute(entry, "id").as_deref(), Some("2"));
//!
//! // Names compare as integers.
//! let entry_name = doc.lookup_name("http://www.w3.org/2005/Atom", "entry").unwrap();
//! let entries = doc.children(feed).filter(|&n| doc.global_name(n) == Some(entry_name)).count();
//! assert_eq!(entries, 2);
//!
//! assert_eq!(doc.to_xml()?, r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>News</title><entry id="1"/><entry id="2"/></feed>"#);
//! # }
//! # Ok::<(), exi::Error>(())
//! ```

mod arena;
mod io;
pub mod raw;

pub use io::{DocEvents, DocumentBuilder};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::ptr::{self, NonNull};

use hashbrown::HashSet;
use rustc_hash::FxBuildHasher;

use crate::strings::{GlobalStrings, Local, Prefix, StrId, Uri};
use crate::{
    AsName, Doctype, Error, ErrorKind, GlobalName, Name, OwnedValue, Result,
    Value,
};
use arena::NodeArena;
use raw::*;

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn check_attribute_name(name: Name<'_>) -> Result<()> {
    if name.uri() == XMLNS_NS
        || name.prefix() == Some("xmlns")
        || name.is("", "xmlns")
    {
        return Err(arg("namespace declarations are made with declare_ns"));
    }
    Ok(())
}

/// A node of a [`Document`]: a slot number and a generation, so an id
/// kept after its node is freed is recognized instead of reaching
/// another node.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

/// What a node is. The C `type` field holds libxml2's numbers for these
/// ([`raw`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Document,
    Element,
    Attribute,
    Text,
    Comment,
    ProcessingInstruction,
    EntityReference,
    Doctype,
}

impl NodeKind {
    fn of(t: core::ffi::c_int) -> NodeKind {
        match t {
            EXI_DOCUMENT_NODE => NodeKind::Document,
            EXI_ELEMENT_NODE => NodeKind::Element,
            EXI_ATTRIBUTE_NODE => NodeKind::Attribute,
            EXI_TEXT_NODE | EXI_CDATA_SECTION_NODE => NodeKind::Text,
            EXI_COMMENT_NODE => NodeKind::Comment,
            EXI_PI_NODE => NodeKind::ProcessingInstruction,
            EXI_ENTITY_REF_NODE => NodeKind::EntityReference,
            _ => NodeKind::Doctype,
        }
    }
}

/// Where a missing namespace declaration is made.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Declare {
    /// On the element itself.
    Here,
    /// Prefixed ones on this ancestor (a copy's root); default ones on
    /// the element.
    At(*mut RawNode),
    /// Nowhere: the element is detached; its `ns` stays NULL until it
    /// is placed.
    No,
}

/// Distinct global string ids held once per document (released on drop).
/// Uses a small bitset for dense ids `< 16384` (3-instruction hot-path check)
/// and a hash set for higher ids.
struct IdSet {
    bits: Vec<u64>,
    big: HashSet<u32, FxBuildHasher>,
    held: Vec<u32>,
}

const BITSET_LIMIT: usize = 16384;

impl IdSet {
    fn new() -> Self {
        IdSet {
            bits: Vec::new(),
            big: HashSet::with_hasher(FxBuildHasher),
            held: Vec::new(),
        }
    }

    /// Marks `id` as held by this document; returns `true` if it was absent.
    #[inline]
    fn insert(&mut self, id: u32) -> bool {
        let idx = id as usize;
        if idx < BITSET_LIMIT {
            let word = idx >> 6;
            let bit = 1u64 << (idx & 63);
            if word < self.bits.len() && (self.bits[word] & bit) != 0 {
                return false;
            }
            return self.insert_bit_slow(id, word, bit);
        }
        self.insert_big(id)
    }

    #[cold]
    fn insert_bit_slow(&mut self, id: u32, word: usize, bit: u64) -> bool {
        if word >= self.bits.len() {
            self.bits.resize(word + 1, 0);
        }
        self.bits[word] |= bit;
        self.held.push(id);
        true
    }

    #[cold]
    fn insert_big(&mut self, id: u32) -> bool {
        if self.big.insert(id) {
            self.held.push(id);
            true
        } else {
            false
        }
    }
}

/// The document's heap block. `raw` is first, so a `*mut RawDoc` is a
/// `*mut DocInner`.
#[repr(C)]
pub(crate) struct DocInner<'g> {
    pub raw: RawDoc,
    pub strings: &'g GlobalStrings,
    arena: NodeArena,
    /// Slot pointers (null when freed), parallel to `gens`.
    ptrs: Vec<*mut RawCommon>,
    gens: Vec<u32>,
    free: Vec<u32>,
    /// Slots whose nodes own an external heap allocation (`EXI_DTD_NODE`,
    /// elements with `nsDef`, or `NodeBox` with `Text`/`Typed`/`Name`/`Pi`).
    heap_nodes: Vec<u32>,
    held_uris: IdSet,
    held_locals: IdSet,
    held_prefixes: IdSet,
    /// The number of the next generated prefix (`ns1`, `ns2`, …).
    ns_count: u32,
    /// True once any non-empty default namespace (`xmlns="..."`) has been
    /// declared or adopted in this document. When false, elements in no
    /// namespace cannot inherit a default namespace.
    has_default_ns: bool,
    url: Option<CBuf>,
    /// Storage the C API keeps per document (typed value views).
    pub(crate) c_scratch: Option<Box<dyn core::any::Any>>,
}

/// A document tree. See the [module documentation](self).
///
/// Every node belongs to one document and lives until it is removed or
/// the document is dropped; unlinked nodes are still the document's,
/// and are freed with it.
pub struct Document<'g> {
    inner: NonNull<DocInner<'g>>,
    _strings: PhantomData<&'g GlobalStrings>,
}

impl Drop for Document<'_> {
    fn drop(&mut self) {
        let d = self.inner.as_ptr();
        // SAFETY: the block and every node in the slots are ours, freed
        // once here. External heap payloads and retained strings are
        // released before freeing the arena chunks.
        unsafe {
            {
                let inner = &mut *d;
                let strings = inner.strings;
                for slot in inner.heap_nodes.drain(..) {
                    let p = inner.ptrs[slot as usize];
                    if p.is_null() {
                        continue;
                    }
                    match (*p).r#type {
                        EXI_ELEMENT_NODE => {
                            let el = p as *mut RawNode;
                            let ns = core::mem::replace(
                                &mut (*el).nsDef,
                                ptr::null_mut(),
                            );
                            free_ns_list(ns);
                        }
                        EXI_DTD_NODE => {
                            inner.ptrs[slot as usize] = ptr::null_mut();
                            drop_node_payload(p);
                        }
                        _ => drop_node_payload(p),
                    }
                }
                for id in inner.held_uris.held.drain(..) {
                    strings.uris().release(StrId::from_raw(id));
                }
                for id in inner.held_locals.held.drain(..) {
                    strings.locals().release(StrId::from_raw(id));
                }
                for id in inner.held_prefixes.held.drain(..) {
                    strings.prefixes().release(StrId::from_raw(id));
                }
                inner.arena.drop_all(&inner.ptrs);
                free_ns_list(inner.raw.oldNs);
            }
            drop(Box::from_raw(d));
        }
    }
}

/// Frees a list of namespace declarations.
///
/// # Safety
///
/// Each entry is a live `NsBox`, freed once.
unsafe fn free_ns_list(mut ns: *mut RawNs) {
    while !ns.is_null() {
        // SAFETY: the caller's contract.
        unsafe {
            let next = (*ns).next;
            drop(Box::from_raw(ns as *mut NsBox<'_>));
            ns = next;
        }
    }
}

fn arg(detail: &'static str) -> Error {
    Error::new(ErrorKind::Arg, detail)
}

impl<'g> Document<'g> {
    /// An empty document whose names are kept in `strings`. Documents
    /// sharing one `GlobalStrings` give the same name the same ids.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::new(&strings);
    /// assert_eq!(doc.root(), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(strings: &'g GlobalStrings) -> Document<'g> {
        let inner = Box::new(DocInner {
            raw: RawDoc::blank(),
            strings,
            arena: NodeArena::new(),
            ptrs: Vec::new(),
            gens: Vec::new(),
            free: Vec::new(),
            heap_nodes: Vec::new(),
            held_uris: IdSet::new(),
            held_locals: IdSet::new(),
            held_prefixes: IdSet::new(),
            ns_count: 0,
            has_default_ns: false,
            url: None,
            c_scratch: None,
        });
        let p = Box::into_raw(inner);
        // SAFETY: `p` is the new block; slot 0 is the document itself.
        unsafe {
            (*p).raw.doc = p as *mut RawDoc;
            (*p).ptrs.push(p as *mut RawCommon);
            (*p).gens.push(0);
        }
        Document {
            inner: NonNull::new(p).expect("Box is not null"),
            _strings: PhantomData,
        }
    }

    /// Gives the document to C: `exiDocPtr`. Take it back with
    /// [`from_raw`](Self::from_raw) to free it.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let raw = Document::new(&strings).into_raw();
    /// // The pointer came from into_raw and this takes back sole ownership.
    /// let doc = unsafe { Document::from_raw(raw) };
    /// assert_eq!(doc.as_raw(), raw.cast_const());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn into_raw(self) -> *mut RawDoc {
        let p = self.doc_ptr();
        core::mem::forget(self);
        p
    }

    /// Takes back a document given away with [`into_raw`](Self::into_raw).
    ///
    /// # Safety
    ///
    /// `p` came from `into_raw` of a document whose strings outlive
    /// `'g`, and is not used after the returned document is dropped.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let raw = Document::new(&strings).into_raw();
    /// // The pointer came from into_raw and this takes back sole ownership.
    /// let doc = unsafe { Document::from_raw(raw) };
    /// assert_eq!(doc.as_raw(), raw.cast_const());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub unsafe fn from_raw(p: *mut RawDoc) -> Document<'g> {
        Document {
            inner: NonNull::new(p as *mut DocInner<'g>)
                .expect("a document pointer"),
            _strings: PhantomData,
        }
    }

    /// The document behind `p`, borrowed: not freed when the result is
    /// dropped.
    ///
    /// # Safety
    ///
    /// As [`from_raw`](Self::from_raw), and no other `Document` for `p` is in use.
    #[cfg_attr(not(feature = "capi"), allow(dead_code))] // for the C API
    pub(crate) unsafe fn borrow_raw(
        p: *mut RawDoc,
    ) -> core::mem::ManuallyDrop<Document<'g>> {
        // SAFETY: the caller's contract.
        core::mem::ManuallyDrop::new(unsafe { Self::from_raw(p) })
    }

    #[cfg_attr(not(feature = "capi"), allow(dead_code))] // for the C API
    pub(crate) fn scratch(&mut self) -> &mut Option<Box<dyn core::any::Any>> {
        &mut self.dm().c_scratch
    }

    fn d(&self) -> &DocInner<'g> {
        // SAFETY: the block lives as long as `self`.
        unsafe { self.inner.as_ref() }
    }

    fn dm(&mut self) -> &mut DocInner<'g> {
        // SAFETY: as `d`, borrowed mutably through `&mut self`.
        unsafe { self.inner.as_mut() }
    }

    fn doc_ptr(&self) -> *mut RawDoc {
        self.inner.as_ptr() as *mut RawDoc
    }

    /// The C view of this document: `exiDocPtr`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::new(&strings);
    /// assert!(!doc.as_raw().is_null());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn as_raw(&self) -> *const RawDoc {
        self.doc_ptr()
    }

    /// The string tables this document's names are in.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::new(&strings);
    /// assert!(core::ptr::eq(doc.strings(), &strings));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn strings(&self) -> &'g GlobalStrings {
        self.d().strings
    }

    // ----- ids and pointers -----

    fn live_ptr(&self, n: NodeId) -> Option<*mut RawCommon> {
        let d = self.d();
        let idx = n.index as usize;
        let ptr = *d.ptrs.get(idx)?;
        (d.gens[idx] == n.generation && !ptr.is_null()).then_some(ptr)
    }

    fn p(&self, n: NodeId) -> *mut RawCommon {
        self.live_ptr(n).unwrap_or_else(|| {
            panic!("NodeId {n:?} is not a live node of this document")
        })
    }

    /// The id of a node of this document, from its pointer.
    pub(crate) fn id(&self, p: *mut RawCommon) -> NodeId {
        // SAFETY: `p` is a node of this document.
        let index = if p == self.doc_ptr() as *mut RawCommon {
            0
        } else {
            unsafe { node_slot(p) }
        };
        NodeId { index, generation: self.d().gens[index as usize] }
    }

    fn opt(&self, p: *mut RawNode) -> Option<NodeId> {
        (!p.is_null()).then(|| self.id(p as *mut RawCommon))
    }

    /// Whether `n` is a live node of this document.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let text = doc.new_text("hello");
    /// assert!(doc.contains(text));
    /// doc.remove(text);
    /// assert!(!doc.contains(text));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn contains(&self, n: NodeId) -> bool {
        self.live_ptr(n).is_some()
    }

    /// The id for a node pointer from C. `None` if `p` is not a live
    /// node of this document.
    ///
    /// ```
    /// # use exi::{dom::{Document, raw::RawCommon}, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let text = doc.new_text("reading");
    /// let raw = doc.raw(text);
    /// assert_eq!(doc.node_id(raw), Some(text));
    /// doc.remove(text);
    /// assert!(!doc.contains(text));
    /// assert_eq!(doc.node_id(raw), None);
    /// assert_eq!(doc.node_id(core::ptr::dangling::<RawCommon>()), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn node_id(&self, p: *const RawCommon) -> Option<NodeId> {
        let d = self.d();
        d.ptrs
            .iter()
            .enumerate()
            .find(|&(_, &ptr)| !ptr.is_null() && ptr::eq(ptr, p))
            .map(|(index, _)| NodeId {
                index: index as u32,
                generation: d.gens[index],
            })
    }

    /// Looks up a pointer whose backing node is known to be live.
    ///
    /// # Safety
    ///
    /// `p` is a non-null pointer to a live node allocated by this
    /// crate.
    #[cfg(feature = "capi")]
    pub(crate) unsafe fn live_node_id(
        &self,
        p: *mut RawCommon,
    ) -> Option<NodeId> {
        let index = if p == self.doc_ptr().cast() {
            0
        } else {
            unsafe { node_slot(p) }
        };
        let d = self.d();
        let ptr = *d.ptrs.get(index as usize)?;
        (ptr == p).then_some(NodeId { index, generation: d.gens[index as usize] })
    }

    /// The node's C struct: `exiNode*`, `exiAttr*`, `exiDtd*` or
    /// `exiDoc*` by its kind.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let text = doc.new_text("hello");
    /// assert_eq!(doc.node_id(doc.raw(text)), Some(text));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn raw(&self, n: NodeId) -> *const RawCommon {
        self.p(n)
    }

    pub(crate) fn has_default_ns(&self) -> bool {
        self.d().has_default_ns
    }

    pub(crate) fn reserve_nodes(&mut self, additional: usize) {
        let d = self.dm();
        d.ptrs.reserve(additional);
        d.gens.reserve(additional);
    }

    fn reserve_slot(&mut self) -> u32 {
        let d = self.dm();
        match d.free.pop() {
            Some(i) => i,
            None => {
                d.ptrs.push(ptr::null_mut());
                d.gens.push(0);
                (d.ptrs.len() - 1) as u32
            }
        }
    }

    fn alloc_node(&mut self, kind: core::ffi::c_int) -> *mut RawCommon {
        let slot = self.reserve_slot();
        let doc = self.doc_ptr();
        let d = self.dm();
        let p = if kind == EXI_ELEMENT_NODE {
            d.arena.alloc_elem(ElemBox {
                raw: RawNode::blank(kind, doc),
                slot,
                want_prefix: NO_ID,
            })
        } else {
            d.arena.alloc_node(NodeBox {
                raw: RawNode::blank(kind, doc),
                x: Extra::new(slot),
            })
        };
        d.ptrs[slot as usize] = p;
        p
    }

    fn alloc_attr(&mut self) -> *mut RawCommon {
        let slot = self.reserve_slot();
        let doc = self.doc_ptr();
        let d = self.dm();
        let p = d.arena.alloc_attr(AttrBox {
            raw: RawAttr::blank(doc),
            slot,
            want_prefix: NO_ID,
        });
        d.ptrs[slot as usize] = p;
        p
    }

    /// Allocates an attribute and its child text node contiguously in the
    /// arena (used by `DocumentBuilder::attribute_ev`).
    fn alloc_attr_and_text(&mut self) -> (*mut RawCommon, *mut RawCommon) {
        let a_slot = self.reserve_slot();
        let t_slot = self.reserve_slot();
        let doc = self.doc_ptr();
        let d = self.dm();
        let mut text_raw = RawNode::blank(EXI_TEXT_NODE, doc);
        text_raw.name = TEXT_NAME.as_ptr();
        let (ap, tp) = d.arena.alloc_attr_and_node(
            AttrBox {
                raw: RawAttr::blank(doc),
                slot: a_slot,
                want_prefix: NO_ID,
            },
            NodeBox { raw: text_raw, x: Extra::new(t_slot) },
        );
        d.ptrs[a_slot as usize] = ap;
        d.ptrs[t_slot as usize] = tp;
        (ap, tp)
    }

    fn alloc_dtd(
        &mut self,
        name: CBuf,
        ids: [CBuf; 3],
    ) -> *mut RawCommon {
        let slot = self.reserve_slot();
        let b = Box::new(DtdBox {
            raw: RawDtd::blank(self.doc_ptr()),
            slot,
            name,
            ids,
        });
        let p = Box::into_raw(b) as *mut RawCommon;
        let d = self.dm();
        d.ptrs[slot as usize] = p;
        d.heap_nodes.push(slot);
        p
    }

    fn alloc_text_raw(&mut self, text: &str) -> *mut RawCommon {
        let p = self.alloc_node(EXI_TEXT_NODE);
        // SAFETY: `p` is new.
        unsafe { (*p).name = TEXT_NAME.as_ptr() };
        self.set_text(p, text);
        p
    }

    fn alloc_value_raw(&mut self, value: OwnedValue) -> *mut RawCommon {
        let p = self.alloc_node(EXI_TEXT_NODE);
        // SAFETY: `p` is new.
        unsafe { (*p).name = TEXT_NAME.as_ptr() };
        self.set_typed(p, value);
        p
    }

    fn alloc_value_ref_raw(&mut self, value: &Value<'_>) -> *mut RawCommon {
        let p = self.alloc_node(EXI_TEXT_NODE);
        // SAFETY: `p` is new.
        unsafe { (*p).name = TEXT_NAME.as_ptr() };
        self.set_value(p, value);
        p
    }

    #[inline]
    fn hold_uri(&mut self, id: StrId<Uri>) {
        let d = self.dm();
        if d.held_uris.insert(id.index()) {
            d.strings.uris().retain(id);
        }
    }

    #[inline]
    fn hold_local(&mut self, id: StrId<Local>) {
        let d = self.dm();
        if d.held_locals.insert(id.index()) {
            d.strings.locals().retain(id);
        }
    }

    #[inline]
    fn hold_prefix(&mut self, id: StrId<Prefix>) {
        let d = self.dm();
        if d.held_prefixes.insert(id.index()) {
            d.strings.prefixes().retain(id);
        }
    }

    fn intern_uri(&mut self, s: &str) -> Result<StrId<Uri>> {
        let tbl = self.d().strings.uris();
        if let Some(id) = tbl.get(s) {
            self.hold_uri(id);
            return Ok(id);
        }
        let id = tbl.intern_id(s)?;
        if !self.dm().held_uris.insert(id.index()) {
            tbl.release(id);
        }
        Ok(id)
    }

    fn intern_local(&mut self, s: &str) -> Result<StrId<Local>> {
        let tbl = self.d().strings.locals();
        if let Some(id) = tbl.get(s) {
            self.hold_local(id);
            return Ok(id);
        }
        let id = tbl.intern_id(s)?;
        if !self.dm().held_locals.insert(id.index()) {
            tbl.release(id);
        }
        Ok(id)
    }

    fn intern_prefix(&mut self, s: &str) -> Result<StrId<Prefix>> {
        let tbl = self.d().strings.prefixes();
        if let Some(id) = tbl.get(s) {
            self.hold_prefix(id);
            return Ok(id);
        }
        let id = tbl.intern_id(s)?;
        if !self.dm().held_prefixes.insert(id.index()) {
            tbl.release(id);
        }
        Ok(id)
    }

    /// Sets the node's name from `name`, holding its strings on the document.
    fn set_name(&mut self, p: *mut RawCommon, name: Name<'_>) -> Result<()> {
        let uri = self.intern_uri(name.uri())?;
        let local = self.intern_local(name.local_name())?;
        let s = self.d().strings;
        // SAFETY: the caller supplies a live element or attribute.
        unsafe { Self::install_name_ids(p, s, uri, local) };
        Ok(())
    }

    /// Sets the node's name from global ids already verified to belong
    /// to `self.strings()`, retaining each distinct id at most once per
    /// document.
    fn set_name_global(&mut self, p: *mut RawCommon, g: GlobalName) {
        self.hold_uri(g.uri);
        self.hold_local(g.local);
        let s = self.d().strings;
        // SAFETY: `g` belongs to `s` and is held by `self`.
        unsafe { Self::install_name_ids(p, s, g.uri, g.local) };
    }

    /// Installs the name ids of a live element or attribute.
    unsafe fn install_name_ids(
        p: *mut RawCommon,
        strings: &GlobalStrings,
        uri: StrId<Uri>,
        local: StrId<Local>,
    ) {
        // SAFETY: `p` is a live element or attribute. The name pointer
        // points into `strings`, alive while the document holds `local`.
        unsafe {
            match (*p).r#type {
                EXI_ATTRIBUTE_NODE => {
                    let a = p as *mut RawAttr;
                    (*a).uriId = uri.index();
                    (*a).localId = local.index();
                }
                _ => {
                    let n = p as *mut RawNode;
                    (*n).uriId = uri.index();
                    (*n).localId = local.index();
                }
            }
            (*p).name = strings.locals().resolve_cstr(local);
        }
    }

    /// Remembers the prefix wanted for an element's or attribute's
    /// namespace, used when its declaration is chosen.
    fn want_prefix(
        &mut self,
        p: *mut RawCommon,
        prefix: Option<&str>,
    ) -> Result<()> {
        let new_id = match prefix.filter(|p| !p.is_empty()) {
            Some(pre) => self.intern_prefix(pre)?.index(),
            None => NO_ID,
        };
        // SAFETY: a live element or attribute.
        unsafe { swap_want_prefix_id(p, new_id) };
        Ok(())
    }

    /// Sets the wanted prefix from an already-interned `PrefixId` in
    /// `self.strings()` (infallible, used by `detach_ns`).
    fn want_prefix_id_retain(
        &mut self,
        p: *mut RawCommon,
        id: Option<StrId<Prefix>>,
    ) {
        let new_id = match id {
            Some(pid) => {
                self.hold_prefix(pid);
                pid.index()
            }
            None => NO_ID,
        };
        // SAFETY: a live element or attribute.
        unsafe { swap_want_prefix_id(p, new_id) };
    }

    fn want_prefix_str(&self, p: *mut RawCommon) -> Option<&'g str> {
        // SAFETY: a live element or attribute.
        let id = unsafe { want_prefix_id(p) };
        (id != NO_ID)
            .then(|| self.d().strings.prefixes().resolve(StrId::from_raw(id)))
    }

    /// A new attribute on no element (`xmlNewProp` with a NULL node).
    /// Put it on one with [`attach_attribute`](Self::attach_attribute).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let attr = doc.new_attribute("mode", "auto")?;
    /// assert_eq!(doc.parent(attr), None);
    /// assert_eq!(doc.content(attr), "auto");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_attribute(
        &mut self,
        name: impl AsName,
        value: &str,
    ) -> Result<NodeId> {
        let name = name.as_name();
        check_attribute_name(name)?;
        let a = self.alloc_attr();
        self.set_name(a, name)?;
        self.want_prefix(a, name.prefix())?;
        let tp = self.alloc_text_raw(value);
        // SAFETY: a new text node under the new attribute.
        unsafe { append_raw(a, tp) };
        Ok(self.id(a))
    }

    /// Puts an attribute node on an element, moving it from wherever it
    /// was and freeing an attribute of the same name already there
    /// (`xmlAddChild` with an attribute).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let attr = doc.set_attribute(root, "mode", "auto")?;
    /// let replacement = doc.new_attribute("mode", "manual")?;
    /// doc.attach_attribute(root, replacement)?;
    /// assert!(!doc.contains(attr));
    /// assert_eq!(doc.attribute(root, "mode").as_deref(), Some("manual"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn attach_attribute(
        &mut self,
        element: NodeId,
        attr: NodeId,
    ) -> Result<()> {
        let (el, a) = (self.p(element), self.p(attr));
        // SAFETY: live nodes.
        unsafe {
            if (*el).r#type != EXI_ELEMENT_NODE
                || (*a).r#type != EXI_ATTRIBUTE_NODE
            {
                return Err(arg(
                    "attach_attribute takes an element and an attribute",
                ));
            }
            if let Some(name) = self.name(attr) {
                let prefix = self.want_prefix_str(a);
                check_attribute_name(match prefix {
                    Some(prefix) => name.with_prefix(prefix),
                    None => name,
                })?;
            }
            if (*a).parent == el as *mut RawNode {
                return Ok(());
            }
        }
        if let Some(name) = self.name(attr).map(|n| {
            (
                alloc::string::ToString::to_string(n.uri()),
                alloc::string::ToString::to_string(n.local_name()),
            )
        }) {
            if let Some(old) = self
                .attribute_node(element, (name.0.as_str(), name.1.as_str()))
            {
                self.remove(old);
            }
        }
        self.unlink(attr);
        // SAFETY: an unlinked attribute onto a live element.
        unsafe { append_attr_raw(el as *mut RawNode, a as *mut RawAttr) };
        let mode = self.mode_at(el);
        self.fix_attr_ns(a as *mut RawAttr, mode)
    }

    /// Gives an element or attribute a new name, or a PI a new target
    /// (`xmlNodeSetName`, and with a namespace `xmlSetNs`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// doc.rename(root, "settings")?;
    /// assert_eq!(doc.node_name(root), "settings");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn rename(&mut self, n: NodeId, name: impl AsName) -> Result<()> {
        let name = name.as_name();
        let p = self.p(n);
        // SAFETY: a live node.
        match unsafe { (*p).r#type } {
            EXI_ELEMENT_NODE => {
                self.set_name(p, name)?;
                self.want_prefix(p, name.prefix())?;
                let mode = self.mode_at(p);
                self.fix_ns_tree(p as *mut RawNode, mode)
            }
            EXI_ATTRIBUTE_NODE => {
                check_attribute_name(name)?;
                self.set_name(p, name)?;
                self.want_prefix(p, name.prefix())?;
                let mode = self.mode_at(p);
                self.fix_attr_ns(p as *mut RawAttr, mode)
            }
            EXI_PI_NODE => {
                let b = cstr(name.local_name());
                // SAFETY: a live PI; `Payload::Pi` keeps both target and data.
                unsafe {
                    (*p).name = b.as_ptr();
                    match &mut extra_mut(p).payload {
                        Payload::Pi(pair) => pair.0 = b,
                        slot => {
                            let data = cstr("");
                            (*(p as *mut RawNode)).content =
                                data.as_ptr() as *mut u8;
                            *slot = Payload::Pi(Box::new((b, data)));
                        }
                    }
                }
                Ok(())
            }
            _ => Err(arg(
                "only elements, attributes and PIs have names to change",
            )),
        }
    }

    /// The declaration `prefix` is bound to at `node`, as a C pointer;
    /// the `xml` namespace's is made on first use.
    #[cfg_attr(not(feature = "capi"), allow(dead_code))] // for the C API
    pub(crate) fn ns_raw(
        &mut self,
        node: NodeId,
        prefix: Option<&str>,
    ) -> *mut RawNs {
        if prefix == Some("xml") {
            return self.xml_ns().unwrap_or(ptr::null_mut());
        }
        match self.scope_element(node) {
            Some(el) => self.find_ns(el, prefix).unwrap_or(ptr::null_mut()),
            None => ptr::null_mut(),
        }
    }

    /// As [`ns_raw`](Self::ns_raw), by URI.
    #[cfg_attr(not(feature = "capi"), allow(dead_code))] // for the C API
    pub(crate) fn ns_raw_by_href(
        &mut self,
        node: NodeId,
        uri: &str,
    ) -> *mut RawNs {
        if uri == XML_NS {
            return self.xml_ns().unwrap_or(ptr::null_mut());
        }
        match self.scope_element(node) {
            Some(el) => {
                self.find_ns_by_href(el, uri, false).unwrap_or(ptr::null_mut())
            }
            None => ptr::null_mut(),
        }
    }

    /// Declares and returns the declaration, as a C pointer.
    #[cfg_attr(not(feature = "capi"), allow(dead_code))] // for the C API
    pub(crate) fn declare_ns_raw(
        &mut self,
        element: NodeId,
        prefix: Option<&str>,
        uri: &str,
    ) -> Result<*mut RawNs> {
        let el = self.p(element);
        // SAFETY: a live node.
        if unsafe { (*el).r#type } != EXI_ELEMENT_NODE {
            return Err(arg("namespaces are declared on elements"));
        }
        let ns = self.declare_raw(el as *mut RawNode, prefix, uri)?;
        let mode = self.mode_at(el);
        self.fix_ns_tree(el as *mut RawNode, mode)?;
        Ok(ns)
    }

    /// Sets the text of a text, comment or PI node.
    fn set_text(&mut self, p: *mut RawCommon, s: &str) {
        // SAFETY: `p` is a live `NodeBox` of this document with a `content`
        // field. Its payload owns the NUL-terminated bytes (inline or
        // heap-allocated), and the arena address never moves while live.
        unsafe {
            if (*p).r#type == EXI_PI_NODE {
                let data = cstr(s);
                (*(p as *mut RawNode)).content = data.as_ptr() as *mut u8;
                if let Payload::Pi(pair) = &mut extra_mut(p).payload {
                    pair.1 = data;
                }
                return;
            }
            if s.len() <= INLINE_MAX {
                let mut buf = [0u8; INLINE_MAX + 1];
                buf[..s.len()].copy_from_slice(s.as_bytes());
                let x = extra_mut(p);
                x.payload = Payload::Inline { len: s.len() as u8, buf };
                let ptr = match &mut x.payload {
                    Payload::Inline { buf, .. } => buf.as_mut_ptr(),
                    _ => unreachable!(),
                };
                (*(p as *mut RawNode)).content = ptr;
            } else if s.len() <= arena::TEXT_MAX {
                let ptr = self.dm().arena.alloc_text(s.as_bytes());
                (*(p as *mut RawNode)).content = ptr;
                extra_mut(p).payload =
                    Payload::Arena { ptr, len: s.len() as u32 };
            } else {
                let text = cstr(s);
                (*(p as *mut RawNode)).content = text.as_ptr() as *mut u8;
                let x = extra_mut(p);
                let slot = x.slot;
                x.payload = Payload::Text(text);
                self.dm().heap_nodes.push(slot);
            }
        }
    }

    fn set_typed(&mut self, p: *mut RawCommon, v: OwnedValue) {
        match v {
            OwnedValue::Str(s) => self.set_text(p, &s),
            OwnedValue::Bool(b) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Bool(b);
            },
            OwnedValue::BoolDigit(b) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::BoolDigit(b);
            },
            OwnedValue::Int(i) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Int(i);
            },
            OwnedValue::Float(f) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Float(f);
            },
            OwnedValue::Decimal(d) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Decimal {
                    negative: d.negative,
                    fraction_digits: d.fraction_digits,
                    integral: d.integral,
                    fraction: d.fraction,
                };
            },
            v => {
                // SAFETY: as in `set_text`.
                unsafe {
                    (*(p as *mut RawNode)).content = ptr::null_mut();
                    let x = extra_mut(p);
                    let slot = x.slot;
                    x.payload = Payload::Typed(Box::new(v));
                    self.dm().heap_nodes.push(slot);
                }
            }
        }
    }

    fn set_value(&mut self, p: *mut RawCommon, v: &Value<'_>) {
        match *v {
            Value::Str(s) => self.set_text(p, s),
            Value::Bool(b) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Bool(b);
            },
            Value::BoolDigit(b) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::BoolDigit(b);
            },
            Value::Int(i) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Int(i);
            },
            Value::Float(f) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Float(f);
            },
            Value::Decimal(d) => unsafe {
                (*(p as *mut RawNode)).content = ptr::null_mut();
                extra_mut(p).payload = Payload::Decimal {
                    negative: d.negative,
                    fraction_digits: d.fraction_digits,
                    integral: d.integral,
                    fraction: d.fraction,
                };
            },
            _ => self.set_typed(p, v.to_owned()),
        }
    }

    // ----- creating nodes -----

    /// A new element, not yet in the tree (`xmlNewDocNode`). Its
    /// namespace is declared when it is placed, if no declaration in
    /// scope has it.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let element = doc.new_element(("urn:demo", "root"))?;
    /// assert!(doc.name(element).unwrap().is("urn:demo", "root"));
    /// assert_eq!(doc.parent(element), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_element(&mut self, name: impl AsName) -> Result<NodeId> {
        let name = name.as_name();
        let p = self.alloc_node(EXI_ELEMENT_NODE);
        self.set_name(p, name)?;
        self.want_prefix(p, name.prefix())?;
        Ok(self.id(p))
    }

    /// A new text node (`xmlNewDocText`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let text = doc.new_text("hello");
    /// assert_eq!(doc.content(text), "hello");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_text(&mut self, text: &str) -> NodeId {
        let p = self.alloc_text_raw(text);
        self.id(p)
    }

    /// A new text node holding a typed value, as a schema-informed
    /// stream gives it.
    ///
    /// ```
    /// # use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # use exi::{OwnedValue, Value};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let n = doc.new_value(OwnedValue::Int(42));
    /// assert_eq!(doc.value(n), Some(Value::Int(42)));
    /// assert_eq!(doc.content(n), "42");            // text on request
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_value(&mut self, value: OwnedValue) -> NodeId {
        let p = self.alloc_value_raw(value);
        self.id(p)
    }

    /// A new comment (`xmlNewDocComment`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_comment("note");
    /// assert_eq!(doc.kind(node), exi::dom::NodeKind::Comment);
    /// assert_eq!(doc.content(node), "note");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_comment(&mut self, text: &str) -> NodeId {
        let p = self.alloc_node(EXI_COMMENT_NODE);
        // SAFETY: `p` is new.
        unsafe { (*p).name = COMMENT_NAME.as_ptr() };
        self.set_text(p, text);
        self.id(p)
    }

    /// A new processing instruction (`xmlNewDocPI`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_pi("target", "data");
    /// assert_eq!(doc.node_name(node), "target");
    /// assert_eq!(doc.content(node), "data");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_pi(&mut self, target: &str, data: &str) -> NodeId {
        let p = self.alloc_node(EXI_PI_NODE);
        let pair = Box::new((cstr(target), cstr(data)));
        // SAFETY: `p` is new; `Payload::Pi` owns both buffers.
        unsafe {
            (*p).name = pair.0.as_ptr();
            (*(p as *mut RawNode)).content = pair.1.as_ptr() as *mut u8;
            let x = extra_mut(p);
            let slot = x.slot;
            x.payload = Payload::Pi(pair);
            self.dm().heap_nodes.push(slot);
        }
        self.id(p)
    }

    /// A new entity reference `&name;`, kept as the reader reported it.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_entity_ref("example");
    /// assert_eq!(doc.node_name(node), "example");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_entity_ref(&mut self, name: &str) -> NodeId {
        let p = self.alloc_node(EXI_ENTITY_REF_NODE);
        let b = cstr(name);
        // SAFETY: as `new_pi`.
        unsafe {
            (*p).name = b.as_ptr();
            let x = extra_mut(p);
            let slot = x.slot;
            x.payload = Payload::Name(b);
            self.dm().heap_nodes.push(slot);
        }
        self.id(p)
    }

    /// Sets the document type declaration, replacing any before it
    /// (`xmlCreateIntSubset`). It goes before the root element.
    ///
    /// ```
    /// # use exi::{dom::Document, strings::{GlobalConfig, GlobalStrings}, Doctype};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("reading")?;
    /// doc.set_root(root)?;
    /// let dtd = Doctype {
    ///     name: "reading",
    ///     public_id: "",
    ///     system_id: "reading.dtd",
    ///     text: "<!ELEMENT reading EMPTY>",
    /// };
    /// let node = doc.set_doctype(dtd);
    /// assert_eq!(doc.children(doc.document_node()).collect::<Vec<_>>(), [node, root]);
    /// assert_eq!(doc.doctype().unwrap().system_id, "reading.dtd");
    /// assert_eq!(doc.doctype().unwrap().text, "<!ELEMENT reading EMPTY>");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_doctype(&mut self, dt: Doctype<'_>) -> NodeId {
        if let Some(old) = self.doctype_node() {
            self.remove(old);
        }
        let p = self.alloc_dtd(
            cstr(dt.name),
            [cstr(dt.public_id), cstr(dt.system_id), cstr(dt.text)],
        );
        let doc = self.doc_ptr();
        // SAFETY: `p` is a new DTD box; its strings are kept by it.
        unsafe {
            let dtd = &mut *(p as *mut DtdBox);
            dtd.raw.name = dtd.name.as_ptr();
            if !dt.public_id.is_empty() {
                dtd.raw.ExternalID = dtd.ids[0].as_ptr();
            }
            if !dt.system_id.is_empty() {
                dtd.raw.SystemID = dtd.ids[1].as_ptr();
            }
            (*doc).intSubset = p as *mut RawDtd;
            // Before the root element, or last if there is none.
            match self.root() {
                Some(root) => insert_before_raw(self.p(root), p),
                None => append_raw(doc as *mut RawCommon, p),
            }
        }
        self.id(p)
    }

    /// The DOCTYPE, if the document has one.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// assert!(doc.doctype().is_none());
    /// doc.set_doctype(exi::Doctype { name: "root", public_id: "", system_id: "", text: "" });
    /// assert_eq!(doc.doctype().unwrap().name, "root");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn doctype(&self) -> Option<Doctype<'_>> {
        let n = self.doctype_node()?;
        let p = self.p(n);
        // SAFETY: a live DTD node; its strings live as long as `&self`.
        unsafe {
            let b = &(*(p as *const DtdBox)).ids;
            Some(Doctype {
                name: str_at((*p).name),
                public_id: text_of(&b[0]),
                system_id: text_of(&b[1]),
                text: text_of(&b[2]),
            })
        }
    }

    fn doctype_node(&self) -> Option<NodeId> {
        // SAFETY: the document's own field.
        let p = unsafe { (*self.doc_ptr()).intSubset };
        self.opt(p as *mut RawNode)
    }

    /// A new element appended to `parent`, with `text` as its content
    /// (`xmlNewTextChild`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let child = doc.new_child(root, "child", Some("hello"))?;
    /// assert_eq!(doc.parent(child), Some(root));
    /// assert_eq!(doc.content(child), "hello");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new_child(
        &mut self,
        parent: NodeId,
        name: impl AsName,
        text: Option<&str>,
    ) -> Result<NodeId> {
        let e = self.new_element(name)?;
        self.append_child(parent, e)?;
        if let Some(t) = text {
            let t = self.new_text(t);
            self.append_child(e, t)?;
        }
        Ok(e)
    }

    // ----- reading the tree -----

    /// The document node: the parent of the root element.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// assert_eq!(doc.parent(root), Some(doc.document_node()));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn document_node(&self) -> NodeId {
        NodeId { index: 0, generation: 0 }
    }

    /// The root element (`xmlDocGetRootElement`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// assert_eq!(doc.root(), Some(root));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn root(&self) -> Option<NodeId> {
        // SAFETY: walking the document's children.
        let mut c = unsafe { (*self.doc_ptr()).children };
        while !c.is_null() {
            // SAFETY: a live child.
            unsafe {
                if (*c).r#type == EXI_ELEMENT_NODE {
                    return Some(self.id(c as *mut RawCommon));
                }
                c = (*c).next;
            }
        }
        None
    }

    /// Makes `element` the root, unlinking the old root
    /// (`xmlDocSetRootElement`); returns the old root, which is still
    /// the document's.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let next = doc.new_element("next")?;
    /// assert_eq!(doc.set_root(next)?, Some(root));
    /// assert_eq!(doc.root(), Some(next));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_root(&mut self, element: NodeId) -> Result<Option<NodeId>> {
        if self.kind(element) != NodeKind::Element {
            return Err(arg("the root must be an element"));
        }
        match self.root() {
            Some(old) if old == element => Ok(None),
            Some(old) => {
                self.replace(old, element)?;
                Ok(Some(old))
            }
            None => {
                self.append_child(self.document_node(), element)?;
                Ok(None)
            }
        }
    }

    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let text = doc.new_text("hello");
    /// assert_eq!(doc.kind(text), exi::dom::NodeKind::Text);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn kind(&self, n: NodeId) -> NodeKind {
        // SAFETY: a live node.
        NodeKind::of(unsafe { (*self.p(n)).r#type })
    }

    /// The parent: an element, the document node, or (for text in an
    /// attribute) an attribute. `None` for the document node and
    /// unlinked nodes.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.parent(a), Some(root));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn parent(&self, n: NodeId) -> Option<NodeId> {
        // SAFETY: a live node.
        self.opt(unsafe { (*self.p(n)).parent })
    }

    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.first_child(root), Some(a));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn first_child(&self, n: NodeId) -> Option<NodeId> {
        // SAFETY: a live node.
        self.opt(unsafe { (*self.p(n)).children })
    }

    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.last_child(root), Some(b));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn last_child(&self, n: NodeId) -> Option<NodeId> {
        // SAFETY: a live node.
        self.opt(unsafe { (*self.p(n)).last })
    }

    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.next_sibling(a), Some(text));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn next_sibling(&self, n: NodeId) -> Option<NodeId> {
        // SAFETY: a live node.
        self.opt(unsafe { (*self.p(n)).next })
    }

    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.prev_sibling(b), Some(text));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn prev_sibling(&self, n: NodeId) -> Option<NodeId> {
        // SAFETY: a live node.
        self.opt(unsafe { (*self.p(n)).prev })
    }

    /// The children of `n` in order.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.children(root).collect::<Vec<_>>(), [a, text, b]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn children(&self, n: NodeId) -> Children<'_, 'g> {
        Children { doc: self, next: self.first_child(n) }
    }

    fn element_from(
        &self,
        mut c: Option<NodeId>,
        forward: bool,
    ) -> Option<NodeId> {
        while let Some(n) = c {
            if self.kind(n) == NodeKind::Element {
                return Some(n);
            }
            c = if forward {
                self.next_sibling(n)
            } else {
                self.prev_sibling(n)
            };
        }
        None
    }

    /// `xmlFirstElementChild`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.first_element_child(root), Some(a));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn first_element_child(&self, n: NodeId) -> Option<NodeId> {
        self.element_from(self.first_child(n), true)
    }

    /// `xmlLastElementChild`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.last_element_child(root), Some(b));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn last_element_child(&self, n: NodeId) -> Option<NodeId> {
        self.element_from(self.last_child(n), false)
    }

    /// `xmlNextElementSibling`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.next_element_sibling(a), Some(b));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn next_element_sibling(&self, n: NodeId) -> Option<NodeId> {
        self.element_from(self.next_sibling(n), true)
    }

    /// `xmlPreviousElementSibling`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.prev_element_sibling(b), Some(a));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn prev_element_sibling(&self, n: NodeId) -> Option<NodeId> {
        self.element_from(self.prev_sibling(n), false)
    }

    /// `xmlChildElementCount`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert_eq!(doc.child_element_count(root), 2);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn child_element_count(&self, n: NodeId) -> usize {
        self.children(n).filter(|&c| self.kind(c) == NodeKind::Element).count()
    }

    /// The name of an element or attribute, with its prefix and global
    /// ids; `None` for other nodes.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// assert!(doc.name(root).unwrap().is("", "root"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn name(&self, n: NodeId) -> Option<Name<'_>> {
        let p = self.p(n);
        // SAFETY: a live node; its held strings live as long as
        // `&self`.
        unsafe {
            let (u, l, ns) = match (*p).r#type {
                EXI_ELEMENT_NODE => {
                    let n = p as *mut RawNode;
                    ((*n).uriId, (*n).localId, (*n).ns)
                }
                EXI_ATTRIBUTE_NODE => {
                    let a = p as *mut RawAttr;
                    ((*a).uriId, (*a).localId, (*a).ns)
                }
                _ => return None,
            };
            if u == NO_ID || l == NO_ID {
                return None;
            }
            let s = self.d().strings;
            let uri_id = StrId::<Uri>::from_raw(u);
            let local_id = StrId::<Local>::from_raw(l);
            let mut name = Name::new(
                s.uris().resolve(uri_id),
                s.locals().resolve(local_id),
            )
            .with_ids(
                None,
                Some(GlobalName { uri: uri_id, local: local_id }),
            )
            .with_pool(s);
            if !ns.is_null() && !(*ns).prefix.is_null() {
                name = name.with_prefix(str_at((*ns).prefix));
            }
            Some(name)
        }
    }

    /// libxml2's `name` field: the local name of an element or
    /// attribute, the target of a PI, `"text"`, `"comment"`, the
    /// DOCTYPE's name, or the entity's name.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// assert_eq!(doc.node_name(root), "root");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn node_name(&self, n: NodeId) -> &str {
        // SAFETY: a live node; the name lives as long as `&self`.
        unsafe { str_at((*self.p(n)).name) }
    }

    /// The name of an element or attribute as global ids: equal for
    /// equal names in every document and stream sharing these string
    /// tables.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// assert_eq!(doc.global_name(root), doc.lookup_name("", "root"));
    /// assert!(doc.global_name(root).is_some());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn global_name(&self, n: NodeId) -> Option<GlobalName> {
        let p = self.p(n);
        // SAFETY: a live node.
        let (u, l) = unsafe { raw_ids(p) };
        (u != NO_ID && l != NO_ID).then(|| GlobalName {
            uri: StrId::from_raw(u),
            local: StrId::from_raw(l),
        })
    }

    /// The global ids of a name, if some node holds it: compare with
    /// [`global_name`](Self::global_name) instead of comparing strings.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// assert!(doc.lookup_name("", "root").is_some());
    /// assert!(doc.lookup_name("", "unknown").is_none());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn lookup_name(&self, uri: &str, local: &str) -> Option<GlobalName> {
        let s = self.d().strings;
        Some(GlobalName {
            uri: s.uris().get(uri)?,
            local: s.locals().get(local)?,
        })
    }

    /// The value of a text node, typed when it came typed; for an
    /// attribute, its value. `None` for other nodes.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_value(exi::OwnedValue::Int(7));
    /// assert!(matches!(doc.value(node), Some(exi::Value::Int(7))));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn value(&self, n: NodeId) -> Option<Value<'_>> {
        let p = self.p(n);
        // SAFETY: a live node; values live as long as `&self`.
        unsafe {
            match (*p).r#type {
                EXI_TEXT_NODE | EXI_CDATA_SECTION_NODE => {
                    Some(extra(p).as_value())
                }
                EXI_ATTRIBUTE_NODE => {
                    let c = (*p).children;
                    if c.is_null() {
                        Some(Value::Str(""))
                    } else if (*c).next.is_null() {
                        self.value(self.id(c as *mut RawCommon))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
    }

    /// Text content (`xmlNodeGetContent`): for an element, the document
    /// or an attribute, the text of every text node inside it, in
    /// order; for a text node, its value as text; for a comment or PI,
    /// its data.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// doc.new_child(root, "a", Some("one"))?;
    /// doc.new_child(root, "b", Some("two"))?;
    /// assert_eq!(doc.content(root), "onetwo");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn content(&self, n: NodeId) -> String {
        let mut out = String::new();
        self.content_into(self.p(n), &mut out);
        out
    }

    fn content_into(&self, p: *mut RawCommon, out: &mut String) {
        // SAFETY: walking live nodes.
        unsafe {
            match (*p).r#type {
                EXI_TEXT_NODE
                | EXI_CDATA_SECTION_NODE
                | EXI_COMMENT_NODE
                | EXI_PI_NODE => match extra(p).as_value() {
                    Value::Str(s) => out.push_str(s),
                    v => out.push_str(
                        &crate::engine::lexical_string(&v).unwrap_or_default(),
                    ),
                },
                EXI_ELEMENT_NODE | EXI_DOCUMENT_NODE | EXI_ATTRIBUTE_NODE => {
                    let mut c = (*p).children;
                    while !c.is_null() {
                        if !matches!(
                            (*c).r#type,
                            EXI_COMMENT_NODE | EXI_PI_NODE
                        ) {
                            self.content_into(c as *mut RawCommon, out);
                        }
                        c = (*c).next;
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether `n` is a text node of whitespace only
    /// (`xmlIsBlankNode`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// # let a = doc.new_child(root, "a", None)?;
    /// # let text = doc.new_text(" ");
    /// # doc.append_child(root, text)?;
    /// # let b = doc.new_child(root, "b", None)?;
    /// assert!(doc.is_blank(text));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn is_blank(&self, n: NodeId) -> bool {
        self.kind(n) == NodeKind::Text
            && matches!(self.value(n), Some(Value::Str(s)) if s.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r')))
    }

    /// A path to `n` (`xmlGetNodePath`): `/feed/entry[2]/title`,
    /// `/feed/@id`, `/feed/text()[1]`. Names have their prefixes;
    /// elements in a default namespace are written `*[local-name()='x'
    /// ]`… as libxml2 writes `*`.
    ///
    /// ```
    /// # #[cfg(feature = "xml")] {
    /// # use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let doc = Document::parse_xml(&strings, br#"<a><b/><b x="1">t</b></a>"#)?;
    /// let b2 = doc.last_element_child(doc.root().unwrap()).unwrap();
    /// assert_eq!(doc.path(b2), "/a/b[2]");
    /// assert_eq!(doc.path(doc.attribute_node(b2, "x").unwrap()), "/a/b[2]/@x");
    /// assert_eq!(doc.path(doc.first_child(b2).unwrap()), "/a/b[2]/text()");
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn path(&self, n: NodeId) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut cur = Some(n);
        while let Some(c) = cur {
            let step = match self.kind(c) {
                NodeKind::Document => break,
                NodeKind::Attribute => alloc::format!("@{}", self.qname(c)),
                NodeKind::Element => {
                    let q = self.qname(c);
                    let me = self.global_name(c);
                    // `*` (an element in a default namespace) counts
                    // every element in that namespace, as libxml2 does.
                    let uri = me.map(|g| g.uri);
                    let generic = q == "*";
                    let same = |x: NodeId| {
                        self.kind(x) == NodeKind::Element
                            && if generic {
                                self.global_name(x).map(|g| g.uri) == uri
                                    && self.qname(x) == "*"
                            } else {
                                self.global_name(x) == me
                            }
                    };
                    alloc::format!("{q}{}", self.position(c, same))
                }
                NodeKind::Text => alloc::format!(
                    "text(){}",
                    self.position(c, |x| self.kind(x) == NodeKind::Text)
                ),
                NodeKind::Comment => alloc::format!(
                    "comment(){}",
                    self.position(c, |x| self.kind(x) == NodeKind::Comment)
                ),
                NodeKind::ProcessingInstruction => alloc::format!(
                    "processing-instruction('{}')",
                    self.node_name(c)
                ),
                NodeKind::EntityReference | NodeKind::Doctype => {
                    "node()".into()
                }
            };
            parts.push(step);
            cur = self.parent(c);
        }
        parts.reverse();
        alloc::format!("/{}", parts.join("/"))
    }

    fn qname(&self, n: NodeId) -> String {
        match self.name(n) {
            Some(nm) => match nm.prefix() {
                Some(p) if !p.is_empty() => {
                    alloc::format!("{p}:{}", nm.local_name())
                }
                _ if !nm.uri().is_empty()
                    && self.kind(n) == NodeKind::Element =>
                {
                    "*".into()
                }
                _ => nm.local_name().into(),
            },
            None => String::new(),
        }
    }

    /// `[k]` when siblings like `n` (by `same`) are more than one.
    fn position(&self, n: NodeId, same: impl Fn(NodeId) -> bool) -> String {
        let Some(p) = self.parent(n) else { return String::new() };
        let mut k = 0;
        let mut mine = 0;
        for c in self.children(p) {
            if same(c) {
                k += 1;
                if c == n {
                    mine = k;
                }
            }
        }
        if k > 1 { alloc::format!("[{mine}]") } else { String::new() }
    }

    // ----- changing the tree -----

    /// Whether `child` may go under `parent`.
    fn check_place(
        &self,
        parent: *mut RawCommon,
        child: *mut RawCommon,
    ) -> Result<()> {
        // SAFETY: live nodes.
        unsafe {
            let ok = match ((*parent).r#type, (*child).r#type) {
                (_, EXI_DOCUMENT_NODE | EXI_ATTRIBUTE_NODE)
                | (EXI_ELEMENT_NODE, EXI_DTD_NODE) => false,
                (EXI_DOCUMENT_NODE, EXI_ELEMENT_NODE) => {
                    let root = self.root().map(|r| self.p(r));
                    root.is_none() || root == Some(child)
                }
                (
                    EXI_DOCUMENT_NODE,
                    EXI_COMMENT_NODE | EXI_PI_NODE | EXI_DTD_NODE,
                )
                | (EXI_ELEMENT_NODE, _)
                | (
                    EXI_ATTRIBUTE_NODE,
                    EXI_TEXT_NODE | EXI_ENTITY_REF_NODE,
                ) => true,
                _ => false,
            };
            if !ok {
                return Err(arg("this kind of node cannot go there"));
            }
            // No cycles: `child` must not be `parent` or an ancestor of
            // it.
            let mut a = parent;
            while !a.is_null() {
                if a == child {
                    return Err(arg("a node cannot go inside itself"));
                }
                a = (*a).parent as *mut RawCommon;
            }
        }
        Ok(())
    }

    /// Appends `child` as the last child of `parent`, moving it from
    /// wherever it was (`xmlAddChild`, without merging text).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let child = doc.new_element("child")?;
    /// doc.append_child(root, child)?;
    /// assert_eq!(doc.parent(child), Some(root));
    /// assert!(doc.append_child(child, root).is_err());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn append_child(
        &mut self,
        parent: NodeId,
        child: NodeId,
    ) -> Result<()> {
        let (pp, cp) = (self.p(parent), self.p(child));
        self.check_place(pp, cp)?;
        // SAFETY: live nodes of this document, checked above.
        unsafe {
            unlink_raw(cp);
            append_raw(pp, cp);
        }
        self.placed(cp)
    }

    /// Inserts `new` before `sibling` (`xmlAddPrevSibling`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.insert_before(a, b)?;
    /// assert_eq!(doc.children(root).collect::<Vec<_>>(), [b, a]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn insert_before(
        &mut self,
        sibling: NodeId,
        new: NodeId,
    ) -> Result<()> {
        let (sp, np) = (self.p(sibling), self.p(new));
        if sp == np {
            return Ok(());
        }
        // SAFETY: as `append_child`.
        let parent = unsafe { (*sp).parent as *mut RawCommon };
        if parent.is_null() {
            return Err(arg("the sibling is not in the tree"));
        }
        self.check_place(parent, np)?;
        // SAFETY: as `append_child`.
        unsafe {
            unlink_raw(np);
            insert_before_raw(sp, np);
        }
        self.placed(np)
    }

    /// Inserts `new` after `sibling` (`xmlAddNextSibling`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.insert_after(b, a)?;
    /// assert_eq!(doc.children(root).collect::<Vec<_>>(), [b, a]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn insert_after(
        &mut self,
        sibling: NodeId,
        new: NodeId,
    ) -> Result<()> {
        let (sp, np) = (self.p(sibling), self.p(new));
        if sp == np {
            return Ok(());
        }
        // SAFETY: as `append_child`.
        let parent = unsafe { (*sp).parent as *mut RawCommon };
        if parent.is_null() {
            return Err(arg("the sibling is not in the tree"));
        }
        self.check_place(parent, np)?;
        // SAFETY: as `append_child`.
        unsafe {
            unlink_raw(np);
            insert_after_raw(sp, np);
        }
        self.placed(np)
    }

    /// After a move: namespaces of moved elements are found again in
    /// their new scope.
    fn placed(&mut self, p: *mut RawCommon) -> Result<()> {
        // SAFETY: a live node.
        if unsafe { (*p).r#type } == EXI_ELEMENT_NODE {
            let mode = self.mode_at(p);
            self.fix_ns_tree(p as *mut RawNode, mode)?;
        }
        Ok(())
    }

    /// Takes `n` out of the tree (`xmlUnlinkNode`). It stays the
    /// document's: put it back with `append_child` and the like, or
    /// free it with `remove`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.unlink(a);
    /// assert_eq!(doc.parent(a), None);
    /// assert!(doc.contains(a));
    /// assert_eq!(doc.first_child(root), Some(b));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn unlink(&mut self, n: NodeId) {
        let p = self.p(n);
        if n.index == 0 {
            return;
        }
        // SAFETY: a live node of this document.
        unsafe { unlink_raw(p) }
        self.detach_ns(p);
    }

    /// After an unlink: namespace pointers from the subtree to
    /// declarations outside it are dropped (and the prefix remembered),
    /// so they cannot dangle if those declarations are freed. Placing
    /// the subtree finds its declarations again.
    fn detach_ns(&mut self, p: *mut RawCommon) {
        // SAFETY: walking a live, unlinked subtree.
        unsafe {
            if (*p).r#type == EXI_ATTRIBUTE_NODE {
                let a = p as *mut RawAttr;
                let ns = (*a).ns;
                if !ns.is_null() && ns != (*self.doc_ptr()).oldNs {
                    let pre = (*(ns as *const NsBox<'g>))
                        .prefix
                        .as_ref()
                        .map(|h| h.id());
                    self.want_prefix_id_retain(p, pre);
                    (*a).ns = ptr::null_mut();
                }
                return;
            }
            if (*p).r#type != EXI_ELEMENT_NODE {
                return;
            }
            let mut inside: Vec<*mut RawNs> = Vec::new();
            collect_ns(p as *mut RawNode, &mut inside);
            let xml = (*self.doc_ptr()).oldNs;
            let mut todo = alloc::vec![p as *mut RawNode];
            while let Some(e) = todo.pop() {
                let ns = (*e).ns;
                if !ns.is_null() && ns != xml && !inside.contains(&ns) {
                    let pre = (*(ns as *const NsBox<'g>))
                        .prefix
                        .as_ref()
                        .map(|h| h.id());
                    self.want_prefix_id_retain(e as *mut RawCommon, pre);
                    (*e).ns = ptr::null_mut();
                }
                let mut a = (*e).properties;
                while !a.is_null() {
                    let ns = (*a).ns;
                    if !ns.is_null() && ns != xml && !inside.contains(&ns) {
                        let pre = (*(ns as *const NsBox<'g>))
                            .prefix
                            .as_ref()
                            .map(|h| h.id());
                        self.want_prefix_id_retain(a as *mut RawCommon, pre);
                        (*a).ns = ptr::null_mut();
                    }
                    a = (*a).next;
                }
                let mut c = (*e).children;
                while !c.is_null() {
                    if (*c).r#type == EXI_ELEMENT_NODE {
                        todo.push(c);
                    }
                    c = (*c).next;
                }
            }
        }
    }

    /// Moves `n` and everything inside it from another document into
    /// this one, unlinked (libxml2's `xmlDOMWrapAdoptNode`;
    /// `xmlAddChild` does it for nodes of another document). Ids in
    /// `from` for the moved nodes are no longer valid; the nodes' C
    /// pointers stay the same. With different string tables the names
    /// are interned again. An interning error leaves the source tree intact.
    ///
    /// ```
    /// # #[cfg(feature = "xml")] {
    /// # use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut a = Document::parse_xml(&strings, b"<a><moved x='1'>t</moved></a>")?;
    /// let mut b = Document::parse_xml(&strings, b"<b/>")?;
    /// let m = a.first_child(a.root().unwrap()).unwrap();
    /// let ptr = a.raw(m);
    /// let m = b.adopt(&mut a, m)?;
    /// assert_eq!(b.raw(m), ptr);                 // the same node, now b's
    /// b.append_child(b.root().unwrap(), m)?;
    /// assert_eq!(b.to_xml()?, r#"<b><moved x="1">t</moved></b>"#);
    /// assert_eq!(a.to_xml()?, "<a/>");
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn adopt(
        &mut self,
        from: &mut Document<'_>,
        n: NodeId,
    ) -> Result<NodeId> {
        if n.index == 0 {
            return Err(arg("a document node cannot be adopted"));
        }
        let p = from.p(n);
        if ptr::eq(self.doc_ptr(), from.doc_ptr()) {
            return Ok(n);
        }
        let strings = self.d().strings;
        let from_strings = from.d().strings;
        let same = ptr::eq(strings, from_strings);
        let from_doc = from.doc_ptr();
        let mut todo = alloc::vec![p];
        let mut nodes = Vec::new();
        let mut namespaces = Vec::new();
        let mut uses_xml = false;
        let mut has_default_ns = false;
        // SAFETY: all pointers are in the source tree. Stage fallible
        // operations while the source still owns every node.
        unsafe {
            while let Some(node) = todo.pop() {
                let kind = (*node).r#type;
                let ns_field = match kind {
                    EXI_ELEMENT_NODE => {
                        ptr::addr_of_mut!((*node.cast::<RawNode>()).ns)
                    }
                    EXI_ATTRIBUTE_NODE => {
                        ptr::addr_of_mut!((*node.cast::<RawAttr>()).ns)
                    }
                    _ => ptr::null_mut(),
                };
                let implicit_xml = !ns_field.is_null()
                    && !(*ns_field).is_null()
                    && *ns_field == (*from_doc).oldNs;
                uses_xml |= implicit_xml;
                let name = if !same && !ns_field.is_null() {
                    let (u, l) = raw_ids(node);
                    let u_str = if u == NO_ID {
                        ""
                    } else {
                        from_strings.uris().resolve(StrId::from_raw(u))
                    };
                    let l_str = if l == NO_ID {
                        ""
                    } else {
                        from_strings.locals().resolve(StrId::from_raw(l))
                    };
                    let uri = strings.uris().intern(u_str)?;
                    let local = strings.locals().intern(l_str)?;
                    let w = want_prefix_id(node);
                    let want = if w != NO_ID {
                        let pre =
                            from_strings.prefixes().resolve(StrId::from_raw(w));
                        Some(strings.prefixes().intern(pre)?)
                    } else {
                        None
                    };
                    Some((uri, local, want))
                } else {
                    None
                };
                nodes.push((node, name, implicit_xml.then_some(ns_field)));
                let mut child = (*node).children;
                while !child.is_null() {
                    todo.push(child.cast());
                    child = (*child).next;
                }
                if kind == EXI_ELEMENT_NODE {
                    let el = node.cast::<RawNode>();
                    let mut attr = (*el).properties;
                    while !attr.is_null() {
                        todo.push(attr.cast());
                        attr = (*attr).next;
                    }
                    let mut ns = (*el).nsDef;
                    while !ns.is_null() {
                        if (*ns).prefix.is_null()
                            && !str_at((*ns).href).is_empty()
                        {
                            has_default_ns = true;
                        }
                        let names = if same {
                            None
                        } else {
                            let b = &*ns.cast::<NsBox<'_>>();
                            let uri = strings.uris().intern(b.uri.as_str())?;
                            let prefix = b
                                .prefix
                                .as_ref()
                                .map(|h| strings.prefixes().intern(h.as_str()))
                                .transpose()?;
                            Some((uri, prefix))
                        };
                        namespaces.push((ns, names));
                        ns = (*ns).next;
                    }
                }
            }
            let xml = if uses_xml { self.xml_ns()? } else { ptr::null_mut() };
            from.unlink(n);
            if (*p).r#type == EXI_DTD_NODE && (*from_doc).intSubset == p.cast()
            {
                (*from_doc).intSubset = ptr::null_mut();
            }
            if has_default_ns {
                self.dm().has_default_ns = true;
            }
            for (node, name, xml_field) in nodes {
                let kind = (*node).r#type;
                let slot = node_slot(node) as usize;
                let source = from.dm();
                source.ptrs[slot] = ptr::null_mut();
                source.gens[slot] = source.gens[slot].wrapping_add(1);
                source.free.push(slot as u32);
                let mine = self.reserve_slot();
                if kind != EXI_DTD_NODE {
                    NodeArena::transfer_node(
                        &mut from.dm().arena,
                        &mut self.dm().arena,
                        node,
                    );
                }
                self.dm().ptrs[mine as usize] = node;
                set_node_slot(node, mine);
                (*node).doc = self.doc_ptr();
                // Text in the source's text arena is freed with the
                // source: the node takes a copy of its own.
                if !matches!(
                    kind,
                    EXI_DTD_NODE | EXI_ELEMENT_NODE | EXI_ATTRIBUTE_NODE
                ) {
                    let x = extra_mut(node);
                    if let Payload::Arena { .. } = x.payload {
                        let text = cstr(x.text());
                        (*(node as *mut RawNode)).content =
                            text.as_ptr() as *mut u8;
                        x.payload = Payload::Text(text);
                    }
                }
                let needs_heap = match kind {
                    EXI_DTD_NODE => true,
                    EXI_ELEMENT_NODE => {
                        !(*(node as *const RawNode)).nsDef.is_null()
                    }
                    EXI_ATTRIBUTE_NODE => false,
                    _ => extra(node).has_heap_payload(),
                };
                if needs_heap {
                    self.dm().heap_nodes.push(mine);
                }
                if let Some((uri, local, want)) = name {
                    let (uid, lid) = (uri.id(), local.id());
                    if self.dm().held_uris.insert(uid.index()) {
                        core::mem::forget(uri);
                    }
                    if self.dm().held_locals.insert(lid.index()) {
                        core::mem::forget(local);
                    }
                    Self::install_name_ids(node, strings, uid, lid);
                    let new_w = match want {
                        Some(h) => {
                            let id = h.id().index();
                            if self.dm().held_prefixes.insert(id) {
                                core::mem::forget(h);
                            }
                            id
                        }
                        None => NO_ID,
                    };
                    swap_want_prefix_id(node, new_w);
                } else if matches!(
                    kind,
                    EXI_ELEMENT_NODE | EXI_ATTRIBUTE_NODE
                ) {
                    let (u, l) = raw_ids(node);
                    if u != NO_ID {
                        self.hold_uri(StrId::from_raw(u));
                    }
                    if l != NO_ID {
                        self.hold_local(StrId::from_raw(l));
                    }
                    let w = want_prefix_id(node);
                    if w != NO_ID {
                        self.hold_prefix(StrId::from_raw(w));
                    }
                }
                if let Some(field) = xml_field {
                    *field = xml;
                }
            }
            for (ns, names) in namespaces {
                let b = &mut *ns.cast::<NsBox<'g>>();
                b.raw.context = self.doc_ptr();
                if let Some((uri, prefix)) = names {
                    b.raw.href = uri.as_cptr();
                    b.raw.uriId = uri.id().index();
                    b.raw.prefix = prefix
                        .as_ref()
                        .map_or(ptr::null(), |h| h.as_cptr());
                    b.uri = uri;
                    b.prefix = prefix;
                }
            }
        }
        Ok(self.id(p))
    }

    /// Unlinks and frees `n` and everything inside it (`xmlUnlinkNode`
    /// and `xmlFreeNode`). Its id and the ids inside it are no longer
    /// valid.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.remove(a);
    /// assert!(!doc.contains(a));
    /// assert_eq!(doc.first_child(root), Some(b));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn remove(&mut self, n: NodeId) {
        if n.index == 0 {
            return;
        }
        let p = self.p(n);
        // SAFETY: a live node of this document, unlinked first and
        // freed once.
        unsafe {
            unlink_raw(p);
            self.free_raw(p);
        }
    }

    /// Frees a tree whose nodes are this document's.
    ///
    /// # Safety
    ///
    /// `p` is a live, unlinked node of this document.
    pub(crate) unsafe fn free_raw(&mut self, p: *mut RawCommon) {
        // SAFETY: all descendants are live and belong to this document.
        // Unlink each non-root leaf before freeing it, then resume at
        // the live parent. The caller already unlinked the root.
        unsafe {
            let mut node = p;
            loop {
                if !(*node).children.is_null() {
                    node = (*node).children.cast();
                    continue;
                }
                if (*node).r#type == EXI_ELEMENT_NODE {
                    let el = node.cast::<RawNode>();
                    if !(*el).properties.is_null() {
                        node = (*el).properties.cast();
                        continue;
                    }
                    let ns = core::mem::replace(
                        &mut (*el).nsDef,
                        ptr::null_mut(),
                    );
                    free_ns_list(ns);
                }
                let kind = (*node).r#type;
                if kind == EXI_DTD_NODE
                    && (*self.doc_ptr()).intSubset == node.cast()
                {
                    (*self.doc_ptr()).intSubset = ptr::null_mut();
                }
                let done = node == p;
                let parent = (*node).parent.cast::<RawCommon>();
                if !done {
                    unlink_raw(node);
                }
                let slot = node_slot(node) as usize;
                let d = self.dm();
                d.ptrs[slot] = ptr::null_mut();
                d.gens[slot] = d.gens[slot].wrapping_add(1);
                d.free.push(slot as u32);
                drop_node_payload(node);
                if kind != EXI_DTD_NODE {
                    d.arena.free_slot(node);
                }
                if done {
                    break;
                }
                node = parent;
            }
        }
    }

    /// Puts `new` where `old` is; `old` is unlinked and stays the
    /// document's (`xmlReplaceNode`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// let new = doc.new_element("c")?;
    /// doc.replace(a, new)?;
    /// assert_eq!(doc.children(root).collect::<Vec<_>>(), [new, b]);
    /// assert_eq!(doc.parent(a), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn replace(&mut self, old: NodeId, new: NodeId) -> Result<()> {
        if old == new {
            return Ok(());
        }
        let (op, np) = (self.p(old), self.p(new));
        // SAFETY: live nodes of this document. `old` comes out first,
        // so a new root can replace the old one; if `new` cannot go
        // there, `old` goes back.
        unsafe {
            let parent = (*op).parent as *mut RawCommon;
            let mut next = (*op).next as *mut RawCommon;
            if next == np {
                next = (*np).next as *mut RawCommon;
            }
            if parent.is_null() {
                return Err(arg("the node to replace is not in the tree"));
            }
            unlink_raw(op);
            let placed = self.check_place(parent, np);
            let target = if placed.is_ok() { np } else { op };
            if target == np {
                unlink_raw(np);
            }
            if next.is_null() {
                append_raw(parent, target);
            } else {
                insert_before_raw(next, target);
            }
            placed?;
        }
        self.detach_ns(op);
        self.placed(np)
    }

    /// A copy of `n` in this document, unlinked: with everything inside
    /// it when `deep`, else with its attributes and namespace
    /// declarations only (`xmlCopyNode` with `extended` 1 or 2).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// let deep = doc.copy(root, true)?;
    /// let shallow = doc.copy(root, false)?;
    /// assert_eq!(doc.children(deep).count(), 2);
    /// assert_eq!(doc.children(shallow).count(), 0);
    /// assert_eq!(doc.parent(deep), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn copy(&mut self, n: NodeId, deep: bool) -> Result<NodeId> {
        let p = self.p(n);
        let src_strings = self.d().strings;
        // SAFETY: `p` is a live node of this document; copying reads it
        // and allocates new nodes, which moves no existing node.
        let c = unsafe { self.copy_raw(src_strings, p, deep)? };
        Ok(self.id(c))
    }

    /// A copy of `n` from another document, unlinked in this one
    /// (`xmlDocCopyNode`). Names are interned in this document's string
    /// tables.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// let mut other = Document::new(&strings);
    /// let copy = other.import(&doc, root, true)?;
    /// assert_eq!(other.children(copy).count(), 2);
    /// assert_eq!(doc.children(root).count(), 2);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn import(
        &mut self,
        from: &Document<'_>,
        n: NodeId,
        deep: bool,
    ) -> Result<NodeId> {
        let p = from.p(n);
        // SAFETY: `p` is a live node of `from`, which is borrowed for
        // the call.
        let c = unsafe { self.copy_raw(from.d().strings, p, deep)? };
        Ok(self.id(c))
    }

    /// A copy whose namespaces are complete: what the copy needs and
    /// does not declare itself is declared on its root, as libxml2's
    /// `xmlCopyNode` does.
    ///
    /// # Safety
    ///
    /// `src` is a live node of some document using `src_strings` that
    /// stays borrowed for the call.
    unsafe fn copy_raw(
        &mut self,
        src_strings: &GlobalStrings,
        src: *mut RawCommon,
        deep: bool,
    ) -> Result<*mut RawCommon> {
        // SAFETY: the caller's contract.
        let c = unsafe { self.copy_node(src_strings, src, deep)? };
        // SAFETY: the new copy.
        if unsafe { (*c).r#type } == EXI_ELEMENT_NODE {
            self.fix_ns_tree(
                c as *mut RawNode,
                Declare::At(c as *mut RawNode),
            )?;
        }
        Ok(c)
    }

    /// # Safety
    ///
    /// As [`copy_raw`](Self::copy_raw).
    unsafe fn copy_node(
        &mut self,
        src_strings: &GlobalStrings,
        src: *mut RawCommon,
        deep: bool,
    ) -> Result<*mut RawCommon> {
        // SAFETY: reads of the live source; the new nodes are ours.
        unsafe {
            let t = (*src).r#type;
            let same = ptr::eq(src_strings, self.d().strings);
            let new = match t {
                EXI_ELEMENT_NODE => {
                    let s = src as *mut RawNode;
                    let e = self.alloc_node(EXI_ELEMENT_NODE);
                    let (u, l) = ((*s).uriId, (*s).localId);
                    if same && u != NO_ID && l != NO_ID {
                        let (uid, lid) =
                            (StrId::from_raw(u), StrId::from_raw(l));
                        self.hold_uri(uid);
                        self.hold_local(lid);
                        Self::install_name_ids(e, self.d().strings, uid, lid);
                    } else {
                        let uri = if u == NO_ID {
                            ""
                        } else {
                            src_strings.uris().resolve(StrId::from_raw(u))
                        };
                        let local = if l == NO_ID {
                            ""
                        } else {
                            src_strings.locals().resolve(StrId::from_raw(l))
                        };
                        self.set_name(e, Name::new(uri, local))?;
                    }
                    let mut ns = (*s).nsDef;
                    while !ns.is_null() {
                        let pre = (!(*ns).prefix.is_null())
                            .then(|| str_at((*ns).prefix));
                        self.declare_raw(
                            e as *mut RawNode,
                            pre,
                            str_at((*ns).href),
                        )?;
                        ns = (*ns).next;
                    }
                    if !(*s).ns.is_null() && !(*(*s).ns).prefix.is_null() {
                        self.want_prefix(e, Some(str_at((*(*s).ns).prefix)))?;
                    }
                    let mut a = (*s).properties;
                    while !a.is_null() {
                        let ac = self.copy_node(
                            src_strings,
                            a as *mut RawCommon,
                            true,
                        )?;
                        (*ac).parent = e as *mut RawNode;
                        append_attr_raw(e as *mut RawNode, ac as *mut RawAttr);
                        a = (*a).next;
                    }
                    e
                }
                EXI_ATTRIBUTE_NODE => {
                    let sa = src as *mut RawAttr;
                    let a = self.alloc_attr();
                    let (u, l) = ((*sa).uriId, (*sa).localId);
                    if same && u != NO_ID && l != NO_ID {
                        let (uid, lid) =
                            (StrId::from_raw(u), StrId::from_raw(l));
                        self.hold_uri(uid);
                        self.hold_local(lid);
                        Self::install_name_ids(a, self.d().strings, uid, lid);
                    } else {
                        let uri = if u == NO_ID {
                            ""
                        } else {
                            src_strings.uris().resolve(StrId::from_raw(u))
                        };
                        let local = if l == NO_ID {
                            ""
                        } else {
                            src_strings.locals().resolve(StrId::from_raw(l))
                        };
                        self.set_name(a, Name::new(uri, local))?;
                    }
                    if !(*sa).ns.is_null() && !(*(*sa).ns).prefix.is_null() {
                        self.want_prefix(a, Some(str_at((*(*sa).ns).prefix)))?;
                    }
                    a
                }
                EXI_TEXT_NODE | EXI_CDATA_SECTION_NODE => {
                    let n = self.alloc_node(EXI_TEXT_NODE);
                    (*n).name = TEXT_NAME.as_ptr();
                    let val = extra(src).as_value();
                    self.set_value(n, &val);
                    return Ok(n);
                }
                EXI_COMMENT_NODE => {
                    let s = extra(src).text();
                    let n = self.new_comment(s);
                    return Ok(self.p(n));
                }
                EXI_PI_NODE => {
                    let s = extra(src).text();
                    let n = self.new_pi(str_at((*src).name), s);
                    return Ok(self.p(n));
                }
                EXI_ENTITY_REF_NODE => {
                    let n = self.new_entity_ref(str_at((*src).name));
                    return Ok(self.p(n));
                }
                _ => return Err(arg("this kind of node is not copied")),
            };
            if deep || t == EXI_ATTRIBUTE_NODE {
                let mut c = (*src).children;
                while !c.is_null() {
                    let cc =
                        self.copy_node(src_strings, c as *mut RawCommon, true)?;
                    append_raw(new, cc);
                    c = (*c).next;
                }
            }
            Ok(new)
        }
    }

    /// Replaces the content (`xmlNodeSetContent`): an element's or
    /// attribute's children become one text node; a text, comment or PI
    /// node gets this text.
    ///
    /// ```
    /// # use exi::{dom::Document, strings::{GlobalConfig, GlobalStrings}, OwnedValue, Value};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_text("warm");
    /// doc.set_content_value(node, OwnedValue::Int(21))?;
    /// assert_eq!(doc.value(node), Some(Value::Int(21)));
    /// assert_eq!(doc.content(node), "21");
    /// doc.set_content(node, "cool")?;
    /// assert_eq!(doc.value(node), Some(Value::Str("cool")));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_content(&mut self, n: NodeId, text: &str) -> Result<()> {
        self.set_content_value(n, OwnedValue::Str(text.into()))
    }

    /// As [`set_content`](Self::set_content), with a typed value.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_text("old");
    /// doc.set_content_value(node, exi::OwnedValue::Int(2))?;
    /// assert!(matches!(doc.value(node), Some(exi::Value::Int(2))));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_content_value(
        &mut self,
        n: NodeId,
        v: OwnedValue,
    ) -> Result<()> {
        let p = self.p(n);
        // SAFETY: a live node.
        match unsafe { (*p).r#type } {
            EXI_TEXT_NODE | EXI_CDATA_SECTION_NODE => self.set_typed(p, v),
            EXI_COMMENT_NODE | EXI_PI_NODE => match v {
                OwnedValue::Str(s) => self.set_text(p, &s),
                v => {
                    let s = crate::engine::lexical_string(&v.as_value())?;
                    self.set_text(p, &s)
                }
            },
            EXI_ELEMENT_NODE | EXI_ATTRIBUTE_NODE => {
                // SAFETY: freeing this node's children, unlinked first.
                unsafe {
                    while !(*p).children.is_null() {
                        let c = (*p).children as *mut RawCommon;
                        unlink_raw(c);
                        self.free_raw(c);
                    }
                }
                let t = self.new_value(v);
                let tp = self.p(t);
                // SAFETY: a new text node under a live element or
                // attribute.
                unsafe { append_raw(p, tp) };
            }
            _ => return Err(arg("this kind of node has no content")),
        }
        Ok(())
    }

    /// Appends text (`xmlNodeAddContent`): to a text node's text, or to
    /// an element's last child when that is text, else as a new text
    /// node.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let node = doc.new_text("one");
    /// doc.add_content(node, " two")?;
    /// assert_eq!(doc.content(node), "one two");
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn add_content(&mut self, n: NodeId, text: &str) -> Result<()> {
        let p = self.p(n);
        match self.kind(n) {
            NodeKind::Text
            | NodeKind::Comment
            | NodeKind::ProcessingInstruction => {
                let mut s = self.content(n);
                s.push_str(text);
                self.set_content(n, &s)
            }
            NodeKind::Element | NodeKind::Attribute => {
                // SAFETY: a live node.
                let last = unsafe { (*p).last };
                match self.opt(last) {
                    Some(l) if self.kind(l) == NodeKind::Text => {
                        self.add_content(l, text)
                    }
                    _ => {
                        let t = self.new_text(text);
                        let tp = self.p(t);
                        // SAFETY: a new text node under a live element
                        // or attribute.
                        unsafe { append_raw(p, tp) };
                        Ok(())
                    }
                }
            }
            _ => Err(arg("this kind of node has no content")),
        }
    }

    // ----- attributes -----

    fn find_attr(&self, el: *mut RawCommon, name: Name<'_>) -> *mut RawAttr {
        // SAFETY: walking a live element's attributes.
        unsafe {
            if (*el).r#type != EXI_ELEMENT_NODE {
                return ptr::null_mut();
            }
            let s = self.d().strings;
            let mut a = (*(el as *mut RawNode)).properties;
            while !a.is_null() {
                let (u, l) = ((*a).uriId, (*a).localId);
                if u != NO_ID
                    && l != NO_ID
                    && s.locals().resolve(StrId::from_raw(l))
                        == name.local_name()
                    && s.uris().resolve(StrId::from_raw(u)) == name.uri()
                {
                    return a;
                }
                a = (*a).next;
            }
        }
        ptr::null_mut()
    }

    /// Sets an attribute, adding it or replacing its value
    /// (`xmlSetProp` for a name in no namespace, `xmlSetNsProp` for one
    /// in a namespace). Returns the attribute node.
    ///
    /// ```
    /// # #[cfg(feature = "xml")] {
    /// # use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let a = doc.new_element(("http://www.w3.org/2000/svg", "a"))?;
    /// doc.set_root(a)?;
    /// let xlink = exi::Name::new("http://www.w3.org/1999/xlink", "href").with_prefix("xlink");
    /// doc.set_attribute(a, xlink, "#leaf")?;
    /// doc.set_attribute(a, "class", "x")?;
    /// assert_eq!(doc.attribute(a, ("http://www.w3.org/1999/xlink", "href")).as_deref(), Some("#leaf"));
    /// assert_eq!(doc.search_ns(a, Some("xlink")), Some("http://www.w3.org/1999/xlink"));
    /// assert_eq!(doc.to_xml()?, r##"<a xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xlink:href="#leaf" class="x"/>"##);
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_attribute(
        &mut self,
        element: NodeId,
        name: impl AsName,
        value: &str,
    ) -> Result<NodeId> {
        self.set_attribute_value(element, name, OwnedValue::Str(value.into()))
    }

    /// As [`set_attribute`](Self::set_attribute), with a typed value.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// doc.set_attribute_value(root, "count", exi::OwnedValue::Int(2))?;
    /// assert_eq!(doc.attribute(root, "count").as_deref(), Some("2"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_attribute_value(
        &mut self,
        element: NodeId,
        name: impl AsName,
        value: OwnedValue,
    ) -> Result<NodeId> {
        let name = name.as_name();
        let el = self.p(element);
        // SAFETY: a live node.
        if unsafe { (*el).r#type } != EXI_ELEMENT_NODE {
            return Err(arg("attributes go on elements"));
        }
        check_attribute_name(name)?;
        let existing = self.find_attr(el, name);
        if !existing.is_null() {
            let id = self.id(existing as *mut RawCommon);
            self.set_content_value(id, value)?;
            return Ok(id);
        }
        let a = self.alloc_attr();
        self.set_name(a, name)?;
        self.want_prefix(a, name.prefix())?;
        // SAFETY: a new attribute on a live element.
        unsafe {
            (*a).parent = el as *mut RawNode;
            append_attr_raw(el as *mut RawNode, a as *mut RawAttr);
        }
        let mode = self.mode_at(el);
        self.fix_attr_ns(a as *mut RawAttr, mode)?;
        let tp = self.alloc_value_raw(value);
        // SAFETY: a new text node under the new attribute.
        unsafe { append_raw(a, tp) };
        Ok(self.id(a))
    }

    /// An attribute's value as text (`xmlGetNsProp`; a `&str` name is
    /// in no namespace, as `xmlGetNoNsProp`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let attr = doc.set_attribute(root, "mode", "auto")?;
    /// assert_eq!(doc.attribute(root, "mode").as_deref(), Some("auto"));
    /// assert_eq!(doc.attribute(root, "missing"), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn attribute(
        &self,
        element: NodeId,
        name: impl AsName,
    ) -> Option<String> {
        let a = self.attribute_node(element, name)?;
        Some(self.content(a))
    }

    /// An attribute's value, typed when it came typed.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// doc.set_attribute_value(root, "count", exi::OwnedValue::Int(2))?;
    /// assert!(matches!(doc.attribute_value(root, "count"), Some(exi::Value::Int(2))));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn attribute_value(
        &self,
        element: NodeId,
        name: impl AsName,
    ) -> Option<Value<'_>> {
        let a = self.attribute_node(element, name)?;
        self.value(a)
    }

    /// The attribute node (`xmlHasNsProp`).
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let attr = doc.set_attribute(root, "mode", "auto")?;
    /// assert_eq!(doc.attribute_node(root, "mode"), Some(attr));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn attribute_node(
        &self,
        element: NodeId,
        name: impl AsName,
    ) -> Option<NodeId> {
        let a = self.find_attr(self.p(element), name.as_name());
        self.opt(a as *mut RawNode)
    }

    /// Removes an attribute (`xmlUnsetNsProp`); `false` if there was
    /// none.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let attr = doc.set_attribute(root, "mode", "auto")?;
    /// assert!(doc.remove_attribute(root, "mode"));
    /// assert!(!doc.remove_attribute(root, "mode"));
    /// assert!(!doc.contains(attr));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn remove_attribute(
        &mut self,
        element: NodeId,
        name: impl AsName,
    ) -> bool {
        match self.attribute_node(element, name) {
            Some(a) => {
                self.remove(a);
                true
            }
            None => false,
        }
    }

    /// The attribute nodes of an element, in order.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let attr = doc.set_attribute(root, "mode", "auto")?;
    /// let other = doc.set_attribute(root, "active", "yes")?;
    /// assert_eq!(doc.attributes(root).collect::<Vec<_>>(), [attr, other]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn attributes(&self, element: NodeId) -> Children<'_, 'g> {
        let p = self.p(element);
        // SAFETY: a live node.
        let first = unsafe {
            if (*p).r#type == EXI_ELEMENT_NODE {
                (*(p as *mut RawNode)).properties
            } else {
                ptr::null_mut()
            }
        };
        Children { doc: self, next: self.opt(first as *mut RawNode) }
    }

    // ----- namespaces -----

    /// Declares `prefix` (`None` for the default namespace) for `uri`
    /// on an element (`xmlNewNs`). Declaring a prefix the element
    /// already declares for another URI is `ErrorKind::Arg`.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// doc.declare_ns(root, Some("p"), "urn:demo")?;
    /// assert!(doc.declare_ns(root, Some("p"), "urn:other").is_err());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn declare_ns(
        &mut self,
        element: NodeId,
        prefix: Option<&str>,
        uri: &str,
    ) -> Result<()> {
        let el = self.p(element);
        // SAFETY: a live node.
        if unsafe { (*el).r#type } != EXI_ELEMENT_NODE {
            return Err(arg("namespaces are declared on elements"));
        }
        self.declare_raw(el as *mut RawNode, prefix, uri)?;
        let mode = self.mode_at(el);
        self.fix_ns_tree(el as *mut RawNode, mode)
    }

    fn declare_raw(
        &mut self,
        el: *mut RawNode,
        prefix: Option<&str>,
        uri: &str,
    ) -> Result<*mut RawNs> {
        let prefix = prefix.filter(|p| !p.is_empty());
        if prefix == Some("xml")
            || prefix == Some("xmlns")
            || uri == XMLNS_NS
            || (uri == XML_NS && prefix != Some("xml"))
        {
            return Err(arg(
                "the xml and xmlns prefixes and namespaces are reserved",
            ));
        }
        if prefix.is_some() && uri.is_empty() {
            return Err(arg("a prefix cannot be declared for no namespace"));
        }
        // SAFETY: walking a live element's declarations.
        unsafe {
            let mut ns = (*el).nsDef;
            let mut last: *mut RawNs = ptr::null_mut();
            while !ns.is_null() {
                let p =
                    (!(*ns).prefix.is_null()).then(|| str_at((*ns).prefix));
                if p == prefix {
                    return if str_at((*ns).href) == uri {
                        Ok(ns)
                    } else {
                        Err(arg("the element already declares this prefix"))
                    };
                }
                last = ns;
                ns = (*ns).next;
            }
            let new = self.new_ns(prefix, uri)?;
            if prefix.is_none() && !uri.is_empty() {
                self.dm().has_default_ns = true;
            }
            if last.is_null() {
                (*el).nsDef = new;
                let slot = node_slot(el as *mut RawCommon);
                self.dm().heap_nodes.push(slot);
            } else {
                (*last).next = new;
            }
            Ok(new)
        }
    }

    fn new_ns(
        &mut self,
        prefix: Option<&str>,
        uri: &str,
    ) -> Result<*mut RawNs> {
        let s = self.d().strings;
        let u = s.uris().intern(uri)?;
        let p = match prefix {
            Some(p) => Some(s.prefixes().intern(p)?),
            None => None,
        };
        let raw = RawNs {
            next: ptr::null_mut(),
            r#type: EXI_NAMESPACE_DECL,
            href: u.as_cptr(),
            prefix: p.as_ref().map_or(ptr::null(), |h| h.as_cptr()),
            _private: ptr::null_mut(),
            context: self.doc_ptr(),
            uriId: u.id().index(),
            _pad: 0,
        };
        Ok(Box::into_raw(Box::new(NsBox { raw, uri: u, prefix: p }))
            as *mut RawNs)
    }

    /// The `xml` namespace's declaration, made on the document when
    /// first needed.
    fn xml_ns(&mut self) -> Result<*mut RawNs> {
        let d = self.doc_ptr();
        // SAFETY: the document's own field.
        unsafe {
            if (*d).oldNs.is_null() {
                (*d).oldNs = self.new_ns(Some("xml"), XML_NS)?;
            }
            Ok((*d).oldNs)
        }
    }

    /// The declaration in scope at `el` for `prefix`, nearest first.
    fn find_ns(
        &self,
        el: *mut RawNode,
        prefix: Option<&str>,
    ) -> Option<*mut RawNs> {
        let prefix = prefix.filter(|p| !p.is_empty());
        let mut e = el;
        // SAFETY: walking live ancestors and their declarations.
        unsafe {
            while !e.is_null() && (*e).r#type == EXI_ELEMENT_NODE {
                let mut ns = (*e).nsDef;
                while !ns.is_null() {
                    let p = (!(*ns).prefix.is_null())
                        .then(|| str_at((*ns).prefix));
                    if p == prefix {
                        return Some(ns);
                    }
                    ns = (*ns).next;
                }
                e = (*e).parent;
            }
            if prefix == Some("xml") {
                return (!(*self.doc_ptr()).oldNs.is_null())
                    .then(|| (*self.doc_ptr()).oldNs);
            }
        }
        None
    }

    /// A declaration in scope at `el` for `uri` whose prefix is not
    /// redeclared nearer (`xmlSearchNsByHref`). With `prefixed`, the
    /// default namespace does not count.
    fn find_ns_by_href(
        &self,
        el: *mut RawNode,
        uri: &str,
        prefixed: bool,
    ) -> Option<*mut RawNs> {
        let mut e = el;
        // SAFETY: as `find_ns`.
        unsafe {
            while !e.is_null() && (*e).r#type == EXI_ELEMENT_NODE {
                let mut ns = (*e).nsDef;
                while !ns.is_null() {
                    if str_at((*ns).href) == uri
                        && !(prefixed && (*ns).prefix.is_null())
                    {
                        let p = (!(*ns).prefix.is_null())
                            .then(|| str_at((*ns).prefix));
                        if self.find_ns(el, p) == Some(ns) {
                            return Some(ns);
                        }
                    }
                    ns = (*ns).next;
                }
                e = (*e).parent;
            }
        }
        None
    }

    /// The URI `prefix` is bound to at `node` (`xmlSearchNs`); `None`
    /// for the default namespace when there is none. `xml` is always
    /// bound.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.declare_ns(root, Some("p"), "urn:demo")?;
    /// assert_eq!(doc.search_ns(a, Some("p")), Some("urn:demo"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn search_ns(
        &self,
        node: NodeId,
        prefix: Option<&str>,
    ) -> Option<&str> {
        if prefix == Some("xml") {
            return Some(XML_NS);
        }
        let el = self.scope_element(node)?;
        // SAFETY: a live declaration; its string lives as long as
        // `&self`.
        self.find_ns(el, prefix)
            .map(|ns| unsafe { str_at((*ns).href) })
            .filter(|u| !u.is_empty())
    }

    /// A prefix bound to `uri` at `node` (`xmlSearchNsByHref`):
    /// `Some(None)` for the default namespace, `None` when `uri` is not
    /// in scope.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.declare_ns(root, Some("p"), "urn:demo")?;
    /// assert_eq!(doc.search_ns_by_href(a, "urn:demo"), Some(Some("p")));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn search_ns_by_href(
        &self,
        node: NodeId,
        uri: &str,
    ) -> Option<Option<&str>> {
        if uri == XML_NS {
            return Some(Some("xml"));
        }
        let el = self.scope_element(node)?;
        // SAFETY: as `search_ns`.
        self.find_ns_by_href(el, uri, false).map(|ns| unsafe {
            (!(*ns).prefix.is_null()).then(|| str_at((*ns).prefix))
        })
    }

    /// The namespaces declared on an element: `(prefix, uri)`, `None`
    /// for the default.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// let root = doc.new_element("root")?;
    /// doc.set_root(root)?;
    /// let a = doc.new_element("a")?;
    /// let b = doc.new_element("b")?;
    /// doc.append_child(root, a)?;
    /// doc.append_child(root, b)?;
    /// doc.declare_ns(root, Some("p"), "urn:demo")?;
    /// assert_eq!(doc.namespaces(root), [(Some("p"), "urn:demo")]);
    /// assert!(doc.namespaces(a).is_empty());
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn namespaces(&self, element: NodeId) -> Vec<(Option<&str>, &str)> {
        let p = self.p(element);
        let mut out = Vec::new();
        // SAFETY: a live element's declarations; their strings live as
        // long as `&self`.
        unsafe {
            if (*p).r#type != EXI_ELEMENT_NODE {
                return out;
            }
            let mut ns = (*(p as *mut RawNode)).nsDef;
            while !ns.is_null() {
                out.push((
                    (!(*ns).prefix.is_null()).then(|| str_at((*ns).prefix)),
                    str_at((*ns).href),
                ));
                ns = (*ns).next;
            }
        }
        out
    }

    /// The element whose scope applies at `node`: itself, an
    /// attribute's element, or a leaf's parent.
    fn scope_element(&self, node: NodeId) -> Option<*mut RawNode> {
        let mut p = self.p(node);
        // SAFETY: walking live nodes.
        unsafe {
            while !p.is_null() && (*p).r#type != EXI_ELEMENT_NODE {
                p = (*p).parent as *mut RawCommon;
            }
        }
        (!p.is_null()).then_some(p as *mut RawNode)
    }

    /// Points an element's `ns` at a declaration in scope for its URI.
    /// When none is in scope, `mode` says where one is declared, if
    /// anywhere.
    fn fix_ns(&mut self, el: *mut RawNode, mode: Declare) -> Result<()> {
        // SAFETY: a live element.
        unsafe {
            let u = (*el).uriId;
            let uri: &'g str = if u == NO_ID {
                ""
            } else {
                self.d().strings.uris().resolve(StrId::from_raw(u))
            };
            if uri.is_empty() {
                (*el).ns = ptr::null_mut();
                // An element in no namespace under a default namespace
                // needs xmlns="".
                if mode != Declare::No
                    && self.d().has_default_ns
                    && self
                        .find_ns(el, None)
                        .is_some_and(|ns| !str_at((*ns).href).is_empty())
                {
                    self.declare_raw(el, None, "")?;
                }
                return Ok(());
            }
            // The declaration it has, if still in scope and right.
            let cur = (*el).ns;
            if !cur.is_null() && str_at((*cur).href) == uri {
                let p =
                    (!(*cur).prefix.is_null()).then(|| str_at((*cur).prefix));
                if self.find_ns(el, p) == Some(cur) {
                    return Ok(());
                }
            }
            // The wanted prefix if it is bound to this URI here; else
            // any binding of the URI; else a new declaration on the
            // element.
            let wanted = self.want_prefix_str(el as *mut RawCommon);
            let found = wanted
                .and_then(|p| {
                    self.find_ns(el, Some(p))
                        .filter(|&ns| str_at((*ns).href) == uri)
                })
                .or_else(|| self.find_ns_by_href(el, uri, false));
            let ns = match found {
                Some(ns) => ns,
                None if mode == Declare::No => {
                    (*el).ns = ptr::null_mut();
                    return Ok(());
                }
                // A prefixed declaration goes on the host (a copy's
                // root, as libxml2 puts it); a default one on the
                // element itself, since declaring a default on an
                // ancestor would change that ancestor's own
                // meaning.
                None => match wanted {
                    Some(p) => self.declare_prefixed(el, mode, p, uri)?,
                    None if !self.declares_prefix(el, None) => {
                        self.declare_raw(el, None, uri)?
                    }
                    None => {
                        let p = self.free_prefix(el);
                        self.declare_prefixed(el, mode, &p, uri)?
                    }
                },
            };
            (*el).ns = ns;
            // A declaration made for the element's own name goes first:
            // `<x xmlns="…" xmlns:a="…">`, as people write it.
            if (*el).nsDef != ns
                && self.declares_prefix(
                    el,
                    (!(*ns).prefix.is_null()).then(|| str_at((*ns).prefix)),
                )
                && (*ns).next.is_null()
            {
                let mut d = (*el).nsDef;
                while (*d).next != ns {
                    d = (*d).next;
                }
                (*d).next = ptr::null_mut();
                (*ns).next = (*el).nsDef;
                (*el).nsDef = ns;
            }
        }
        Ok(())
    }

    /// Declares `prefix` for `uri` where `mode` says, so that it is in
    /// scope at `el`: on the host, or on `el` when the host's
    /// declaration would not reach it or the host already uses the
    /// prefix.
    fn declare_prefixed(
        &mut self,
        el: *mut RawNode,
        mode: Declare,
        prefix: &str,
        uri: &str,
    ) -> Result<*mut RawNs> {
        let host = match mode {
            Declare::At(h) => h,
            _ => el,
        };
        if host != el && !self.declares_prefix(host, Some(prefix)) {
            let ns = self.declare_raw(host, Some(prefix), uri)?;
            if self.find_ns(el, Some(prefix)) == Some(ns) {
                return Ok(ns);
            }
        }
        // `el` already declares this prefix for another URI: use a new
        // prefix.
        if self.declares_prefix(el, Some(prefix)) {
            let p = self.free_prefix(el);
            return self.declare_raw(el, Some(&p), uri);
        }
        self.declare_raw(el, Some(prefix), uri)
    }

    /// Whether `el` itself declares `prefix`.
    fn declares_prefix(&self, el: *mut RawNode, prefix: Option<&str>) -> bool {
        // SAFETY: walking a live element's declarations.
        unsafe {
            let mut d = (*el).nsDef;
            while !d.is_null() {
                if (!(*d).prefix.is_null()).then(|| str_at((*d).prefix))
                    == prefix
                {
                    return true;
                }
                d = (*d).next;
            }
        }
        false
    }

    fn free_prefix(&mut self, el: *mut RawNode) -> String {
        loop {
            let d = self.dm();
            d.ns_count += 1;
            let p = alloc::format!("ns{}", d.ns_count);
            if self.find_ns(el, Some(&p)).is_none() {
                return p;
            }
        }
    }

    /// As [`fix_ns`](Self::fix_ns) for an attribute: a prefixed declaration, since the
    /// default namespace does not apply to attributes.
    fn fix_attr_ns(&mut self, a: *mut RawAttr, mode: Declare) -> Result<()> {
        // SAFETY: a live attribute on a live element.
        unsafe {
            let u = (*a).uriId;
            let uri: &'g str = if u == NO_ID {
                ""
            } else {
                self.d().strings.uris().resolve(StrId::from_raw(u))
            };
            if uri.is_empty() {
                (*a).ns = ptr::null_mut();
                return Ok(());
            }
            if uri == XML_NS {
                (*a).ns = self.xml_ns()?;
                return Ok(());
            }
            let el = (*a).parent;
            if el.is_null() {
                (*a).ns = ptr::null_mut();
                return Ok(());
            }
            let cur = (*a).ns;
            if !cur.is_null()
                && !(*cur).prefix.is_null()
                && str_at((*cur).href) == uri
                && self.find_ns(el, Some(str_at((*cur).prefix))) == Some(cur)
            {
                return Ok(());
            }
            let wanted = self.want_prefix_str(a as *mut RawCommon);
            let found = wanted
                .and_then(|p| {
                    self.find_ns(el, Some(p))
                        .filter(|&ns| str_at((*ns).href) == uri)
                })
                .or_else(|| self.find_ns_by_href(el, uri, true));
            let ns = match found {
                Some(ns) => ns,
                None if mode == Declare::No => {
                    (*a).ns = ptr::null_mut();
                    return Ok(());
                }
                None => match wanted {
                    Some(p) if self.find_ns(el, Some(p)).is_none() => {
                        self.declare_prefixed(el, mode, p, uri)?
                    }
                    _ => {
                        let p = self.free_prefix(el);
                        self.declare_prefixed(el, mode, &p, uri)?
                    }
                },
            };
            (*a).ns = ns;
        }
        Ok(())
    }

    /// `fix_ns` for an element, its attributes and every element inside
    /// it.
    fn fix_ns_tree(&mut self, el: *mut RawNode, mode: Declare) -> Result<()> {
        // SAFETY: the subtree stays live and namespace repair edits
        // declarations and bindings without changing the tree links.
        unsafe {
            for element in subtree_elements(el) {
                self.fix_ns(element, mode)?;
                let mut attr = (*element).properties;
                while !attr.is_null() {
                    self.fix_attr_ns(attr, mode)?;
                    attr = (*attr).next;
                }
            }
        }
        Ok(())
    }

    /// Whether `p` is in the document's tree (not unlinked, nor inside
    /// something unlinked).
    fn in_tree(&self, p: *mut RawCommon) -> bool {
        let mut c = p;
        // SAFETY: walking live parents.
        unsafe {
            while !(*c).parent.is_null() {
                c = (*c).parent as *mut RawCommon;
            }
        }
        c == self.doc_ptr() as *mut RawCommon
    }

    /// How to declare missing namespaces for an element now at `p`: on
    /// the element when it is in the tree; not at all while it is
    /// detached (its declarations are chosen when it is placed).
    fn mode_at(&self, p: *mut RawCommon) -> Declare {
        if self.in_tree(p) { Declare::Here } else { Declare::No }
    }

    /// The document's URL (`doc->URL`), when it was read from one.
    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// assert_eq!(doc.url(), None);
    /// doc.set_url("urn:document");
    /// assert_eq!(doc.url(), Some("urn:document"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn url(&self) -> Option<&str> {
        self.d().url.as_ref().map(|b| text_of(b))
    }

    ///
    /// ```
    /// use exi::dom::Document;
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let mut doc = Document::new(&strings);
    /// doc.set_url("urn:document");
    /// assert_eq!(doc.url(), Some("urn:document"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn set_url(&mut self, url: &str) {
        let b = cstr(url);
        let d = self.dm();
        d.raw.URL = b.as_ptr();
        d.url = Some(b);
    }
}

/// Children or attributes, in order.
pub struct Children<'d, 'g> {
    doc: &'d Document<'g>,
    next: Option<NodeId>,
}

impl Iterator for Children<'_, '_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let n = self.next?;
        self.next = self.doc.next_sibling(n);
        Some(n)
    }
}

// ----- links -----

/// `(uriId, localId)` on an element or attribute; `(NO_ID, NO_ID)` on
/// other nodes.
///
/// # Safety
///
/// `p` is a live node.
unsafe fn raw_ids(p: *mut RawCommon) -> (u32, u32) {
    // SAFETY: the caller's contract.
    unsafe {
        match (*p).r#type {
            EXI_ELEMENT_NODE => {
                let n = p as *mut RawNode;
                ((*n).uriId, (*n).localId)
            }
            EXI_ATTRIBUTE_NODE => {
                let a = p as *mut RawAttr;
                ((*a).uriId, (*a).localId)
            }
            _ => (NO_ID, NO_ID),
        }
    }
}

/// Every declaration made inside the subtree of `el`.
///
/// # Safety
///
/// `el` is a live element.
unsafe fn collect_ns(el: *mut RawNode, out: &mut Vec<*mut RawNs>) {
    // SAFETY: the caller's contract.
    unsafe {
        for element in subtree_elements(el) {
            let mut ns = (*element).nsDef;
            while !ns.is_null() {
                out.push(ns);
                ns = (*ns).next;
            }
        }
    }
}

/// Elements in preorder. The subtree must stay live with unchanged
/// links while the iterator is in use.
unsafe fn subtree_elements(
    root: *mut RawNode,
) -> impl Iterator<Item = *mut RawNode> {
    let mut next = Some(root);
    core::iter::from_fn(move || {
        let element = next.take()?;
        // SAFETY: the caller keeps the subtree and links alive.
        unsafe {
            let first_element = |mut node: *mut RawNode| {
                while !node.is_null() && (*node).r#type != EXI_ELEMENT_NODE {
                    node = (*node).next;
                }
                (!node.is_null()).then_some(node)
            };
            next = first_element((*element).children);
            let mut at = element;
            while next.is_none() && at != root {
                next = first_element((*at).next);
                at = (*at).parent;
            }
        }
        Some(element)
    })
}

/// Takes `p` out of its parent's list.
///
/// # Safety
///
/// `p` is a live node; its neighbours and parent are live.
pub(crate) unsafe fn unlink_raw(p: *mut RawCommon) {
    // SAFETY: the caller's contract.
    unsafe {
        let parent = (*p).parent as *mut RawCommon;
        let (prev, next) =
            ((*p).prev as *mut RawCommon, (*p).next as *mut RawCommon);
        if !parent.is_null() {
            if (*p).r#type == EXI_ATTRIBUTE_NODE {
                let el = parent as *mut RawNode;
                if (*el).properties == p as *mut RawAttr {
                    (*el).properties = next as *mut RawAttr;
                }
            } else {
                if (*parent).children == p as *mut RawNode {
                    (*parent).children = next as *mut RawNode;
                }
                if (*parent).last == p as *mut RawNode {
                    (*parent).last = prev as *mut RawNode;
                }
            }
        }
        if !prev.is_null() {
            (*prev).next = next as *mut RawNode;
        }
        if !next.is_null() {
            (*next).prev = prev as *mut RawNode;
        }
        (*p).parent = ptr::null_mut();
        (*p).prev = ptr::null_mut();
        (*p).next = ptr::null_mut();
    }
}

/// Appends an unlinked `child` to `parent`'s children.
///
/// # Safety
///
/// Both live, `child` unlinked.
pub(crate) unsafe fn append_raw(
    parent: *mut RawCommon,
    child: *mut RawCommon,
) {
    // SAFETY: the caller's contract.
    unsafe {
        let last = (*parent).last;
        (*child).parent = parent as *mut RawNode;
        (*child).prev = last;
        (*child).next = ptr::null_mut();
        if last.is_null() {
            (*parent).children = child as *mut RawNode;
        } else {
            (*last).next = child as *mut RawNode;
        }
        (*parent).last = child as *mut RawNode;
    }
}

/// # Safety
///
/// Both live, `sib` linked, `new` unlinked.
pub(crate) unsafe fn insert_before_raw(
    sib: *mut RawCommon,
    new: *mut RawCommon,
) {
    // SAFETY: the caller's contract.
    unsafe {
        let parent = (*sib).parent as *mut RawCommon;
        let prev = (*sib).prev;
        (*new).parent = parent as *mut RawNode;
        (*new).prev = prev;
        (*new).next = sib as *mut RawNode;
        (*sib).prev = new as *mut RawNode;
        if prev.is_null() {
            (*parent).children = new as *mut RawNode;
        } else {
            (*prev).next = new as *mut RawNode;
        }
    }
}

/// # Safety
///
/// As [`insert_before_raw`].
pub(crate) unsafe fn insert_after_raw(
    sib: *mut RawCommon,
    new: *mut RawCommon,
) {
    // SAFETY: the caller's contract.
    unsafe {
        let parent = (*sib).parent as *mut RawCommon;
        let next = (*sib).next;
        (*new).parent = parent as *mut RawNode;
        (*new).next = next;
        (*new).prev = sib as *mut RawNode;
        (*sib).next = new as *mut RawNode;
        if next.is_null() {
            (*parent).last = new as *mut RawNode;
        } else {
            (*next).prev = new as *mut RawNode;
        }
    }
}

/// Appends an attribute to an element's list.
///
/// # Safety
///
/// Both live; `a` unlinked, its `parent` already `el`.
pub(crate) unsafe fn append_attr_raw(el: *mut RawNode, a: *mut RawAttr) {
    // SAFETY: the caller's contract.
    unsafe {
        let mut last = (*el).properties;
        if last.is_null() {
            (*el).properties = a;
        } else {
            while !(*last).next.is_null() {
                last = (*last).next;
            }
            (*last).next = a;
            (*a).prev = last;
        }
        (*a).parent = el;
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::strings::GlobalConfig;

    #[test]
    fn deep_namespace_repair_and_removal_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(96 * 1024)
            .spawn(|| {
                let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
                let mut doc = Document::new(&strings);
                let root = doc.new_element("root").unwrap();
                let mut parent = root;
                for _ in 0..4096 {
                    let child = doc.new_element("child").unwrap();
                    // SAFETY: link live nodes directly to construct a
                    // deep fixture without repairing the tree at every
                    // insertion.
                    unsafe { append_raw(doc.p(parent), doc.p(child)) };
                    parent = child;
                }
                let attr = doc.set_attribute(parent, "label", "leaf").unwrap();
                doc.set_root(root).unwrap();
                assert_eq!(
                    doc.attribute(parent, "label").as_deref(),
                    Some("leaf")
                );
                doc.remove(root);
                assert!(doc.root().is_none());
                assert!(!doc.contains(root));
                assert!(!doc.contains(parent));
                assert!(!doc.contains(attr));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
