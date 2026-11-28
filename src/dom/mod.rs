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
mod attr;
mod io;
mod ns;
#[cfg(feature = "xml")]
mod parse;
pub mod raw;
mod tree;

pub use io::{DocEvents, DocumentBuilder};
#[cfg(feature = "xml")]
pub use io::XmlParseOptions;
#[cfg(feature = "std")]
pub use arena::release_cache;

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

    #[cfg(feature = "xml")]
    pub(crate) fn reserve_nodes(&mut self, additional: usize) {
        let d = self.dm();
        d.ptrs.reserve(additional);
        d.gens.reserve(additional);
    }

    /// Gives back slot capacity a parse reserved but did not use, when
    /// that is more than an eighth.
    #[cfg(feature = "xml")]
    pub(crate) fn shrink_slots(&mut self) {
        let d = self.dm();
        if d.ptrs.capacity() - d.ptrs.len() > d.ptrs.len() / 8 {
            d.ptrs.shrink_to_fit();
            d.gens.shrink_to_fit();
        }
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
