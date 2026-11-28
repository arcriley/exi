// src/dom/tree.rs
//
// Reading and changing a document tree
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Reading the tree and changing it: navigation, content, placing, unlinking, adopting, copying and removing nodes.

use super::*;

impl<'g> Document<'g> {
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

    pub(super) fn element_from(
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

    pub(super) fn content_into(&self, p: *mut RawCommon, out: &mut String) {
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

    pub(super) fn qname(&self, n: NodeId) -> String {
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
    pub(super) fn position(&self, n: NodeId, same: impl Fn(NodeId) -> bool) -> String {
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
    pub(super) fn check_place(
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
    pub(super) fn placed(&mut self, p: *mut RawCommon) -> Result<()> {
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
    pub(super) fn detach_ns(&mut self, p: *mut RawCommon) {
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
    pub(super) unsafe fn copy_raw(
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
    pub(super) unsafe fn copy_node(
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
}
