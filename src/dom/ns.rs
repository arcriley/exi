// src/dom/ns.rs
//
// Namespaces of a document tree
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Namespace declarations: making them, finding them in scope, and choosing one for each element and attribute.

use super::*;

impl<'g> Document<'g> {
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

    pub(super) fn declare_raw(
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

    pub(super) fn new_ns(
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
    pub(super) fn xml_ns(&mut self) -> Result<*mut RawNs> {
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
    pub(super) fn find_ns(
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
    pub(super) fn find_ns_by_href(
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
    pub(super) fn scope_element(&self, node: NodeId) -> Option<*mut RawNode> {
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
    pub(super) fn fix_ns(&mut self, el: *mut RawNode, mode: Declare) -> Result<()> {
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
    pub(super) fn declare_prefixed(
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
    pub(super) fn declares_prefix(&self, el: *mut RawNode, prefix: Option<&str>) -> bool {
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

    pub(super) fn free_prefix(&mut self, el: *mut RawNode) -> String {
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
    pub(super) fn fix_attr_ns(&mut self, a: *mut RawAttr, mode: Declare) -> Result<()> {
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
    pub(super) fn fix_ns_tree(&mut self, el: *mut RawNode, mode: Declare) -> Result<()> {
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
    pub(super) fn in_tree(&self, p: *mut RawCommon) -> bool {
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
    pub(super) fn mode_at(&self, p: *mut RawCommon) -> Declare {
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
