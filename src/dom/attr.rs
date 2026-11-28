// src/dom/attr.rs
//
// Attributes of a document tree
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Attributes: setting, reading and removing them.

use super::*;

impl<'g> Document<'g> {
    // ----- attributes -----

    pub(super) fn find_attr(&self, el: *mut RawCommon, name: Name<'_>) -> *mut RawAttr {
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
}
