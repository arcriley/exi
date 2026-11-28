// src/dom/parse.rs
//
// XML text straight into a document tree
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! [`Document::parse_xml`](super::Document::parse_xml) reads the common
//! case here: the whole document is in one slice, so names, values and
//! text are read where they lie and the tree is built directly, without
//! the streaming [`XmlReader`](crate::xml::XmlReader), its event queue or
//! session string tables.
//!
//! This reader accepts a subset of what `XmlReader` accepts and builds
//! the same tree. For anything else it returns `None` and the caller
//! parses the document again with `XmlReader`, which then also reports
//! any error. So there is one definition of well-formed XML, and errors
//! read the same whichever way a document is parsed.
//!
//! | Here | Left to `XmlReader` |
//! |---|---|
//! | UTF-8, with or without a byte order mark | UTF-16, any other declared encoding |
//! | `<?xml version="1.0"` with `encoding="UTF-8"` and `standalone` | other versions and declarations |
//! | elements, attributes, text, CDATA, comments, PIs | a DOCTYPE (entities, attribute defaults) |
//! | the five predefined entities and character references | any other entity reference |
//! | ASCII names | names with other characters |
//! | `xmlns` and `xmlns:p` for ordinary URIs, `xml:` attributes | the `xml` and `xmlns` prefixes and namespaces otherwise |
//! | input within `Limits::new()` | anything near a limit |
//! | well-formed input | every error |

use alloc::vec::Vec;
use core::ptr;

use hashbrown::HashMap;
use rustc_hash::FxBuildHasher;

use super::raw::*;
use super::{Document, XmlParseOptions, append_raw};
use crate::Limits;
use crate::xml::scan;
use crate::strings::{GlobalStrings, StrId, Uri};

/// Stop short of the session tables' `max_names`, so a document that
/// might reach it is left to `XmlReader`, which applies it exactly.
const NAME_BUDGET: usize = 30_000;

/// An element name met before: the ids the tree stores for it.
#[derive(Clone, Copy)]
struct Qname {
    /// Bytes of the prefix in the input (`0..0` for none).
    prefix: (u32, u32),
    local: u32,
    local_ptr: *const u8,
    /// The prefix's id, `NO_ID` for none.
    prefix_id: u32,
}

/// A namespace declaration in scope.
struct Scope<'s> {
    prefix: &'s [u8],
    /// The declaration.
    ns: *mut RawNs,
    uri: u32,
    /// `xmlns=""`: no namespace.
    empty: bool,
}

/// An element whose end tag has not been read.
struct Open {
    el: *mut RawNode,
    /// Its name in the input, for the end tag.
    name: (u32, u32),
    /// `scopes.len()` before its declarations.
    scopes: usize,
}

/// One attribute of the start tag being read.
struct Attr<'s> {
    name: &'s [u8],
    /// Where the value starts in the input; `None` when it was decoded
    /// into `values` at `decoded`.
    value: Result<&'s [u8], (usize, usize)>,
}

/// Text read since the last node, before it becomes a text node.
enum Run {
    None,
    /// Text lying in the input unchanged.
    Slice(usize, usize),
    /// Text with references or CDATA, decoded into `text`.
    Buf,
}

struct P<'s, 'g> {
    s: &'s [u8],
    i: usize,
    doc: Document<'g>,
    strings: &'g GlobalStrings,
    ignore_blanks: bool,
    /// Keep comments and PIs; count lines (`XmlParseOptions`).
    comments: bool,
    pis: bool,
    lines: bool,
    line: u32,
    /// Input before this offset is counted in `line`.
    counted: usize,
    stack: Vec<Open>,
    scopes: Vec<Scope<'s>>,
    names: HashMap<&'s [u8], Qname, FxBuildHasher>,
    attrs: Vec<Attr<'s>>,
    /// Decoded attribute values of the current start tag.
    values: Vec<u8>,
    /// `(uri, local)` of the current start tag's attributes.
    expanded: Vec<(u32, u32)>,
    run: Run,
    text: Vec<u8>,
    /// The id of `""`, the URI of names in no namespace.
    no_uri: u32,
    /// The id of the `xml` namespace, once an `xml:` attribute used it.
    xml_uri: Option<u32>,
    prefixes: Vec<(&'s [u8], u32)>,
    root_seen: bool,
    limits: Limits,
}

/// Bytes allowed between the parts of a tag.
#[inline]
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// ASCII name bytes: `START` may begin an NCName, `NAME` may follow.
const START: u8 = 1;
const NAME: u8 = 2;
static CLASS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0;
    while b < 256 {
        let c = b as u8;
        if c.is_ascii_alphabetic() || c == b'_' {
            t[b] = START | NAME;
        } else if c.is_ascii_digit() || c == b'-' || c == b'.' {
            t[b] = NAME;
        }
        b += 1;
    }
    t
};

#[inline]
fn is_name_start(b: u8) -> bool {
    CLASS[b as usize] & START != 0
}

#[inline]
fn is_name_char(b: u8) -> bool {
    CLASS[b as usize] & NAME != 0
}

/// Whether `c` is an XML `Char`.
fn is_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// UTF-8 without characters XML forbids: C0 controls but tab, LF and CR,
/// U+FFFE and U+FFFF. (UTF-8 validity already excludes surrogates.)
fn chars_ok(s: &[u8]) -> bool {
    if core::str::from_utf8(s).is_err() {
        return false;
    }
    let mut i = 0;
    loop {
        i += scan::first_forbidden(&s[i..]);
        if i == s.len() {
            return true;
        }
        if s[i] < 0x20 {
            return false;
        }
        // 0xEF: U+FFFE and U+FFFF are EF BF BE and EF BF BF.
        if s.get(i + 1) == Some(&0xBF) && matches!(s.get(i + 2), Some(&0xBE | &0xBF)) {
            return false;
        }
        i += 1;
    }
}

/// CR LF and lone CR as LF (XML 1.0 §2.11), when the input has a CR.
fn normalize_newlines(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\r' {
            out.push(b'\n');
            i += 1;
            if s.get(i) == Some(&b'\n') {
                i += 1;
            }
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// The document `xml` as `XmlReader` with comments, PIs and the DOCTYPE
/// reported would make it, or `None` to leave it to `XmlReader`.
pub(super) fn parse<'g>(
    strings: &'g GlobalStrings,
    xml: &[u8],
    opts: &XmlParseOptions,
) -> Option<Document<'g>> {
    if xml.starts_with(&[0xFF, 0xFE]) || xml.starts_with(&[0xFE, 0xFF]) {
        return None;
    }
    let xml = xml.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(xml);
    let limits = opts.limits;
    if xml.len() as u64 > u32::MAX as u64 || !chars_ok(xml) {
        return None;
    }
    let owned;
    let s = if xml.contains(&b'\r') {
        owned = normalize_newlines(xml);
        &owned[..]
    } else {
        xml
    };
    let mut doc = Document::new(strings);
    // Each `<` begins a node and is usually followed by text; each `=`
    // is an attribute and its text node. This slightly overestimates
    // (1-10 % on the benchmark documents), so the table never grows.
    doc.reserve_nodes(2 * (scan::count(s, b'<') + scan::count(s, b'=')));
    let no_uri = doc.intern_uri("").ok()?.index();
    let mut p = P {
        s,
        i: 0,
        doc,
        strings,
        ignore_blanks: opts.ignore_blanks,
        comments: opts.comments,
        pis: opts.pis,
        lines: opts.lines,
        line: 1,
        counted: 0,
        stack: Vec::new(),
        scopes: Vec::new(),
        names: HashMap::with_hasher(FxBuildHasher),
        attrs: Vec::new(),
        values: Vec::new(),
        expanded: Vec::new(),
        run: Run::None,
        text: Vec::new(),
        no_uri,
        xml_uri: None,
        prefixes: Vec::new(),
        root_seen: false,
        limits,
    };
    p.document()?;
    p.doc.shrink_slots();
    Some(p.doc)
}

impl<'s, 'g> P<'s, 'g> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    #[inline]
    fn rest(&self) -> &'s [u8] {
        &self.s[self.i..]
    }

    #[inline]
    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.rest().starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) -> bool {
        let start = self.i;
        while self.peek().is_some_and(is_space) {
            self.i += 1;
        }
        self.i > start
    }

    /// The line offset `at` is on (counting from 1). Offsets only grow,
    /// so each byte is counted once.
    fn line_at(&mut self, at: usize) -> u32 {
        if !self.lines {
            return 0;
        }
        let n = scan::count(&self.s[self.counted..at], b'\n');
        self.line += n as u32;
        self.counted = at;
        self.line
    }

    /// An ASCII QName at `i`: its bytes and where its colon is.
    fn qname(&mut self) -> Option<(&'s [u8], Option<usize>)> {
        let start = self.i;
        if !self.peek().is_some_and(is_name_start) {
            return None;
        }
        self.i += 1;
        let mut colon = None;
        loop {
            match self.peek() {
                Some(b) if is_name_char(b) => self.i += 1,
                Some(b':') if colon.is_none() => {
                    colon = Some(self.i - start);
                    self.i += 1;
                    if !self.peek().is_some_and(is_name_start) {
                        return None;
                    }
                    self.i += 1;
                }
                Some(b) if b >= 0x80 || b == b':' => return None,
                _ => break,
            }
        }
        Some((&self.s[start..self.i], colon))
    }

    /// The whole document: prolog, root element, epilog.
    fn document(&mut self) -> Option<()> {
        if self.rest().starts_with(b"<?xml") && self.s.get(5).is_some_and(|&b| is_space(b)) {
            self.declaration()?;
        }
        loop {
            self.skip_space();
            if self.i == self.s.len() {
                return self.root_seen.then_some(());
            }
            if !self.eat(b"<") {
                return None; // text outside the root element
            }
            match self.peek()? {
                b'?' => {
                    self.i += 1;
                    self.pi()?;
                }
                b'!' => {
                    if !self.eat(b"!--") {
                        return None; // DOCTYPE, or CDATA outside the root
                    }
                    self.comment()?;
                }
                _ if self.root_seen => return None,
                _ => {
                    self.root_seen = true;
                    self.start_tag()?;
                    if !self.stack.is_empty() {
                        self.content()?;
                    }
                }
            }
        }
    }

    /// `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>`, the
    /// optional parts optional.
    fn declaration(&mut self) -> Option<()> {
        self.i = 5;
        for (n, name) in [&b"version"[..], b"encoding", b"standalone"].into_iter().enumerate() {
            let save = self.i;
            if !self.skip_space() || !self.eat(name) {
                self.i = save;
                if n == 0 {
                    return None;
                }
                continue;
            }
            self.skip_space();
            if !self.eat(b"=") {
                return None;
            }
            self.skip_space();
            let q = self.peek()?;
            if q != b'"' && q != b'\'' {
                return None;
            }
            self.i += 1;
            let start = self.i;
            let end = start + self.rest().iter().position(|&b| b == q)?;
            let v = &self.s[start..end];
            let ok = match n {
                0 => v == b"1.0",
                1 => v.eq_ignore_ascii_case(b"UTF-8"),
                _ => v == b"yes" || v == b"no",
            };
            if !ok {
                return None;
            }
            self.i = end + 1;
        }
        self.skip_space();
        self.eat(b"?>").then_some(())
    }

    /// After `<?`: a processing instruction.
    fn pi(&mut self) -> Option<()> {
        let start = self.i;
        if !self.peek().is_some_and(|b| is_name_start(b) || b == b':') {
            return None;
        }
        while self.peek().is_some_and(|b| is_name_char(b) || b == b':') {
            self.i += 1;
        }
        let target = &self.s[start..self.i];
        if target.eq_ignore_ascii_case(b"xml") || self.peek().is_some_and(|b| b >= 0x80) {
            return None;
        }
        let data = if self.eat(b"?>") {
            &self.s[0..0]
        } else {
            if !self.skip_space() {
                return None;
            }
            let start = self.i;
            let end = start + find(self.rest(), b"?>")?;
            self.i = end + 2;
            &self.s[start..end]
        };
        if !self.pis {
            return Some(());
        }
        self.flush()?;
        // SAFETY: the input is valid UTF-8 and both ends are ASCII.
        let (target, data) = unsafe {
            (core::str::from_utf8_unchecked(target), core::str::from_utf8_unchecked(data))
        };
        let n = self.doc.new_pi(target, data);
        self.place(n);
        Some(())
    }

    /// After `<!--`: a comment.
    fn comment(&mut self) -> Option<()> {
        let start = self.i;
        let end = start + find(self.rest(), b"--")?;
        if self.s.get(end + 2) != Some(&b'>') {
            return None; // `--` inside, or `--->`
        }
        self.i = end + 3;
        if !self.comments {
            return Some(()); // the text around it runs on
        }
        self.flush()?;
        // SAFETY: valid UTF-8 between ASCII delimiters.
        let text = unsafe { core::str::from_utf8_unchecked(&self.s[start..end]) };
        let n = self.doc.new_comment(text);
        self.place(n);
        Some(())
    }

    /// Puts a new comment or PI under the open element, or the document.
    fn place(&mut self, n: super::NodeId) {
        let parent = match self.stack.last() {
            Some(o) => o.el as *mut RawCommon,
            None => self.doc.doc_ptr() as *mut RawCommon,
        };
        let p = self.doc.p(n);
        // SAFETY: a new node appended to a live element or the document.
        unsafe { append_raw(parent, p) };
    }

    /// The prefix id for `prefix`, interned and held once.
    fn prefix_id(&mut self, prefix: &'s [u8]) -> Option<u32> {
        if let Some(&(_, id)) = self.prefixes.iter().find(|(p, _)| *p == prefix) {
            return Some(id);
        }
        // SAFETY: ASCII.
        let id = self.doc.intern_prefix(unsafe { core::str::from_utf8_unchecked(prefix) }).ok()?.index();
        self.prefixes.push((prefix, id));
        Some(id)
    }

    /// The ids for a QName, interned and held once per document.
    fn name(&mut self, q: &'s [u8], colon: Option<usize>) -> Option<Qname> {
        if let Some(n) = self.names.get(q) {
            return Some(*n);
        }
        if self.names.len() >= NAME_BUDGET {
            return None;
        }
        let (prefix, local) = match colon {
            Some(c) => (&q[..c], &q[c + 1..]),
            None => (&q[..0], q),
        };
        // SAFETY: ASCII.
        let id = self.doc.intern_local(unsafe { core::str::from_utf8_unchecked(local) }).ok()?;
        let prefix_id = if prefix.is_empty() { NO_ID } else { self.prefix_id(prefix)? };
        let at = q.as_ptr() as usize - self.s.as_ptr() as usize;
        let n = Qname {
            prefix: (at as u32, (at + prefix.len()) as u32),
            local: id.index(),
            local_ptr: self.strings.locals().resolve_cstr(id),
            prefix_id,
        };
        self.names.insert(q, n);
        Some(n)
    }

    /// The nearest declaration in scope for `prefix`.
    #[inline]
    fn scope(&self, prefix: &[u8]) -> Option<&Scope<'s>> {
        self.scopes.iter().rev().find(|d| d.prefix == prefix)
    }

    /// After `<`: a start tag, and the element it opens.
    fn start_tag(&mut self) -> Option<()> {
        let tag_at = self.i - 1;
        let (q, colon) = self.qname()?;
        let name = self.name(q, colon)?;
        // Attributes, read whole first: declarations may follow the
        // attributes that use them.
        self.attrs.clear();
        self.values.clear();
        let empty = loop {
            let spaced = self.skip_space();
            match self.peek()? {
                b'>' => {
                    self.i += 1;
                    break false;
                }
                b'/' => {
                    self.i += 1;
                    if !self.eat(b">") {
                        return None;
                    }
                    break true;
                }
                _ if !spaced => return None,
                _ => {
                    let (an, _) = self.qname()?;
                    self.skip_space();
                    if !self.eat(b"=") {
                        return None;
                    }
                    self.skip_space();
                    let value = self.attr_value()?;
                    if self.attrs.len() as u32 >= self.limits.get_max_attributes() {
                        return None;
                    }
                    self.attrs.push(Attr { name: an, value });
                }
            }
        };
        if self.stack.len() as u32 + 1 > self.limits.get_max_depth() {
            return None;
        }
        // The same QName twice.
        let n = self.attrs.len();
        for a in 0..n {
            for b in a + 1..n {
                if self.attrs[a].name == self.attrs[b].name {
                    return None;
                }
            }
        }
        self.flush()?;
        let line = self.line_at(tag_at);
        // The element, as `DocumentBuilder::start_ev` makes it.
        let p = self.doc.alloc_node(EXI_ELEMENT_NODE);
        let el = p as *mut RawNode;
        let parent = match self.stack.last() {
            Some(o) => o.el as *mut RawCommon,
            None => self.doc.doc_ptr() as *mut RawCommon,
        };
        // SAFETY: a new element appended to a live element or the
        // document.
        unsafe {
            (*el).localId = name.local;
            (*el).name = name.local_ptr;
            (*el).line = line.min(u16::MAX as u32) as u16;
            append_raw(parent, p);
            if name.prefix_id != NO_ID {
                swap_want_prefix_id(p, name.prefix_id);
            }
        }
        let scopes = self.scopes.len();
        // Declarations, in order.
        for k in 0..n {
            let an = self.attrs[k].name;
            let prefix = if an == b"xmlns" {
                &an[..0]
            } else if let Some(p) = an.strip_prefix(b"xmlns:") {
                p
            } else {
                continue;
            };
            let uri = value_of(&self.attrs, &self.values, k);
            if prefix == b"xml" || prefix == b"xmlns" || uri == XML_NS.as_bytes() || uri == XMLNS_NS.as_bytes() {
                return None;
            }
            if !prefix.is_empty() && uri.is_empty() {
                return None; // xmlns:p=""
            }
            // SAFETY: the input is UTF-8; the prefix is ASCII.
            let (pre, u) = unsafe {
                (core::str::from_utf8_unchecked(prefix), core::str::from_utf8_unchecked(uri))
            };
            let ns = self.doc.declare_raw(el, (!prefix.is_empty()).then_some(pre), u).ok()?;
            // SAFETY: a new declaration.
            let uri_id = unsafe { (*ns).uriId };
            // Held once here for every element and attribute in it.
            self.doc.hold_uri(StrId::<Uri>::from_raw(uri_id));
            self.scopes.push(Scope { prefix, ns, uri: uri_id, empty: uri.is_empty() });
        }
        // The element's namespace.
        let prefix = &self.s[name.prefix.0 as usize..name.prefix.1 as usize];
        if prefix == b"xml" || prefix == b"xmlns" {
            return None;
        }
        let (uri, ns) = match self.scope(prefix) {
            Some(d) if !d.empty => (d.uri, d.ns),
            Some(_) | None if prefix.is_empty() => (self.no_uri, ptr::null_mut()),
            _ => return None, // an undeclared prefix
        };
        // SAFETY: the new element.
        unsafe {
            (*el).uriId = uri;
            (*el).ns = ns;
        }
        // Attributes.
        let mut last: *mut RawAttr = ptr::null_mut();
        self.expanded.clear();
        for k in 0..n {
            let an = self.attrs[k].name;
            if an == b"xmlns" || an.starts_with(b"xmlns:") {
                continue;
            }
            let colon = an.iter().position(|&b| b == b':');
            let aname = self.name(an, colon)?;
            let prefix = &self.s[aname.prefix.0 as usize..aname.prefix.1 as usize];
            let (uri, ns) = if prefix.is_empty() {
                (self.no_uri, ptr::null_mut())
            } else if prefix == b"xml" {
                let u = match self.xml_uri {
                    Some(u) => u,
                    None => {
                        let u = self.doc.intern_uri(XML_NS).ok()?.index();
                        self.xml_uri = Some(u);
                        u
                    }
                };
                (u, self.doc.xml_ns().ok()?)
            } else {
                match self.scope(prefix) {
                    Some(d) if !d.empty => (d.uri, d.ns),
                    _ => return None,
                }
            };
            if self.expanded.contains(&(uri, aname.local)) {
                return None; // the same URI and local name twice
            }
            self.expanded.push((uri, aname.local));
            let (a, tp) = self.doc.alloc_attr_and_text();
            let ap = a as *mut RawAttr;
            // SAFETY: a new attribute on the new element, and its text.
            unsafe {
                (*ap).uriId = uri;
                (*ap).localId = aname.local;
                (*ap).name = aname.local_ptr;
                (*ap).ns = ns;
                (*ap).parent = el;
                if last.is_null() {
                    (*el).properties = ap;
                } else {
                    (*last).next = ap;
                    (*ap).prev = last;
                }
                if aname.prefix_id != NO_ID {
                    swap_want_prefix_id(a, aname.prefix_id);
                }
            }
            last = ap;
            let v = value_of(&self.attrs, &self.values, k);
            // SAFETY: valid UTF-8 (input, or decoded from it).
            let v = unsafe { core::str::from_utf8_unchecked(v) };
            self.doc.set_text(tp, v);
            // SAFETY: the text node under its attribute.
            unsafe { append_raw(a, tp) };
        }
        if empty {
            self.scopes.truncate(scopes);
        } else {
            self.stack.push(Open {
                el,
                name: ((tag_at + 1) as u32, (tag_at + 1 + q.len()) as u32),
                scopes,
            });
        }
        Some(())
    }


    /// A quoted attribute value: in place when it needs no change.
    fn attr_value(&mut self) -> Option<Result<&'s [u8], (usize, usize)>> {
        let q = self.peek()?;
        if q != b'"' && q != b'\'' {
            return None;
        }
        self.i += 1;
        let start = self.i;
        let max = self.limits.get_max_value_chars() as usize;
        // Plain: no reference, no whitespace to normalize.
        let j = start + scan::find5(&self.s[start..], [q, b'<', b'&', b'\t', b'\n']);
        match self.s.get(j) {
            None | Some(b'<') => return None,
            Some(&b) if b == q => {
                if j - start > max {
                    return None;
                }
                self.i = j + 1;
                return Some(Ok(&self.s[start..j]));
            }
            _ => {}
        }
        // Decoded into `values`.
        let at = self.values.len();
        self.values.extend_from_slice(&self.s[start..j]);
        self.i = j;
        loop {
            let b = self.peek()?;
            self.i += 1;
            match b {
                b if b == q => break,
                b'<' => return None,
                b'\t' | b'\n' => self.values.push(b' '),
                b'&' => {
                    let mut buf = [0u8; 4];
                    let c = self.reference(&mut buf)?;
                    self.values.extend_from_slice(c);
                }
                b => self.values.push(b),
            }
            if self.values.len() - at > max {
                return None;
            }
        }
        Some(Err((at, self.values.len())))
    }

    /// After `&`: a predefined entity or a character reference, as UTF-8.
    fn reference<'b>(&mut self, buf: &'b mut [u8; 4]) -> Option<&'b [u8]> {
        let end = self.i + self.rest().iter().take(12).position(|&b| b == b';')?;
        let r = &self.s[self.i..end];
        self.i = end + 1;
        let c = match r {
            b"lt" => '<',
            b"gt" => '>',
            b"amp" => '&',
            b"apos" => '\'',
            b"quot" => '"',
            _ => {
                let (digits, radix) = match r.strip_prefix(b"#x") {
                    Some(h) => (h, 16),
                    None => (r.strip_prefix(b"#")?, 10),
                };
                if digits.is_empty() || digits.len() > 8 {
                    return None;
                }
                let mut v: u32 = 0;
                for &d in digits {
                    v = v * radix + (d as char).to_digit(radix)?;
                }
                if !is_char(v) {
                    return None;
                }
                char::from_u32(v)?
            }
        };
        Some(c.encode_utf8(buf).as_bytes())
    }

    /// Content of the open elements, until the root's end tag.
    fn content(&mut self) -> Option<()> {
        let max = self.limits.get_max_value_chars() as usize;
        while !self.stack.is_empty() {
            // Text up to the next markup.
            let start = self.i;
            let s = self.s;
            let j = start + scan::find3(&s[start..], b'<', b'&', b']');
            if j > start {
                self.push_text(start, j);
            }
            self.i = j;
            match *s.get(j)? {
                b']' => {
                    if s[j..].starts_with(b"]]>") {
                        return None;
                    }
                    self.push_text(j, j + 1);
                    self.i = j + 1;
                }
                b'&' => {
                    self.i += 1;
                    let mut buf = [0u8; 4];
                    let c = self.reference(&mut buf)?;
                    self.buffer_run();
                    self.text.extend_from_slice(c);
                }
                _ => {
                    self.i += 1;
                    match self.peek()? {
                        b'/' => {
                            self.i += 1;
                            self.end_tag()?;
                        }
                        b'!' => {
                            if self.eat(b"!--") {
                                self.comment()?;
                            } else if self.eat(b"![CDATA[") {
                                let start = self.i;
                                let end = start + find(self.rest(), b"]]>")?;
                                self.i = end + 3;
                                self.buffer_run();
                                self.text.extend_from_slice(&self.s[start..end]);
                            } else {
                                return None;
                            }
                        }
                        b'?' => {
                            self.i += 1;
                            self.pi()?;
                        }
                        _ => self.start_tag()?,
                    }
                }
            }
            if self.run_len() > max {
                return None;
            }
        }
        Some(())
    }

    fn run_len(&self) -> usize {
        match self.run {
            Run::None => 0,
            Run::Slice(a, b) => b - a,
            Run::Buf => self.text.len(),
        }
    }

    /// Adds input bytes `a..b` to the text run.
    #[inline]
    fn push_text(&mut self, a: usize, b: usize) {
        match self.run {
            Run::None => self.run = Run::Slice(a, b),
            Run::Slice(x, y) if y == a => self.run = Run::Slice(x, b),
            _ => {
                self.buffer_run();
                self.text.extend_from_slice(&self.s[a..b]);
            }
        }
    }

    /// Moves the text run into `text`.
    fn buffer_run(&mut self) {
        match self.run {
            Run::Buf => {}
            Run::None => {
                self.text.clear();
                self.run = Run::Buf;
            }
            Run::Slice(a, b) => {
                self.text.clear();
                self.text.extend_from_slice(&self.s[a..b]);
                self.run = Run::Buf;
            }
        }
    }

    /// Makes the text run a text node of the open element.
    fn flush(&mut self) -> Option<()> {
        let run = core::mem::replace(&mut self.run, Run::None);
        let bytes: *const [u8] = match run {
            Run::None => return Some(()),
            Run::Slice(a, b) => &self.s[a..b],
            Run::Buf => &self.text[..],
        };
        let Some(open) = self.stack.last() else {
            return Some(()); // around the root: not kept
        };
        let el = open.el as *mut RawCommon;
        // SAFETY: `bytes` is `self.s` or `self.text`, neither of which
        // changes until the next run; both hold valid UTF-8 (input, or
        // decoded from it).
        let t: &str = unsafe { core::str::from_utf8_unchecked(&*bytes) };
        if self.ignore_blanks && t.bytes().all(is_space) {
            return Some(());
        }
        let tp = self.doc.alloc_node(EXI_TEXT_NODE);
        // SAFETY: a new text node under the open element.
        unsafe {
            (*tp).name = TEXT_NAME.as_ptr();
            self.doc.set_text(tp, t);
            append_raw(el, tp);
        }
        Some(())
    }

    /// After `</`: the end tag of the open element.
    fn end_tag(&mut self) -> Option<()> {
        let open = self.stack.last()?;
        let name = &self.s[open.name.0 as usize..open.name.1 as usize];
        if !self.rest().starts_with(name) {
            return None;
        }
        self.i += name.len();
        if self.peek().is_some_and(|b| is_name_char(b) || b == b':' || b >= 0x80) {
            return None; // a longer name
        }
        self.skip_space();
        if !self.eat(b">") {
            return None;
        }
        self.flush()?;
        let open = self.stack.pop()?;
        self.scopes.truncate(open.scopes);
        Some(())
    }
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// The value of the current start tag's attribute `k`.
fn value_of<'a>(attrs: &'a [Attr<'_>], values: &'a [u8], k: usize) -> &'a [u8] {
    match attrs[k].value {
        Ok(v) => v,
        Err((a, b)) => &values[a..b],
    }
}

/// Where `needle` first occurs in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        {
            let k = hay[i..].iter().position(|&b| b == first)?;
            i += k;
            if hay[i..].starts_with(needle) {
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

