// src/xml/reader.rs
//
// XML 1.0 text reader
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # What it accepts
//!
//! | Area | Rule |
//! |---|---|
//! | Version | XML 1.0. `version="1.1"` is `ErrorKind::Unsupported`; any other version is `ErrorKind::Xml` |
//! | Encoding | UTF-8, with or without the byte order mark `EF BB BF`; UTF-16 with a byte order mark (`FF FE` little-endian, `FE FF` big-endian). Input without a byte order mark is UTF-8. A declared encoding must match (`UTF-8` or `UTF-16`, any letter case), otherwise `ErrorKind::Xml`; any other declared encoding is `ErrorKind::Unsupported` |
//! | Line ends | CR LF and a lone CR become LF before anything else is parsed |
//! | Characters | only XML `Char`: `#x9`, `#xA`, `#xD`, `#x20–#xD7FF`, `#xE000–#xFFFD`, `#x10000–#x10FFFF`; anything else, including in UTF-8 that is malformed, is `ErrorKind::Xml` |
//! | Names | element and attribute names are QNames: an NCName, or `prefix:local` with both NCNames. Entity, PI target and DOCTYPE names are Names |
//! | Namespaces | `xmlns="uri"` sets the default namespace for elements (`xmlns=""` removes it); `xmlns:p="uri"` binds `p`. Errors (`ErrorKind::Xml`): an undeclared prefix; `xmlns:p=""`; declaring the `xmlns` prefix; binding `xml` to any URI but `http://www.w3.org/XML/1998/namespace`, or that URI to any other prefix or as the default; binding `http://www.w3.org/2000/xmlns/` at all. Unprefixed attributes are in no namespace. The `xml` prefix is always bound |
//! | Attributes | `<` in a value is an error. Values are normalized: each literal tab, LF or CR becomes a space; character references keep the character they name; entity references are expanded (internal entities only). Attributes the internal subset declares with a type other than CDATA then also lose leading and trailing spaces, and runs of spaces become one. Two attributes with the same QName, or the same URI and local name, are an error |
//! | Character data | the text `]]>` outside a CDATA section is an error. CDATA sections are character data: EXI has no CDATA event |
//! | References | character references `&#n;` and `&#xh;` must name a `Char`; the five predefined entities `lt`, `gt`, `amp`, `apos`, `quot` |
//! | Comments | `--` inside a comment is an error, so is a comment ending `--->` |
//! | PIs | a target `xml` in any letter case is an error, except the XML declaration at the very start |
//! | Structure | one root element; comments, PIs and whitespace before and after it; a DOCTYPE only before it. Text other than whitespace outside the root is an error |
//!
//! # The DOCTYPE and entities
//!
//! The reader is a non-validating processor: it reads the internal
//! subset for entity declarations and attribute defaults and fetches
//! nothing.
//!
//! - **Internal general entities** are expanded under
//!   `EntityPolicy::Internal` (the default). The replacement text of
//!   every entity expanded, at every level of nesting, counts in bytes
//!   toward `Limits::max_expanded_bytes` for the whole document, and
//!   nesting counts toward `max_entity_depth`. Counting every level
//!   (not only the text that reaches the document) also bounds chains
//!   of entities that expand to nothing. This stops exponential
//!   ("billion laughs") and quadratic blowup. An entity that refers to
//!   itself, directly or not, is an error. The replacement text must be
//!   balanced: an element started in it ends in it.
//! - **Attribute defaults** declared in the internal subset are added
//!   to elements that lack the attribute, after the attributes given.
//! - **Parameter entities** are expanded only between declarations. A
//!   reference to a parameter entity the reader cannot read (an
//!   external one) means later declarations may depend on what it
//!   holds: after it, entity and attribute-list declarations are
//!   ignored, unless the document says `standalone="yes"`.
//! - **External entities and the external subset are never fetched.** A
//!   reference to an external parsed entity becomes an
//!   `EntityReference` event. A reference to an unparsed (NDATA) entity
//!   in content is an error.
//! - **Undeclared entities** are an error, except when the DOCTYPE has
//!   an external subset or an unread parameter entity reference:
//!   declarations may then be missing, and the reference becomes an
//!   `EntityReference` event.
//! - Under `EntityPolicy::PredefinedOnly`, a DOCTYPE that declares any
//!   entity is an error.
//! - With `report_doctype(true)` the DOCTYPE arrives as a `Doctype`
//!   event with its name, public and system ids and internal subset
//!   text.
//!
//! # Events
//!
//! `StartDocument`; then, in document order, `Doctype` (when reported),
//! `Comment` and `ProcessingInstruction` (when reported),
//! `StartElement`, one `Namespace` per declaration in the order
//! written, the element's `Attribute`s in the order written (then
//! defaults), `Characters`, `EntityReference`, `EndElement`; finally
//! `EndDocument`.
//!
//! - Adjacent character data, CDATA sections, character references and
//!   expanded entity text form one `Characters` event, ended by markup
//!   that is not a reference. A run longer than
//!   `Limits::max_value_chars` is `ErrorKind::Limit`.
//! - Whitespace inside the root element is reported like any text;
//!   whitespace outside it is not reported.
//! - The XML declaration is not an event.
//!
//! # Errors
//!
//! Well-formedness errors are `ErrorKind::Xml`, with `line` and
//! `column` (1-based, columns in characters) of the token that failed.
//! After any error the reader is poisoned.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::error::Position;
use crate::event::{Doctype, Event, EventMeta, NsDecl};
use crate::grammar::Grammars;
use crate::input::InputRing;
use crate::name::{GlobalName, Name};
use crate::stream::{EventSource, Step};
use crate::tables::{SessionTables, ValueRules};
use crate::value::{Value, ValueFlags};
use crate::xml::chars::{is_char, is_name, is_name_char, is_ncname, is_space};
use crate::{Error, ErrorKind, Limits, NameId, Result};

/// How entity references are treated (module documentation).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum EntityPolicy {
    /// Only the five predefined entities and character references.
    PredefinedOnly,
    /// Also internal general entities declared in the internal subset.
    #[default]
    Internal,
}

/// Configuration for an [`XmlReader`].
///
/// ```
/// use exi::xml::{XmlConfig, EntityPolicy};
/// let c = XmlConfig::new();
/// assert_eq!((c.get_entities(), c.get_report_doctype(), c.get_report_comments(), c.get_report_pis()),
///            (EntityPolicy::Internal, false, true, true));
/// ```
#[derive(Clone, Debug)]
pub struct XmlConfig {
    limits: Limits,
    entities: EntityPolicy,
    report_doctype: bool,
    report_comments: bool,
    report_pis: bool,
}

impl XmlConfig {
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// let config = XmlConfig::new();
    /// assert!(config.get_report_comments());
    /// assert!(config.get_report_pis());
    /// assert!(!config.get_report_doctype());
    /// ```
    pub fn new() -> Self {
        XmlConfig {
            limits: Limits::new(),
            entities: EntityPolicy::Internal,
            report_doctype: false,
            report_comments: true,
            report_pis: true,
        }
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new().limits(exi::Limits::new().max_depth(1)), &grammars);
    /// r.feed(b"<a><b/></a>");
    /// r.end_input();
    /// loop {
    ///     match r.next() {
    ///         Ok(Step::Event(..)) => {}
    ///         Err(e) => { assert_eq!(e.kind(), exi::ErrorKind::Limit); break; }
    ///         other => panic!("expected a depth error, got {other:?}"),
    ///     }
    /// }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn limits(self, l: Limits) -> Self {
        XmlConfig { limits: l, ..self }
    }
    ///
    /// ```
    /// use exi::xml::{EntityPolicy, XmlConfig};
    /// let config = XmlConfig::new().entities(EntityPolicy::PredefinedOnly);
    /// assert_eq!(config.get_entities(), EntityPolicy::PredefinedOnly);
    /// ```
    pub fn entities(self, p: EntityPolicy) -> Self {
        XmlConfig { entities: p, ..self }
    }
    /// Report the DOCTYPE as a `Doctype` event (default off; needed for
    /// `Preserve.dtd`).
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// let config = XmlConfig::new();
    /// assert!(!config.get_report_doctype());
    /// let config = config.report_doctype(true);
    /// assert!(config.get_report_doctype());
    /// assert!(!config.report_doctype(false).get_report_doctype());
    /// ```
    pub fn report_doctype(self, on: bool) -> Self {
        XmlConfig { report_doctype: on, ..self }
    }
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// assert!(!XmlConfig::new().report_comments(false).get_report_comments());
    /// ```
    pub fn report_comments(self, on: bool) -> Self {
        XmlConfig { report_comments: on, ..self }
    }
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// assert!(!XmlConfig::new().report_pis(false).get_report_pis());
    /// ```
    pub fn report_pis(self, on: bool) -> Self {
        XmlConfig { report_pis: on, ..self }
    }
    ///
    /// ```
    /// use exi::xml::{EntityPolicy, XmlConfig};
    /// assert_eq!(XmlConfig::new().get_entities(), EntityPolicy::Internal);
    /// ```
    pub fn get_entities(&self) -> EntityPolicy {
        self.entities
    }
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// assert!(XmlConfig::new().report_doctype(true).get_report_doctype());
    /// ```
    pub fn get_report_doctype(&self) -> bool {
        self.report_doctype
    }
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// assert!(XmlConfig::new().get_report_comments());
    /// ```
    pub fn get_report_comments(&self) -> bool {
        self.report_comments
    }
    ///
    /// ```
    /// use exi::xml::XmlConfig;
    /// assert!(XmlConfig::new().get_report_pis());
    /// ```
    pub fn get_report_pis(&self) -> bool {
        self.report_pis
    }
}

impl Default for XmlConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads XML 1.0 text into events, through an input ring of `N` octets.
///
/// Setup used by the examples, and a helper that reads a whole document
/// into owned events.
///
/// ```
/// use exi::xml::{XmlReader, XmlConfig};
/// use exi::{Step, Event, OwnedEvent};
/// use exi::grammar::GrammarSet;
/// use exi::strings::{GlobalStrings, GlobalConfig};
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let set = GrammarSet::schemaless();
/// let grammars = set.attach(&strings)?;
/// let mut r: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
/// r.feed(br#"<svg xmlns="http://www.w3.org/2000/svg"><g/></svg>"#);
/// r.end_input();
/// assert!(matches!(r.next()?, Step::Event(Event::StartDocument, _)));
/// match r.next()? {
///     Step::Event(Event::StartElement(n), meta) => {
///         assert!(n.is("http://www.w3.org/2000/svg", "svg"));
///         assert!(n.id().is_some() && n.global().is_some());
///         assert_eq!(meta.depth, 1);
///     }
///     other => panic!("{other:?}"),
/// }
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Text rules: line ends, CDATA, references, and `]]>`.
///
/// ```
/// # use exi::xml::{XmlReader, XmlConfig};
/// # use exi::{Step, OwnedEvent, OwnedValue, ErrorKind};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let text = |xml: &[u8]| -> exi::Result<Vec<String>> {
///     let mut r: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
///     r.feed(xml);
///     r.end_input();
///     let mut out = Vec::new();
///     loop {
///         match r.next()? {
///             Step::Event(e, _) => if let OwnedEvent::Characters(OwnedValue::Str(s)) = e.to_owned() { out.push(s.into()) },
///             Step::End => return Ok(out),
///             Step::NeedInput => unreachable!(),
///         }
///     }
/// };
/// assert_eq!(text(b"<a>x\r\ny\rz</a>")?, ["x\ny\nz"]);
/// assert_eq!(text(b"<a>1<![CDATA[<2>]]>&amp;&#51;&#x34;</a>")?, ["1<2>&34"]);  // one event
/// assert_eq!(text(b"<a>&#x10FFFF;</a>")?, ["\u{10FFFF}"]);
/// assert_eq!(text(b"<a> </a>")?, [" "]);                      // whitespace inside the root
/// let err = |xml: &[u8]| text(xml).unwrap_err().kind();
/// assert_eq!(err(b"<a>]]></a>"), ErrorKind::Xml);
/// assert_eq!(err(b"<a>&#0;</a>"), ErrorKind::Xml);
/// assert_eq!(err(b"<a>&#xD800;</a>"), ErrorKind::Xml);
/// assert_eq!(err(b"<a>&nope;</a>"), ErrorKind::Xml);           // undeclared
/// assert_eq!(err(b"<a>\xFF</a>"), ErrorKind::Xml);             // malformed UTF-8
/// assert_eq!(err(b"<a>\x01</a>"), ErrorKind::Xml);             // not a Char
/// assert_eq!(err(b"<a/>text"), ErrorKind::Xml);                // text after the root
/// assert_eq!(err(b"<a/><b/>"), ErrorKind::Xml);                // two roots
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Namespaces and attributes.
///
/// ```
/// # use exi::xml::{XmlReader, XmlConfig};
/// # use exi::{Step, ErrorKind};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let check = |xml: &str| -> exi::Result<()> {
///     let mut r: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
///     r.feed(xml.as_bytes());
///     r.end_input();
///     loop { if let Step::End = r.next()? { return Ok(()); } }
/// };
/// let err = |xml: &str| check(xml).unwrap_err().kind();
/// assert_eq!(err(r#"<r xmlns:a="u" xmlns:b="u" a:x="1" b:x="2"/>"#), ErrorKind::Xml);  // same expanded name
/// assert_eq!(err(r#"<r x="1" x="2"/>"#), ErrorKind::Xml);
/// assert_eq!(err(r#"<p:r/>"#), ErrorKind::Xml);                                       // undeclared prefix
/// assert_eq!(err(r#"<r xmlns:p=""/>"#), ErrorKind::Xml);
/// assert_eq!(err(r#"<r xmlns:xml="urn:other"/>"#), ErrorKind::Xml);
/// assert_eq!(err(r#"<r xmlns:xmlns="urn:x"/>"#), ErrorKind::Xml);
/// assert_eq!(err(r#"<r a="<"/>"#), ErrorKind::Xml);
/// assert_eq!(err(r#"<a:b:c xmlns:a="u"/>"#), ErrorKind::Xml);                          // not a QName
/// assert!(check(r#"<r xmlns:xml="http://www.w3.org/XML/1998/namespace" xml:lang="en"/>"#).is_ok());
/// assert!(check(r#"<r xmlns="u"><s xmlns=""/></r>"#).is_ok());
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Comments, PIs, the declaration, encodings and versions.
///
/// ```
/// # use exi::xml::{XmlReader, XmlConfig};
/// # use exi::{Step, ErrorKind, Event};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let count = |xml: &[u8]| -> exi::Result<usize> {
///     let mut r: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
///     r.feed(xml);
///     r.end_input();
///     let mut n = 0;
///     loop { match r.next()? { Step::Event(..) => n += 1, Step::End => return Ok(n), Step::NeedInput => unreachable!() } }
/// };
/// let err = |xml: &[u8]| count(xml).unwrap_err().kind();
/// assert_eq!(count(b"<?xml version=\"1.0\"?><!--c--><?pi d?><a/>")?, 6);   // SD CM PI SE EE ED
/// assert_eq!(err(b"<a><!-- a -- b --></a>"), ErrorKind::Xml);
/// assert_eq!(err(b"<a><!-- x ---></a>"), ErrorKind::Xml);
/// assert_eq!(err(b"<a><?XmL x?></a>"), ErrorKind::Xml);
/// assert_eq!(err(b"<?xml version=\"1.1\"?><a/>"), ErrorKind::Unsupported);
/// assert_eq!(err(b"<?xml version=\"2.0\"?><a/>"), ErrorKind::Xml);
/// assert_eq!(err(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>"), ErrorKind::Unsupported);
/// assert_eq!(err(b"<?xml version=\"1.0\" encoding=\"UTF-16\"?><a/>"), ErrorKind::Xml);  // bytes are UTF-8
/// assert_eq!(err(b" <?xml version=\"1.0\"?><a/>"), ErrorKind::Xml);   // the declaration must be first
/// // UTF-16 little-endian with a byte order mark, and UTF-8 with one:
/// let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("<a/>".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
/// assert_eq!(count(&utf16)?, 4);
/// assert_eq!(count(b"\xEF\xBB\xBF<a/>")?, 4);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Entities: expansion, limits, external references, defaults, and the
/// parameter-entity rule.
///
/// ```
/// # use exi::xml::{XmlReader, XmlConfig, EntityPolicy};
/// # use exi::{Step, ErrorKind, OwnedEvent, Limits};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let events = |cfg: XmlConfig, xml: &str| -> exi::Result<Vec<OwnedEvent>> {
///     let mut r: XmlReader = XmlReader::new(cfg, &grammars);
///     r.feed(xml.as_bytes());
///     r.end_input();
///     let mut out = Vec::new();
///     loop { match r.next()? { Step::Event(e, _) => out.push(e.to_owned()), Step::End => return Ok(out), Step::NeedInput => unreachable!() } }
/// };
/// let ev = |xml: &str| events(XmlConfig::new(), xml);
/// let has = |v: &[OwnedEvent], e: &OwnedEvent| v.contains(e);
///
/// let lol = r#"<!DOCTYPE r [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">
///   <!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;">]><r>&d;&d;</r>"#;
/// // Expanded replacement text: 2 × 30 (d) + 20 × 30 (c) + 200 × 30 (b) + 2000 × 10 (a) = 26 660.
/// let small = XmlConfig::new().limits(Limits::new().max_expanded_bytes(26_659));
/// assert_eq!(events(small, lol).unwrap_err().kind(), ErrorKind::Limit);
/// let exact = XmlConfig::new().limits(Limits::new().max_expanded_bytes(26_660));
/// assert!(events(exact, lol).is_ok());                                  // exactly the limit
/// let shallow = XmlConfig::new().limits(Limits::new().max_entity_depth(3));
/// assert_eq!(events(shallow, lol).unwrap_err().kind(), ErrorKind::Limit); // d → c → b → a is 4
/// assert_eq!(ev(r#"<!DOCTYPE r [<!ENTITY a "&a;">]><r>&a;</r>"#).unwrap_err().kind(), ErrorKind::Xml);
/// assert_eq!(ev(r#"<!DOCTYPE r [<!ENTITY a "<x>">]><r>&a;</r>"#).unwrap_err().kind(), ErrorKind::Xml); // unbalanced
/// // Comments in the internal subset follow the same rules as in content.
/// assert_eq!(ev("<!DOCTYPE r [<!-- a -- b -->]><r/>").unwrap_err().kind(), ErrorKind::Xml);
/// assert_eq!(ev("<!DOCTYPE r [<!-- a --->]><r/>").unwrap_err().kind(), ErrorKind::Xml);
/// assert!(ev("<!DOCTYPE r [<!-- a - b -->]><r/>").is_ok());
///
/// let xxe = ev(r#"<!DOCTYPE r [<!ENTITY e SYSTEM "file:///etc/passwd">]><r>&e;</r>"#)?;
/// assert!(has(&xxe, &OwnedEvent::EntityReference("e".into())));        // reported, never fetched
///
/// let unicode = ev("<!DOCTYPE café [<!ENTITY mot 'été'>]><café>&mot;</café>")?;
/// assert!(unicode.iter().any(|e| matches!(e.as_event(), exi::Event::Characters(v) if v.as_str() == Some("été"))));
/// let defaults = ev(r#"<!DOCTYPE r [<!ATTLIST r lang CDATA "en" kind NMTOKEN " a ">]><r/>"#)?;
/// assert!(has(&defaults, &OwnedEvent::Attribute(exi::OwnedName::new("", "lang"), exi::OwnedValue::Str("en".into()))));
/// assert!(has(&defaults, &OwnedEvent::Attribute(exi::OwnedName::new("", "kind"), exi::OwnedValue::Str("a".into()))));
///
/// // After an unread parameter entity, later ATTLIST declarations are ignored...
/// let pe = r#"<!DOCTYPE r [<!ENTITY % ext SYSTEM "x.dtd">%ext;<!ATTLIST r lang CDATA "en">]><r/>"#;
/// assert!(ev(pe)?.iter().all(|e| !matches!(e, OwnedEvent::Attribute(..))));
/// // ...unless the document is standalone.
/// let standalone = format!("<?xml version=\"1.0\" standalone=\"yes\"?>{pe}");
/// assert!(ev(&standalone)?.iter().any(|e| matches!(e, OwnedEvent::Attribute(..))));
///
/// let only = XmlConfig::new().entities(EntityPolicy::PredefinedOnly);
/// assert_eq!(events(only, r#"<!DOCTYPE r [<!ENTITY a "x">]><r/>"#).unwrap_err().kind(), ErrorKind::Xml);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Positions, limits and partial input.
///
/// ```
/// # use exi::xml::{XmlReader, XmlConfig};
/// # use exi::{Step, ErrorKind, OwnedEvent, Limits};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let read = |cfg: XmlConfig, chunks: &[&[u8]]| -> exi::Result<Vec<OwnedEvent>> {
///     let mut r: XmlReader = XmlReader::new(cfg, &grammars);
///     let mut out = Vec::new();
///     for (i, c) in chunks.iter().enumerate() {
///         let mut rest = *c;
///         loop {                                    // the ring takes what fits
///             rest = &rest[r.feed(rest)..];
///             if rest.is_empty() && i + 1 == chunks.len() { r.end_input(); }
///             loop { match r.next()? { Step::Event(e, _) => out.push(e.to_owned()), Step::NeedInput | Step::End => break } }
///             if rest.is_empty() { break; }
///         }
///     }
///     Ok(out)
/// };
/// let e = read(XmlConfig::new(), &[b"<r>\n  <a></b>\n</r>"]).unwrap_err();
/// assert_eq!((e.kind(), e.position().line, e.position().column), (ErrorKind::Xml, 2, 6)); // at </b>
///
/// let doc = "<r a=\"é\">x<b>😀</b><!--c--></r>".as_bytes();
/// let whole = read(XmlConfig::new(), &[doc])?;
/// for cut in 1..doc.len() {                                  // splits inside UTF-8 too
///     assert_eq!(read(XmlConfig::new(), &[&doc[..cut], &doc[cut..]])?, whole, "cut at {cut}");
/// }
///
/// let deep = "<a>".repeat(10_000) + &"</a>".repeat(10_000);   // no recursion
/// let cfg = XmlConfig::new().limits(Limits::new().max_depth(10_000));
/// assert!(read(cfg, &[deep.as_bytes()]).is_ok());
/// let cfg = XmlConfig::new().limits(Limits::new().max_depth(9_999));
/// assert_eq!(read(cfg, &[deep.as_bytes()]).unwrap_err().kind(), ErrorKind::Limit);
/// let cfg = XmlConfig::new().limits(Limits::new().max_attributes(1));
/// assert_eq!(read(cfg, &[b"<r a='1' b='2'/>"]).unwrap_err().kind(), ErrorKind::Limit);
/// let cfg = XmlConfig::new().limits(Limits::new().max_value_chars(3));
/// assert_eq!(read(cfg, &[b"<r>abcd</r>"]).unwrap_err().kind(), ErrorKind::Limit);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct XmlReader<'g, const N: usize = 4096> {
    ring: InputRing<N>,
    cfg: XmlConfig,
    tables: SessionTables<'g>,
    enc: Enc,
    bom16: bool,
    /// Decoded characters, line ends normalized, from `cur` on not yet
    /// parsed.
    text: String,
    cur: usize,
    line: u32,
    col: u32,
    bytes: u64,
    pending_cr: bool,
    decode_err: Option<Error>,
    ended: bool,
    started: bool,
    phase: XPhase,
    /// Nothing but the byte order mark has been read: an XML
    /// declaration may come.
    at_start: bool,
    standalone: bool,
    seen_doctype: bool,
    dtd: Dtd,
    stack: Vec<Open>,
    ns: Vec<(String, String)>,
    ents: Vec<Src>,
    expanded: u64,
    /// The character data run being collected, and where it started.
    chars: String,
    chars_len: u64,
    chars_pos: Position,
    /// `]` characters just before the current point of a text run.
    brackets: u8,
    queue: VecDeque<(XEv, EventMeta)>,
    current: Option<XEv>,
    poisoned: bool,
    events: u64,
}

impl<'g, const N: usize> XmlReader<'g, N> {
    /// Names are numbered in session tables started from `grammars`'
    /// initial entries.
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartDocument, _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(cfg: XmlConfig, grammars: &'g Grammars<'g>) -> Self {
        let () = InputRing::<N>::VALID;
        let tables = SessionTables::new(
            grammars.initial(),
            ValueRules::unbounded(),
            &cfg.limits,
        );
        XmlReader {
            ring: InputRing::new(),
            cfg,
            tables,
            enc: Enc::Unknown,
            bom16: false,
            text: String::new(),
            cur: 0,
            line: 1,
            col: 1,
            bytes: 0,
            pending_cr: false,
            decode_err: None,
            ended: false,
            started: false,
            phase: XPhase::Prolog,
            at_start: true,
            standalone: false,
            seen_doctype: false,
            dtd: Dtd::default(),
            stack: Vec::new(),
            ns: Vec::new(),
            ents: Vec::new(),
            expanded: 0,
            chars: String::new(),
            chars_len: 0,
            chars_pos: Position::default(),
            brackets: 0,
            queue: VecDeque::new(),
            current: None,
            poisoned: false,
            events: 0,
        }
    }
    /// Same contract as [`Reader::feed`](crate::Reader::feed).
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// assert_eq!(r.feed(b"<root/>"), 7);
    /// r.end_input();
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartDocument, _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn feed(&mut self, data: &[u8]) -> usize {
        self.ring.feed(data)
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// r.buffer()[..4].copy_from_slice(b"<a/>");
    /// r.commit(4);
    /// r.end_input();
    /// r.next()?;
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartElement(_), _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn buffer(&mut self) -> &mut [u8] {
        self.ring.buffer()
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// r.buffer()[..4].copy_from_slice(b"<a/>");
    /// r.commit(4);
    /// r.end_input();
    /// r.next()?;
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartElement(_), _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn commit(&mut self, n: usize) {
        self.ring.commit(n);
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// r.feed(b"<a");
    /// r.end_input();
    /// r.next()?;
    /// assert_eq!(r.next().unwrap_err().kind(), exi::ErrorKind::Xml);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_input(&mut self) {
        self.ended = true;
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// r.feed(b"<root/>");
    /// r.end_input();
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartDocument, _)));
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartElement(_), _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    #[allow(clippy::should_implement_trait)] // events borrow the reader: not an Iterator
    pub fn next(&mut self) -> Result<Step<'_>> {
        self.step()
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// assert_eq!(r.position().byte, 0);
    /// r.feed(b"<root>");
    /// r.next()?;
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartElement(_), _)));
    /// assert_eq!(r.position().byte, 6);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn position(&self) -> Position {
        self.pos_now()
    }
    ///
    /// ```
    /// use exi::{Step, xml::{XmlConfig, XmlReader}};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r = XmlReader::<64>::new(XmlConfig::new(), &grammars);
    /// r.feed(b"<root>");
    /// r.next()?;
    /// assert!(matches!(r.next()?, Step::Event(exi::Event::StartElement(_), _)));
    /// assert_eq!(r.depth(), 1);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn depth(&self) -> u32 {
        self.stack.len() as u32
    }
}

impl<const N: usize> EventSource for XmlReader<'_, N> {
    fn next(&mut self) -> Result<Step<'_>> {
        self.step()
    }
}

// =======================================================================================
// Implementation
// =======================================================================================

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn xml_err(detail: &'static str) -> Error {
    Error::new(ErrorKind::Xml, detail)
}

fn limit(detail: &'static str) -> Error {
    Error::new(ErrorKind::Limit, detail)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Enc {
    Unknown,
    Utf8,
    Utf16Le,
    Utf16Be,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum XPhase {
    Prolog,
    Content,
    Epilog,
    /// ED queued.
    Done,
}

/// The replacement text of an entity being read.
struct Src {
    name: String,
    text: String,
    pos: usize,
    /// Open elements when the entity started: it must end with the same
    /// number.
    depth: usize,
}

#[derive(Clone, Debug)]
enum Entity {
    Internal(String),
    External,
    Unparsed,
}

#[derive(Clone, Debug)]
struct AttDef {
    name: String,
    cdata: bool,
    default: Option<String>,
}

#[derive(Default)]
struct Dtd {
    entities: Vec<(String, Entity)>,
    pes: Vec<(String, Entity)>,
    attlists: Vec<(String, Vec<AttDef>)>,
    has_external: bool,
    unread_pe: bool,
}

impl Dtd {
    fn entity(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|(n, _)| n == name).map(|(_, e)| e)
    }
    fn attdefs(&self, element: &str) -> &[AttDef] {
        self.attlists
            .iter()
            .find(|(n, _)| n == element)
            .map_or(&[], |(_, v)| v)
    }
}

struct Open {
    qname: String,
    id: NameId,
    prefix: String,
    ns_mark: usize,
}

#[derive(Debug)]
enum XEv {
    SD,
    ED,
    SE(NameId, String),
    EE(NameId, String),
    AT(NameId, String, String),
    NS(String, String, bool),
    CH(String),
    CM(String),
    PI(String, String),
    DT(String, String, String, String),
    ER(String),
}

/// What one parsing step did.
enum P {
    Again,
    Produced,
    Need,
}

/// A small cursor over declaration text.
struct Cur<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Cur<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }
    fn ws(&mut self) -> bool {
        let r = self.rest();
        let n = r.len() - r.trim_start_matches(is_space).len();
        self.i += n;
        n > 0
    }
    fn eat(&mut self, lit: &str) -> bool {
        if self.rest().starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> Option<&'a str> {
        let r = self.rest();
        let n = r.find(|c: char| !is_name_char(c)).unwrap_or(r.len());
        let name = &r[..n];
        if is_name(name) {
            self.i += n;
            Some(name)
        } else {
            None
        }
    }
    fn quoted(&mut self) -> Option<&'a str> {
        let r = self.rest();
        let q = r.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let end = r[1..].find(q)?;
        self.i += end + 2;
        Some(&r[1..1 + end])
    }
    fn until(&mut self, lit: &str) -> Option<&'a str> {
        let r = self.rest();
        let end = r.find(lit)?;
        self.i += end + lit.len();
        Some(&r[..end])
    }
    /// Skips to the `>` that ends a declaration, past quoted text.
    fn skip_decl(&mut self) -> Option<()> {
        loop {
            let c = self.rest().chars().next()?;
            match c {
                '"' | '\'' => {
                    self.quoted()?;
                }
                '>' => {
                    self.i += 1;
                    return Some(());
                }
                _ => self.i += c.len_utf8(),
            }
        }
    }
}

/// Replaces character references in an entity value.
fn expand_char_refs(s: &str) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("&#") {
        out.push_str(&rest[..i]);
        let end = rest[i..]
            .find(';')
            .ok_or_else(|| xml_err("an unterminated character reference"))?;
        out.push(char_ref(&rest[i + 2..i + end])?);
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The character of a reference body: `123` or `x7B`.
fn char_ref(body: &str) -> Result<char> {
    let v = match body.strip_prefix('x') {
        Some(h)
            if !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            u32::from_str_radix(h, 16).ok()
        }
        None if !body.is_empty()
            && body.bytes().all(|b| b.is_ascii_digit()) =>
        {
            body.parse().ok()
        }
        _ => None,
    };
    v.and_then(char::from_u32).filter(|c| is_char(*c)).ok_or_else(|| {
        xml_err("a character reference to something that is not a Char")
    })
}

fn predefined(name: &str) -> Option<char> {
    Some(match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "apos" => '\'',
        "quot" => '"',
        _ => return None,
    })
}

fn split_qname(q: &str) -> Result<(&str, &str)> {
    match q.split_once(':') {
        Some((p, l)) if is_ncname(p) && is_ncname(l) => Ok((p, l)),
        None if is_ncname(q) => Ok(("", q)),
        _ => Err(xml_err("a name that is not a QName")),
    }
}

impl<'g, const N: usize> XmlReader<'g, N> {
    fn pos_now(&self) -> Position {
        Position {
            byte: self.bytes,
            bit: 0,
            event: self.events,
            line: self.line,
            column: self.col,
        }
    }

    // ----- Input: bytes to characters -----

    fn push_char(&mut self, c: char) {
        if self.pending_cr {
            self.pending_cr = false;
            self.text.push('\n');
            if c == '\n' {
                return;
            }
        }
        if c == '\r' {
            self.pending_cr = true;
        } else {
            self.text.push(c);
        }
    }

    /// Decodes the ring into `text`, stopping at a malformed or
    /// forbidden character (kept in `decode_err` until the parser
    /// reaches it).
    fn decode_input(&mut self) {
        if self.decode_err.is_some() {
            return;
        }
        let (a, b) = self.ring.filled();
        let mut bytes = Vec::with_capacity(a.len() + b.len());
        bytes.extend_from_slice(a);
        bytes.extend_from_slice(b);
        let mut i = 0;
        if self.enc == Enc::Unknown {
            let bom_prefix = |p: &[u8]| {
                [&[0xEF, 0xBB, 0xBF][..], &[0xFF, 0xFE], &[0xFE, 0xFF]]
                    .iter()
                    .any(|bom| bom.starts_with(p) || p.starts_with(bom))
            };
            if !self.ended && bytes.len() < 3 && bom_prefix(&bytes) {
                return;
            }
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.enc = Enc::Utf8;
                i = 3;
            } else if bytes.starts_with(&[0xFF, 0xFE]) {
                self.enc = Enc::Utf16Le;
                self.bom16 = true;
                i = 2;
            } else if bytes.starts_with(&[0xFE, 0xFF]) {
                self.enc = Enc::Utf16Be;
                self.bom16 = true;
                i = 2;
            } else {
                self.enc = Enc::Utf8;
            }
        }
        while i < bytes.len() {
            let (c, n) = match self.enc {
                Enc::Utf8 | Enc::Unknown => {
                    let b0 = bytes[i];
                    let n = match b0 {
                        0x00..=0x7F => 1,
                        0xC2..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        0xF0..=0xF4 => 4,
                        _ => 0,
                    };
                    if n == 0 {
                        self.decode_err = Some(xml_err("malformed UTF-8"));
                        break;
                    }
                    if i + n > bytes.len() {
                        if self.ended {
                            self.decode_err = Some(xml_err(
                                "malformed UTF-8 at the end of the input",
                            ));
                        }
                        break;
                    }
                    match core::str::from_utf8(&bytes[i..i + n]) {
                        Ok(s) => (s.chars().next().expect("one character"), n),
                        Err(_) => {
                            self.decode_err = Some(xml_err("malformed UTF-8"));
                            break;
                        }
                    }
                }
                Enc::Utf16Le | Enc::Utf16Be => {
                    let unit = |j: usize| -> Option<u16> {
                        let p = bytes.get(j..j + 2)?;
                        Some(if self.enc == Enc::Utf16Le {
                            u16::from_le_bytes([p[0], p[1]])
                        } else {
                            u16::from_be_bytes([p[0], p[1]])
                        })
                    };
                    let Some(u) = unit(i) else {
                        if self.ended {
                            self.decode_err = Some(xml_err(
                                "an odd number of octets in UTF-16",
                            ));
                        }
                        break;
                    };
                    if (0xD800..0xDC00).contains(&u) {
                        let Some(l) = unit(i + 2) else {
                            if self.ended {
                                self.decode_err = Some(xml_err(
                                    "a lone surrogate in UTF-16",
                                ));
                            }
                            break;
                        };
                        match char::decode_utf16([u, l]).next() {
                            Some(Ok(c)) => (c, 4),
                            _ => {
                                self.decode_err = Some(xml_err(
                                    "a lone surrogate in UTF-16",
                                ));
                                break;
                            }
                        }
                    } else {
                        match char::from_u32(u as u32) {
                            Some(c) => (c, 2),
                            None => {
                                self.decode_err = Some(xml_err(
                                    "a lone surrogate in UTF-16",
                                ));
                                break;
                            }
                        }
                    }
                }
            };
            if !is_char(c) {
                self.decode_err =
                    Some(xml_err("a character outside XML Char"));
                break;
            }
            self.push_char(c);
            i += n;
        }
        self.ring.consume(i);
        self.bytes += i as u64;
        if self.ended && self.ring.is_empty() && self.pending_cr {
            self.pending_cr = false;
            self.text.push('\n');
        }
    }

    // ----- Sources -----

    /// The text the parser reads now, and whether no more will come
    /// after it.
    fn peek(&self) -> (&str, bool) {
        match self.ents.last() {
            Some(e) => (&e.text[e.pos..], true),
            None => {
                let fin =
                    self.ended && self.ring.is_empty() && !self.pending_cr;
                (&self.text[self.cur..], fin || self.decode_err.is_some())
            }
        }
    }

    fn adv(&mut self, n: usize) {
        match self.ents.last_mut() {
            Some(e) => e.pos += n,
            None => {
                for c in self.text[self.cur..self.cur + n].chars() {
                    if c == '\n' {
                        self.line += 1;
                        self.col = 1;
                    } else {
                        self.col += 1;
                    }
                }
                self.cur += n;
                if self.cur > 4096 && self.cur * 2 > self.text.len() {
                    self.text.drain(..self.cur);
                    self.cur = 0;
                }
            }
        }
    }

    fn queue(&mut self, ev: XEv, depth: u32, pos: Position) {
        self.queue.push_back((
            ev,
            EventMeta { depth, position: pos, flags: ValueFlags::empty() },
        ));
    }

    fn flush_chars(&mut self) {
        if !self.chars.is_empty() {
            let s = core::mem::take(&mut self.chars);
            let pos = self.chars_pos;
            self.chars_len = 0;
            self.queue(XEv::CH(s), self.stack.len() as u32, pos);
        }
    }

    fn add_chars(&mut self, s: &str) -> Result<()> {
        if self.chars.is_empty() {
            self.chars_pos = self.pos_now();
        }
        self.chars_len += s.chars().count() as u64;
        if self.chars_len > self.cfg.limits.get_max_value_chars() {
            return Err(limit("character data longer than max_value_chars"));
        }
        self.chars.push_str(s);
        Ok(())
    }

    /// [`add_chars`](Self::add_chars) for `from..to` of the text the
    /// parser reads now, without copying it first.
    fn add_input(&mut self, from: usize, to: usize) -> Result<()> {
        if self.chars.is_empty() {
            self.chars_pos = self.pos_now();
        }
        let s = match self.ents.last() {
            Some(e) => &e.text[e.pos + from..e.pos + to],
            None => &self.text[self.cur + from..self.cur + to],
        };
        self.chars_len += s.chars().count() as u64;
        if self.chars_len > self.cfg.limits.get_max_value_chars() {
            return Err(limit("character data longer than max_value_chars"));
        }
        self.chars.push_str(s);
        Ok(())
    }

    fn lookup(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NS);
        }
        match self.ns.iter().rev().find(|(p, _)| p == prefix) {
            Some((_, u)) => Some(u),
            None if prefix.is_empty() => Some(""),
            None => None,
        }
    }

    /// Enters an internal entity, checking recursion and the expansion
    /// limits.
    fn enter_entity(&mut self, name: &str, text: String) -> Result<()> {
        if self.ents.iter().any(|e| e.name == name) {
            return Err(xml_err("an entity that refers to itself"));
        }
        if self.ents.len() as u32 + 1 > self.cfg.limits.get_max_entity_depth()
        {
            return Err(limit("entities nested deeper than max_entity_depth"));
        }
        self.expanded += text.len() as u64;
        if self.expanded > self.cfg.limits.get_max_expanded_bytes() {
            return Err(limit("entity expansion beyond max_expanded_bytes"));
        }
        self.ents.push(Src {
            name: name.into(),
            text,
            pos: 0,
            depth: self.stack.len(),
        });
        Ok(())
    }

    // ----- The parser -----

    fn produce(&mut self) -> Result<bool> {
        loop {
            if let Some(e) = self.ents.last() {
                if e.pos >= e.text.len() {
                    if self.stack.len() != e.depth {
                        return Err(xml_err(
                            "an entity's replacement text is not balanced",
                        ));
                    }
                    self.ents.pop();
                    continue;
                }
            }
            let (s, fin) = self.peek();
            if s.is_empty() {
                if !fin {
                    if self.text.len() - self.cur > self.token_cap() {
                        return Err(limit(
                            "a token longer than the limits allow",
                        ));
                    }
                    return Ok(false);
                }
                if let Some(e) = self.decode_err.take() {
                    return Err(e.at(self.pos_now()));
                }
                return match self.phase {
                    XPhase::Prolog => Err(xml_err("no document element")),
                    XPhase::Content => Err(xml_err(
                        "the input ends inside the document element",
                    )),
                    XPhase::Epilog => {
                        let pos = self.pos_now();
                        self.queue(XEv::ED, 0, pos);
                        self.phase = XPhase::Done;
                        Ok(true)
                    }
                    XPhase::Done => Ok(true),
                };
            }
            let r = match self.phase {
                XPhase::Prolog | XPhase::Epilog => self.misc_step()?,
                XPhase::Content => self.content_step()?,
                XPhase::Done => return Ok(true),
            };
            match r {
                P::Again => continue,
                P::Produced => return Ok(true),
                P::Need => {
                    if self.text.len() - self.cur > self.token_cap() {
                        return Err(limit(
                            "a token longer than the limits allow",
                        ));
                    }
                    return Ok(false);
                }
            }
        }
    }

    fn token_cap(&self) -> usize {
        let l = &self.cfg.limits;
        (l.get_max_value_chars() as usize)
            .saturating_mul(4)
            .saturating_add(l.get_max_expanded_bytes() as usize)
            .saturating_add(1 << 16)
    }

    /// Finds `lit` after `from` in the current text; `Err(P::Need)`
    /// when it may still come.
    fn find(
        &self,
        lit: &str,
        from: usize,
    ) -> Result<core::result::Result<usize, P>> {
        let (s, fin) = self.peek();
        match s.get(from..).and_then(|r| r.find(lit)) {
            Some(i) => Ok(Ok(from + i)),
            None if fin => Err(xml_err("markup that does not end")),
            None => Ok(Err(P::Need)),
        }
    }

    fn misc_step(&mut self) -> Result<P> {
        let (s, fin) = self.peek();
        let c = s.chars().next().expect("not empty");
        if is_space(c) {
            let n = s.len() - s.trim_start_matches(is_space).len();
            self.at_start = false;
            self.adv(n);
            return Ok(P::Again);
        }
        if c != '<' {
            return Err(xml_err(if self.phase == XPhase::Epilog {
                "text after the document element"
            } else {
                "text before the document element"
            })
            .at(self.pos_now()));
        }
        if s.len() < 9
            && !fin
            && ["<!DOCTYPE", "<!--", "<?"]
                .iter()
                .any(|m| m.starts_with(s) && s.len() < m.len())
        {
            return Ok(P::Need);
        }
        if s.starts_with("<?") {
            return self.pi();
        }
        if s.starts_with("<!--") {
            return self.comment();
        }
        if s.starts_with("<!DOCTYPE") {
            if self.phase != XPhase::Prolog || self.seen_doctype {
                return Err(xml_err(
                    "a DOCTYPE after the document element or a second one",
                )
                .at(self.pos_now()));
            }
            return self.doctype();
        }
        if s.starts_with("<!") || s.starts_with("</") {
            return Err(xml_err(
                "markup not allowed outside the document element",
            )
            .at(self.pos_now()));
        }
        if self.phase == XPhase::Epilog {
            return Err(
                xml_err("a second document element").at(self.pos_now())
            );
        }
        self.at_start = false;
        self.start_tag()
    }

    fn content_step(&mut self) -> Result<P> {
        let (s, fin) = self.peek();
        let c = s.chars().next().expect("not empty");
        if c == '<' {
            const CDATA: &str = "<![CDATA[";
            if s.starts_with(CDATA) {
                let end = match self.find("]]>", CDATA.len())? {
                    Ok(e) => e,
                    Err(p) => return Ok(p),
                };
                self.add_input(CDATA.len(), end)?;
                self.brackets = 0;
                self.adv(end + 3);
                return Ok(P::Again);
            }
            if !fin && s.len() < CDATA.len() && CDATA.starts_with(s) {
                return Ok(P::Need);
            }
            if !self.chars.is_empty() {
                self.flush_chars();
                return Ok(P::Produced);
            }
            if s.starts_with("</") {
                return self.end_tag();
            }
            if !fin && s.len() < 4 && "<!--".starts_with(s) {
                return Ok(P::Need);
            }
            if s.starts_with("<!--") {
                return self.comment();
            }
            if s.starts_with("<?") {
                return self.pi();
            }
            if s.starts_with("<!") {
                return Err(xml_err("markup not allowed in content")
                    .at(self.pos_now()));
            }
            return self.start_tag();
        }
        if c == '&' {
            return self.reference();
        }
        // A run of text up to markup or a reference.
        let n = s.find(['<', '&']).unwrap_or(s.len());
        let mut brackets = self.brackets;
        for ch in s[..n].chars() {
            match ch {
                ']' => brackets = brackets.saturating_add(1),
                '>' if brackets >= 2 => {
                    return Err(
                        xml_err("]]> in character data").at(self.pos_now())
                    );
                }
                _ => brackets = 0,
            }
        }
        self.brackets = brackets;
        self.add_input(0, n)?;
        self.adv(n);
        Ok(P::Again)
    }

    fn reference(&mut self) -> Result<P> {
        let pos = self.pos_now();
        let (s, _) = self.peek();
        let Some(end) = s.find(';') else {
            let (_, fin) = self.peek();
            if fin
                || s.len() > 1024
                || s[1..].contains(|c: char| !is_name_char(c) && c != '#')
            {
                return Err(xml_err("an unterminated reference").at(pos));
            }
            return Ok(P::Need);
        };
        let body: String = s[1..end].into();
        self.brackets = 0;
        if let Some(num) = body.strip_prefix('#') {
            let c = char_ref(num).map_err(|e| e.at(pos))?;
            let mut b = [0u8; 4];
            self.add_chars(c.encode_utf8(&mut b))?;
            self.adv(end + 1);
            return Ok(P::Again);
        }
        if let Some(c) = predefined(&body) {
            let mut b = [0u8; 4];
            self.add_chars(c.encode_utf8(&mut b))?;
            self.adv(end + 1);
            return Ok(P::Again);
        }
        if !is_name(&body) {
            return Err(xml_err("a reference that is not a name").at(pos));
        }
        match self.dtd.entity(&body).cloned() {
            Some(Entity::Internal(text)) => {
                self.adv(end + 1);
                self.enter_entity(&body, text).map_err(|e| e.at(pos))?;
                Ok(P::Again)
            }
            Some(Entity::Unparsed) => {
                Err(xml_err("a reference to an unparsed entity").at(pos))
            }
            Some(Entity::External) => self.entity_ref_event(&body, end, pos),
            None if self.dtd.has_external || self.dtd.unread_pe => {
                self.entity_ref_event(&body, end, pos)
            }
            None => Err(xml_err("an undeclared entity").at(pos)),
        }
    }

    fn entity_ref_event(
        &mut self,
        name: &str,
        end: usize,
        pos: Position,
    ) -> Result<P> {
        if !self.chars.is_empty() {
            self.flush_chars();
            return Ok(P::Produced);
        }
        self.adv(end + 1);
        self.queue(XEv::ER(name.into()), self.stack.len() as u32, pos);
        Ok(P::Produced)
    }

    fn comment(&mut self) -> Result<P> {
        let pos = self.pos_now();
        let (s, fin) = self.peek();
        let i = match s[4..].find("--") {
            Some(i) => i + 4,
            None if fin => {
                return Err(xml_err("a comment that does not end").at(pos));
            }
            None => return Ok(P::Need),
        };
        match s.as_bytes().get(i + 2) {
            Some(b'>') => {}
            Some(_) => return Err(xml_err("-- inside a comment").at(pos)),
            None if fin => {
                return Err(xml_err("a comment that does not end").at(pos));
            }
            None => return Ok(P::Need),
        }
        let text: String = s[4..i].into();
        self.at_start = false;
        self.adv(i + 3);
        if self.cfg.report_comments {
            self.queue(XEv::CM(text), self.stack.len() as u32, pos);
            return Ok(P::Produced);
        }
        Ok(P::Again)
    }

    fn pi(&mut self) -> Result<P> {
        let pos = self.pos_now();
        let end = match self.find("?>", 2)? {
            Ok(e) => e,
            Err(p) => return Ok(p),
        };
        let (s, _) = self.peek();
        let body: String = s[2..end].into();
        let tn = body.find(is_space).unwrap_or(body.len());
        let target = &body[..tn];
        let data = body[tn..].trim_start_matches(is_space);
        if tn < body.len() && tn == 0
            || !is_name(target)
            || target.contains(':')
        {
            return Err(xml_err("a PI target that is not a name").at(pos));
        }
        if target.eq_ignore_ascii_case("xml") {
            if target == "xml" && self.at_start && self.phase == XPhase::Prolog
            {
                self.xml_decl(data).map_err(|e| e.at(pos))?;
                self.at_start = false;
                self.adv(end + 2);
                return Ok(P::Again);
            }
            return Err(xml_err("a PI with the target xml").at(pos));
        }
        let (target, data) = (target.to_string(), data.to_string());
        self.at_start = false;
        self.adv(end + 2);
        if self.cfg.report_pis {
            self.queue(XEv::PI(target, data), self.stack.len() as u32, pos);
            return Ok(P::Produced);
        }
        Ok(P::Again)
    }

    fn xml_decl(&mut self, data: &str) -> Result<()> {
        let mut c = Cur { s: data, i: 0 };
        let attr = |c: &mut Cur<'_>, name: &str| -> Result<Option<String>> {
            let save = c.i;
            c.ws();
            if !c.eat(name) {
                c.i = save;
                return Ok(None);
            }
            c.ws();
            if !c.eat("=") {
                return Err(xml_err("a malformed XML declaration"));
            }
            c.ws();
            c.quoted()
                .map(|v| Some(v.into()))
                .ok_or_else(|| xml_err("a malformed XML declaration"))
        };
        let version = attr(&mut c, "version")?
            .ok_or_else(|| xml_err("an XML declaration without a version"))?;
        let encoding = attr(&mut c, "encoding")?;
        let standalone = attr(&mut c, "standalone")?;
        c.ws();
        if !c.done() {
            return Err(xml_err("a malformed XML declaration"));
        }
        match version.as_str() {
            "1.0" => {}
            "1.1" => {
                return Err(Error::new(ErrorKind::Unsupported, "XML 1.1"));
            }
            _ => return Err(xml_err("an XML version other than 1.0")),
        }
        if let Some(e) = encoding {
            let utf16 = matches!(self.enc, Enc::Utf16Le | Enc::Utf16Be);
            match e.to_ascii_lowercase().as_str() {
                "utf-8" if !utf16 => {}
                "utf-16" if utf16 => {}
                "utf-8" | "utf-16" => {
                    return Err(xml_err(
                        "the declared encoding does not match the octets",
                    ));
                }
                _ => {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        "an encoding other than UTF-8 and UTF-16",
                    ));
                }
            }
        }
        match standalone.as_deref() {
            None | Some("no") => {}
            Some("yes") => self.standalone = true,
            Some(_) => return Err(xml_err("standalone must be yes or no")),
        }
        Ok(())
    }

    /// The end of a tag: the `>` outside quoted values.
    fn tag_end(&self) -> Result<core::result::Result<usize, P>> {
        let (s, fin) = self.peek();
        let mut quote = None;
        for (i, c) in s.char_indices().skip(1) {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => return Ok(Ok(i)),
                (None, '<') => return Err(xml_err("< inside a tag")),
                _ => {}
            }
        }
        if fin {
            Err(xml_err("a tag that does not end"))
        } else {
            Ok(Err(P::Need))
        }
    }

    fn start_tag(&mut self) -> Result<P> {
        let pos = self.pos_now();
        let end = match self.tag_end().map_err(|e| e.at(pos))? {
            Ok(e) => e,
            Err(p) => return Ok(p),
        };
        let (s, _) = self.peek();
        let token: String = s[1..end].into();
        let (body, empty) = match token.strip_suffix('/') {
            Some(b) => (b, true),
            None => (token.as_str(), false),
        };
        let mut c = Cur { s: body, i: 0 };
        let qname = c
            .name()
            .ok_or_else(|| xml_err("a start tag without a name").at(pos))?;
        let mut raw: Vec<(&str, &str)> = Vec::new();
        loop {
            let had_space = c.ws();
            if c.done() {
                break;
            }
            if !had_space {
                return Err(xml_err(
                    "attributes must be separated by white space",
                )
                .at(pos));
            }
            let name = c
                .name()
                .ok_or_else(|| xml_err("a malformed attribute").at(pos))?;
            c.ws();
            if !c.eat("=") {
                return Err(xml_err("an attribute without a value").at(pos));
            }
            c.ws();
            let v = c.quoted().ok_or_else(|| {
                xml_err("an attribute value without quotes").at(pos)
            })?;
            if raw.iter().any(|(n, _)| *n == name) {
                return Err(xml_err("an attribute given twice").at(pos));
            }
            raw.push((name, v));
        }
        self.adv(end + 1);
        self.element(qname, raw, empty, pos).map_err(|e| {
            if e.position() == Position::default() { e.at(pos) } else { e }
        })?;
        Ok(P::Produced)
    }

    /// Attribute value normalization (XML 1.0 §3.3.3), with references
    /// expanded.
    fn normalize(
        &mut self,
        raw: &str,
        out: &mut String,
        depth: u32,
        open: &mut Vec<String>,
    ) -> Result<()> {
        let mut rest = raw;
        while !rest.is_empty() {
            let i =
                rest.find(['&', '<', '\t', '\n', '\r']).unwrap_or(rest.len());
            out.push_str(&rest[..i]);
            rest = &rest[i..];
            let Some(c) = rest.chars().next() else { break };
            match c {
                '<' => return Err(xml_err("< in an attribute value")),
                '\t' | '\n' | '\r' => {
                    out.push(' ');
                    rest = &rest[1..];
                }
                _ => {
                    let end = rest
                        .find(';')
                        .ok_or_else(|| xml_err("an unterminated reference"))?;
                    let body = &rest[1..end];
                    if let Some(num) = body.strip_prefix('#') {
                        out.push(char_ref(num)?);
                    } else if let Some(c) = predefined(body) {
                        out.push(c);
                    } else {
                        match self.dtd.entity(body).cloned() {
                            Some(Entity::Internal(text)) => {
                                if open.iter().any(|n| n == body) {
                                    return Err(xml_err(
                                        "an entity that refers to itself",
                                    ));
                                }
                                if depth + 1
                                    > self.cfg.limits.get_max_entity_depth()
                                {
                                    return Err(limit(
                                        "entities nested deeper than max_entity_depth",
                                    ));
                                }
                                self.expanded += text.len() as u64;
                                if self.expanded
                                    > self.cfg.limits.get_max_expanded_bytes()
                                {
                                    return Err(limit(
                                        "entity expansion beyond max_expanded_bytes",
                                    ));
                                }
                                open.push(body.into());
                                self.normalize(&text, out, depth + 1, open)?;
                                open.pop();
                            }
                            Some(_) => {
                                return Err(xml_err(
                                    "an external or unparsed entity in an attribute value",
                                ));
                            }
                            None => {
                                return Err(xml_err("an undeclared entity"));
                            }
                        }
                    }
                    rest = &rest[end + 1..];
                }
            }
        }
        Ok(())
    }

    fn element(
        &mut self,
        qname: &str,
        raw: Vec<(&str, &str)>,
        empty: bool,
        pos: Position,
    ) -> Result<()> {
        let limits = self.cfg.limits;
        if self.stack.len() as u32 + 1 > limits.get_max_depth() {
            return Err(limit("elements nested deeper than max_depth"));
        }
        // Values, then defaults from the internal subset.
        let defs: Vec<AttDef> = self.dtd.attdefs(qname).to_vec();
        let mut attrs = Vec::with_capacity(raw.len());
        for (n, v) in raw {
            let mut out = String::new();
            self.normalize(v, &mut out, 0, &mut Vec::new())?;
            attrs.push((n, out));
        }
        for d in &defs {
            if let Some(dv) = &d.default {
                if !attrs.iter().any(|(n, _)| *n == d.name) {
                    let mut out = String::new();
                    self.normalize(dv, &mut out, 0, &mut Vec::new())?;
                    attrs.push((d.name.as_str(), out));
                }
            }
        }
        for (n, v) in &mut attrs {
            if defs.iter().any(|d| d.name == *n && !d.cdata) {
                *v = v
                    .split(' ')
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            if v.chars().count() as u64 > limits.get_max_value_chars() {
                return Err(limit(
                    "an attribute value longer than max_value_chars",
                ));
            }
        }
        // Namespace declarations.
        let ns_mark = self.ns.len();
        let mut decls: Vec<(String, String)> = Vec::new();
        let mut plain = Vec::new();
        for (n, v) in attrs {
            let prefix = if n == "xmlns" {
                Some(String::new())
            } else {
                n.strip_prefix("xmlns:").map(String::from)
            };
            match prefix {
                Some(p) => {
                    if !p.is_empty() && !is_ncname(&p) {
                        return Err(xml_err(
                            "a namespace prefix that is not an NCName",
                        ));
                    }
                    if p == "xmlns" {
                        return Err(xml_err(
                            "the xmlns prefix cannot be declared",
                        ));
                    }
                    if v == XMLNS_NS {
                        return Err(xml_err(
                            "the xmlns namespace cannot be bound",
                        ));
                    }
                    if (p == "xml") != (v == XML_NS) {
                        return Err(xml_err(
                            "the xml prefix and its namespace go only with each other",
                        ));
                    }
                    if !p.is_empty() && v.is_empty() {
                        return Err(xml_err(
                            "a prefix cannot be bound to the empty URI",
                        ));
                    }
                    decls.push((p, v));
                }
                None => plain.push((n, v)),
            }
        }
        if plain.len() as u32 > limits.get_max_attributes() {
            return Err(limit("more attributes than max_attributes"));
        }
        for (p, u) in &decls {
            self.ns.push((p.clone(), u.clone()));
        }
        let (eprefix, elocal) = split_qname(qname)?;
        let euri = self
            .lookup(eprefix)
            .ok_or_else(|| xml_err("an undeclared prefix"))?
            .to_string();
        let mut resolved: Vec<(String, String, String, String)> =
            Vec::with_capacity(plain.len());
        for (n, v) in plain {
            let (p, l) = split_qname(n)?;
            let uri = if p.is_empty() {
                String::new()
            } else {
                self.lookup(p)
                    .ok_or_else(|| xml_err("an undeclared prefix"))?
                    .to_string()
            };
            if resolved.iter().any(|(u, l2, _, _)| *u == uri && l2 == l) {
                return Err(xml_err(
                    "two attributes with the same expanded name",
                ));
            }
            resolved.push((uri, l.to_string(), p.to_string(), v));
        }
        let id = self.tables.add_name(&euri, elocal)?;
        let depth = self.stack.len() as u32 + 1;
        let eprefix = eprefix.to_string();
        self.queue(XEv::SE(id, eprefix.clone()), depth, pos);
        let mut marked = false;
        for (p, u) in decls {
            let local = !marked && p == eprefix && u == euri;
            marked |= local;
            self.queue(XEv::NS(p, u, local), depth, pos);
        }
        for (uri, l, p, v) in resolved {
            let aid = self.tables.add_name(&uri, &l)?;
            self.queue(XEv::AT(aid, p, v), depth, pos);
        }
        if empty {
            self.ns.truncate(ns_mark);
            self.queue(XEv::EE(id, eprefix), depth - 1, pos);
            if self.stack.is_empty() {
                self.phase = XPhase::Epilog;
            } else {
                self.phase = XPhase::Content;
            }
        } else {
            self.stack.push(Open {
                qname: qname.into(),
                id,
                prefix: eprefix,
                ns_mark,
            });
            self.phase = XPhase::Content;
        }
        Ok(())
    }

    fn end_tag(&mut self) -> Result<P> {
        let pos = self.pos_now();
        let end = match self.tag_end().map_err(|e| e.at(pos))? {
            Ok(e) => e,
            Err(p) => return Ok(p),
        };
        let (s, _) = self.peek();
        let name = s[2..end].trim_end_matches(is_space).to_string();
        let open = self.stack.last().ok_or_else(|| {
            xml_err("an end tag with no element open").at(pos)
        })?;
        if open.qname != name {
            return Err(xml_err(
                "an end tag that does not match the start tag",
            )
            .at(pos));
        }
        if let Some(e) = self.ents.last() {
            if self.stack.len() <= e.depth {
                return Err(xml_err(
                    "an entity's replacement text is not balanced",
                )
                .at(pos));
            }
        }
        self.adv(end + 1);
        let open = self.stack.pop().expect("checked above");
        self.ns.truncate(open.ns_mark);
        self.queue(
            XEv::EE(open.id, open.prefix),
            self.stack.len() as u32,
            pos,
        );
        if self.stack.is_empty() {
            self.phase = XPhase::Epilog;
        }
        Ok(P::Produced)
    }

    // ----- The DOCTYPE -----

    fn doctype(&mut self) -> Result<P> {
        let pos = self.pos_now();
        let (s, fin) = self.peek();
        // Find the closing '>' past quotes, the internal subset, and
        // comments and PIs in it.
        let b = s.as_bytes();
        let mut i = 9;
        let mut subset = false;
        let mut end = None;
        while i < b.len() {
            match b[i] {
                q @ (b'"' | b'\'') => match s[i + 1..].find(q as char) {
                    Some(j) => i += j + 2,
                    None => break,
                },
                b'<' if subset && s[i..].starts_with("<!--") => {
                    match s[i..].find("-->") {
                        Some(j) => i += j + 3,
                        None => break,
                    }
                }
                b'<' if subset && s[i..].starts_with("<?") => {
                    match s[i..].find("?>") {
                        Some(j) => i += j + 2,
                        None => break,
                    }
                }
                b'[' if !subset => {
                    subset = true;
                    i += 1;
                }
                b']' if subset => {
                    subset = false;
                    i += 1;
                }
                b'>' if !subset => {
                    end = Some(i);
                    break;
                }
                _ => i += 1,
            }
        }
        let Some(end) = end else {
            if fin {
                return Err(xml_err("a DOCTYPE that does not end").at(pos));
            }
            return Ok(P::Need);
        };
        let token: String = s[9..end].into();
        self.adv(end + 1);
        self.seen_doctype = true;
        self.at_start = false;
        let mut c = Cur { s: &token, i: 0 };
        let bad = || xml_err("a malformed DOCTYPE").at(pos);
        if !c.ws() {
            return Err(bad());
        }
        let name = c.name().ok_or_else(bad)?.to_string();
        c.ws();
        let (mut public, mut system) = (String::new(), String::new());
        if c.eat("SYSTEM") {
            c.ws();
            system = c.quoted().ok_or_else(bad)?.into();
            self.dtd.has_external = true;
        } else if c.eat("PUBLIC") {
            c.ws();
            public = c.quoted().ok_or_else(bad)?.into();
            c.ws();
            system = c.quoted().ok_or_else(bad)?.into();
            self.dtd.has_external = true;
        }
        c.ws();
        let mut text = String::new();
        if c.eat("[") {
            let r = c.rest();
            let close = r.rfind(']').ok_or_else(bad)?;
            text = r[..close].into();
            c.i += close + 1;
            c.ws();
        }
        if !c.done() {
            return Err(bad());
        }
        self.subset(&text, 0).map_err(|e| {
            if e.position() == Position::default() { e.at(pos) } else { e }
        })?;
        if self.cfg.report_doctype {
            self.queue(XEv::DT(name, public, system, text), 0, pos);
            return Ok(P::Produced);
        }
        Ok(P::Again)
    }

    fn subset(&mut self, text: &str, depth: u32) -> Result<()> {
        let mut c = Cur { s: text, i: 0 };
        let bad = || xml_err("a malformed declaration in the internal subset");
        loop {
            c.ws();
            if c.done() {
                return Ok(());
            }
            if c.eat("%") {
                let name = c.name().ok_or_else(bad)?.to_string();
                if !c.eat(";") {
                    return Err(bad());
                }
                match self
                    .dtd
                    .pes
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, e)| e.clone())
                {
                    Some(Entity::Internal(t)) => {
                        if depth + 1 > self.cfg.limits.get_max_entity_depth() {
                            return Err(limit(
                                "entities nested deeper than max_entity_depth",
                            ));
                        }
                        self.expanded += t.len() as u64;
                        if self.expanded
                            > self.cfg.limits.get_max_expanded_bytes()
                        {
                            return Err(limit(
                                "entity expansion beyond max_expanded_bytes",
                            ));
                        }
                        self.subset(&t, depth + 1)?;
                    }
                    _ => self.dtd.unread_pe = true,
                }
            } else if c.eat("<!--") {
                let text = c.until("-->").ok_or_else(bad)?;
                if text.contains("--") || text.ends_with('-') {
                    return Err(xml_err("-- inside a comment"));
                }
            } else if c.eat("<?") {
                c.until("?>").ok_or_else(bad)?;
            } else if c.eat("<!ENTITY") {
                if self.cfg.entities == EntityPolicy::PredefinedOnly {
                    return Err(xml_err(
                        "an entity declaration under EntityPolicy::PredefinedOnly",
                    ));
                }
                if !c.ws() {
                    return Err(bad());
                }
                let pe = c.eat("%");
                if pe && !c.ws() {
                    return Err(bad());
                }
                let name = c.name().ok_or_else(bad)?.to_string();
                c.ws();
                let ent = if let Some(v) = c.quoted() {
                    Entity::Internal(expand_char_refs(v)?)
                } else {
                    if c.eat("SYSTEM") {
                        c.ws();
                        c.quoted().ok_or_else(bad)?;
                    } else if c.eat("PUBLIC") {
                        c.ws();
                        c.quoted().ok_or_else(bad)?;
                        c.ws();
                        c.quoted().ok_or_else(bad)?;
                    } else {
                        return Err(bad());
                    }
                    c.ws();
                    if c.eat("NDATA") {
                        c.ws();
                        c.name().ok_or_else(bad)?;
                        Entity::Unparsed
                    } else {
                        Entity::External
                    }
                };
                c.ws();
                if !c.eat(">") {
                    return Err(bad());
                }
                let ignore = self.dtd.unread_pe && !self.standalone;
                if !ignore {
                    let list = if pe {
                        &mut self.dtd.pes
                    } else {
                        &mut self.dtd.entities
                    };
                    if !list.iter().any(|(n, _)| *n == name) {
                        list.push((name, ent));
                    }
                }
            } else if c.eat("<!ATTLIST") {
                if !c.ws() {
                    return Err(bad());
                }
                let element = c.name().ok_or_else(bad)?.to_string();
                let mut defs = Vec::new();
                loop {
                    c.ws();
                    if c.eat(">") {
                        break;
                    }
                    let name = c.name().ok_or_else(bad)?.to_string();
                    c.ws();
                    let cdata = c.eat("CDATA");
                    if !cdata {
                        if c.eat("NOTATION") {
                            c.ws();
                        }
                        if c.eat("(") {
                            c.until(")").ok_or_else(bad)?;
                        } else {
                            c.name().ok_or_else(bad)?;
                        }
                    }
                    c.ws();
                    let default = if c.eat("#REQUIRED") || c.eat("#IMPLIED") {
                        None
                    } else {
                        if c.eat("#FIXED") {
                            c.ws();
                        }
                        Some(c.quoted().ok_or_else(bad)?.to_string())
                    };
                    defs.push(AttDef { name, cdata, default });
                }
                let ignore = self.dtd.unread_pe && !self.standalone;
                if !ignore {
                    match self
                        .dtd
                        .attlists
                        .iter_mut()
                        .find(|(n, _)| *n == element)
                    {
                        Some((_, list)) => {
                            for d in defs {
                                if !list.iter().any(|x| x.name == d.name) {
                                    list.push(d);
                                }
                            }
                        }
                        None => self.dtd.attlists.push((element, defs)),
                    }
                }
            } else if c.eat("<!ELEMENT") || c.eat("<!NOTATION") {
                c.skip_decl().ok_or_else(bad)?;
            } else {
                return Err(bad());
            }
        }
    }

    fn step<'g2>(&'g2 mut self) -> Result<Step<'g2>> {
        if self.poisoned {
            return Err(Error::new(
                ErrorKind::State,
                "the reader is poisoned by an earlier error",
            ));
        }
        if self.queue.is_empty() {
            if self.phase == XPhase::Done {
                if self.ended {
                    self.current = None;
                    return Ok(Step::End);
                }
                return Ok(Step::NeedInput);
            }
            if !self.started {
                self.started = true;
                let pos = self.pos_now();
                self.queue(XEv::SD, 0, pos);
            } else {
                self.decode_input();
                match self.produce() {
                    Ok(true) => {}
                    Ok(false) => return Ok(Step::NeedInput),
                    Err(e) => {
                        self.poisoned = true;
                        return Err(if e.position() == Position::default() {
                            e.at(self.pos_now())
                        } else {
                            e
                        });
                    }
                }
                if self.queue.is_empty() {
                    return if self.phase == XPhase::Done && self.ended {
                        Ok(Step::End)
                    } else {
                        Ok(Step::NeedInput)
                    };
                }
            }
        }
        let (ev, meta) = self.queue.pop_front().expect("not empty");
        self.current = Some(ev);
        self.events += 1;
        let this: &Self = self;
        let name = |id: NameId, prefix: &'g2 str| -> Name<'g2> {
            xname(&this.tables, id, prefix)
        };
        let ev = match this.current.as_ref().expect("set above") {
            XEv::SD => Event::StartDocument,
            XEv::ED => Event::EndDocument,
            XEv::SE(id, p) => Event::StartElement(name(*id, p)),
            XEv::EE(id, p) => Event::EndElement(name(*id, p)),
            XEv::AT(id, p, v) => Event::Attribute(name(*id, p), Value::Str(v)),
            XEv::NS(p, u, l) => Event::Namespace(NsDecl {
                prefix: p,
                uri: u,
                local_element_ns: *l,
            }),
            XEv::CH(s) => Event::Characters(Value::Str(s)),
            XEv::CM(s) => Event::Comment(s),
            XEv::PI(t, d) => {
                Event::ProcessingInstruction { target: t, data: d }
            }
            XEv::DT(n, p, s, t) => Event::Doctype(Doctype {
                name: n,
                public_id: p,
                system_id: s,
                text: t,
            }),
            XEv::ER(n) => Event::EntityReference(n),
        };
        Ok(Step::Event(ev, meta))
    }
}

fn xname<'a>(
    t: &'a SessionTables<'_>,
    id: NameId,
    prefix: &'a str,
) -> Name<'a> {
    let n = Name::new(
        t.uri_str(id.uri).unwrap_or(""),
        t.local_str(id.uri, id.local).unwrap_or(""),
    );
    let n = if prefix.is_empty() { n } else { n.with_prefix(prefix) };
    let global = match (t.uri(id.uri), t.local(id.uri, id.local)) {
        (Some(uri), Some(local)) => Some(GlobalName { uri, local }),
        _ => None,
    };
    n.with_ids(Some(id), global)
}
