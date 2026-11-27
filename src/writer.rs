// src/writer.rs
//
// The EXI writer events in/out and coding options
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Use
//!
//! Create a [`Writer`] over a [`Sink`], send events through
//! [`EventSink`] (or its provided methods), and finish with
//! [`Writer::end_document`]. SD is implicit before the first
//! event; an explicit `StartDocument` event is accepted once, first.
//!
//! # Rules the writer applies
//!
//! 1. **Header.** Written before the first event:
//!    [`WriterConfig::header`], default `Header::WithOptions { cookie:
//!    false }`.
//! 2. **Start tags are buffered** until the first content event,
//!    `end_element` or `flush`, then written in EXI's order: SE, NS
//!    events, `xsi:type`, `xsi:nil`, then the other attributes. Under
//!    schema-informed grammars the declared attributes are sorted by
//!    local name, then URI; callers need not sort. Schema-less streams
//!    keep the order given.
//! 3. **Events the options do not keep are dropped** without error:
//!    comments without `Preserve.comments`, and so on (see
//!    [`crate::event`]).
//! 4. **`xmlns` attributes** given as attributes become NS events with
//!    `Preserve.prefixes`, and are dropped without it. Namespace
//!    declarations are never written as attributes.
//! 5. **Names** use the session's string tables and learned productions are always
//!    used when they exist.
//! 6. **Typed values** (schema-informed): a value given as text for a
//!    typed slot is parsed into the slot's datatype; a typed value is
//!    checked against it.
//! 7. **Refused events**: under `strict`, an event that fits no
//!    production — an undeclared element or attribute, a value the type
//!    cannot hold, a missing required attribute before content —
//!    returns `ErrorKind::Unencodable`. Nothing is written and nothing
//!    changes; the writer stays usable. The caller skips the event (for
//!    a start element, the whole element) and carries on. A buffered
//!    start tag (rule 2) is written before the next event is matched,
//!    because what may follow depends on the whole tag. When the
//!    element's type has sub-types, its attributes are checked only
//!    then (an `xsi:type` could still come), and a refusal there
//!    poisons the writer: the SE is already written.
//! 8. **Insignificant whitespace** (schema-informed): character data
//!    that is all XML whitespace, where the state has no CH production,
//!    is dropped. Indentation between elements of element-only content
//!    is not part of the document's data (EXIficient does the same).
//! 9. **Empty values** (schema-informed): an element that ends where
//!    its state allows only CH, such as `<name/>` of type `xs:string`,
//!    gets an empty value first. An empty value the type cannot hold,
//!    such as `<count/>` of type `xs:int`, is `Unencodable`.
//! 10. **Blocks** (pre-compression and compression): values wait until
//!     their block of `blockSize` values is complete, then go out in
//!     channel order, with the string tables updated in that order (EXI
//!     §9, `crate::coding::block`). The DEFLATE level is
//!     [`WriterConfig::deflate_level`].
//! 11. **Ends.** `end_document` writes ED, pads to an octet, writes the
//!     last block, and flushes. It is an error (`ErrorKind::State`)
//!     while elements are open, or before the document element of a
//!     non-fragment stream.
//!
//! Every other error poisons the writer: later calls return
//! `ErrorKind::State`.
//!
//! # Flushing
//!
//! [`Writer::flush`] commits a buffered start tag and pushes every
//! complete octet to the sink. What stays behind depends on the coding:
//!
//! | Coding | Held after `flush` |
//! |---|---|
//! | byte-aligned | nothing: the receiver has every event written so far |
//! | bit-packed | up to 7 bits of the last event (`flush` returns how many) |
//! | pre-compression, compression | the whole current block, until it fills or the document ends |
//!
//! # Frames
//!
//! With `Sequence::Frames` the options must set `fragment` and byte
//! alignment, or [`Writer::new`] fails with `ErrorKind::Arg`. The
//! header goes at the start of the first frame. [`Writer::end_frame`]
//! is valid right after a top-level element ends, and returns
//! the octets written since the previous frame. Joined end to end, the
//! frames are one EXI fragment stream; `end_document` adds its ED.

use alloc::string::String;
use alloc::vec::Vec;

use crate::coding::body::BodyEncoder;
use crate::coding::datatypes::write_string;
use crate::coding::{Encoder, Sink, names};
use crate::engine::{
    self, Prepared, Slot, State, Top, WriteCode, check_options, err,
};
use crate::error::Position;
use crate::event::{Event, NsDecl};
use crate::grammar::informed::ElementId;
use crate::grammar::{EventCode, Grammars, Terminal};
use crate::header::{Header, write_header};
use crate::name::{Name, OwnedName};
use crate::reader::Sequence;
use crate::stream::EventSink;
use crate::tables::SessionTables;
use crate::value::{OwnedValue, Value};
use crate::{Coding, ErrorKind, Limits, NameId, Options, Preserve, Result};

mod attributes;

const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// Configuration for a [`Writer`].
///
/// Every coding writes the same events: attributes, prefixed names, namespaces and
/// repeated values here are read back unchanged.
///
/// ```
/// # #[cfg(all(feature = "bit-packed", feature = "compression"))] {
/// use exi::{Coding, EventSink, Options, OwnedEvent, Preserve, Reader, ReaderConfig, Step, Writer, WriterConfig};
/// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let mut seen: Vec<Vec<OwnedEvent>> = Vec::new();
/// for coding in [Coding::BitPacked, Coding::ByteAligned, Coding::PreCompression, Coding::Compression] {
///     let opts = Options::builder()
///         .coding(coding)
///         .preserve(Preserve::PREFIXES)
///         .build()?;
///     let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &grammars))?;
///     w.start_element(("urn:feed", "feed"))?;
///     w.namespace("", "urn:feed")?;
///     w.namespace("x", "urn:x")?;
///     for i in 0..150 {
///         w.start_element(("urn:feed", "item"))?;
///         w.attribute("id", i.to_string().as_str())?;
///         w.attribute(("urn:x", "kind"), if i % 2 == 0 { "even" } else { "odd" })?;
///         w.text("same text")?;
///         w.end_element()?;
///     }
///     w.end_element()?;
///     w.end_document()?;
///     let bytes = w.into_sink();
///     let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
///     r.feed(&bytes);
///     r.end_input();
///     let mut events = Vec::new();
///     while let Step::Event(e, _) = r.next()? { events.push(e.to_owned()); }
///     seen.push(events);
/// }
/// assert_eq!(seen[0].len(), 2 + 4 + 150 * 5);              // SD SE NS NS EE ED; SE AT AT CH EE
/// assert!(seen.windows(2).all(|p| p[0] == p[1]));
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Clone)]
pub struct WriterConfig<'g> {
    options: Options,
    grammars: &'g Grammars<'g>,
    header: Header,
    sequence: Sequence,
    limits: Limits,
    count_deviations: bool,
    deflate_level: u32,
    canonical: bool,
    utc_time: bool,
}

impl<'g> WriterConfig<'g> {
    /// Writes streams with `options` informed by `grammars`: one stream, options in the
    /// header, no cookie, default limits.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Options, Reader, ReaderConfig, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(byte.clone(), &grammars))?;
    /// w.start_element("a")?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// let bytes = w.into_sink();
    /// assert_eq!(bytes[0], 0xA0);                              // options follow
    /// // So a reader needs no options of its own.
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
    /// r.feed(&bytes);
    /// r.end_input();
    /// r.next()?;
    /// assert_eq!(r.options(), Some(&byte));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(options: Options, grammars: &'g Grammars<'g>) -> Self {
        WriterConfig {
            options,
            grammars,
            header: Header::default(),
            sequence: Sequence::Single,
            limits: Limits::new(),
            count_deviations: false,
            deflate_level: 6,
            canonical: false,
            utc_time: false,
        }
    }
    /// Canonical EXI: the same events, options
    /// and grammars always give the same octets, in this writer and in
    /// other implementations of EXI C14N. Refuses a header with the
    /// cookie and `Coding::Compression` (use pre-compression).
    ///
    /// ```
    /// use exi::{grammar::GrammarSet, strings::*, *};
    /// let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let set = GrammarSet::schemaless();
    /// let g = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let write = |canonical: bool| -> Result<Vec<u8>> {
    ///     let cfg = WriterConfig::new(byte.clone(), &g).canonical(canonical);
    ///     let mut w = Writer::new(Vec::new(), cfg)?;
    ///     w.start_element("a")?;
    ///     w.attribute("z", "1")?;
    ///     w.attribute("y", "2")?;
    ///     w.text("x")?;
    ///     w.text("y")?; // merged with the text before
    ///     w.end_element()?;
    ///     w.end_document()?;
    ///     Ok(w.into_sink())
    /// };
    /// assert_ne!(write(false)?, write(true)?);
    /// // Canonical: attributes sorted, one CH. The same as writing them so.
    /// let cfg = WriterConfig::new(byte.clone(), &g);
    /// let mut w = Writer::new(Vec::new(), cfg)?;
    /// w.start_element("a")?;
    /// w.attribute("y", "2")?;
    /// w.attribute("z", "1")?;
    /// w.text("xy")?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// assert_eq!(w.into_sink(), write(true)?);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn canonical(self, on: bool) -> Self {
        WriterConfig { canonical: on, ..self }
    }
    /// With [`canonical`](Self::canonical): date-times with a time zone are written in UTC
    /// (EXI C14N's `utcTime`).
    ///
    /// ```
    /// use exi::{Coding, DateTimeKind, EventSink, Header, OwnedName, Options, Writer, WriterConfig};
    /// use exi::grammar::{GrammarSet, informed::Representation};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// let mut b = GrammarSet::builder();
    /// let dt = b.simple_type(None, Representation::DateTime(DateTimeKind::DateTime));
    /// let ty = b.simple_content_type(None, dt);
    /// let at = b.element(OwnedName::new("", "at"), ty, false);
    /// b.global(at);
    /// let set = b.build()?;
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let grammars = set.attach(&strings)?;
    /// let strict = Options::builder().coding(Coding::ByteAligned).strict(true).build()?;
    /// let write = |utc: bool, when: &str| -> exi::Result<Vec<u8>> {
    ///     let cfg = WriterConfig::new(strict.clone(), &grammars)
    ///         .header(Header::Bare { cookie: false })
    ///         .canonical(true)
    ///         .utc_time(utc);
    ///     let mut w = Writer::new(Vec::new(), cfg)?;
    ///     w.start_element("at")?;
    ///     w.text(when)?;
    ///     w.end_element()?;
    ///     w.end_document()?;
    ///     Ok(w.into_sink())
    /// };
    /// // The same instant: written the same way in UTC, differently without.
    /// let (east, utc) = ("2026-11-25T10:00:00+02:00", "2026-11-25T08:00:00Z");
    /// assert_eq!(write(true, east)?, write(true, utc)?);
    /// assert_ne!(write(false, east)?, write(false, utc)?);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn utc_time(self, on: bool) -> Self {
        WriterConfig { utc_time: on, ..self }
    }
    /// The DEFLATE level, 0 (store) to 9 (smallest), for
    /// `Coding::Compression`; 6 when not set. Readers need no setting:
    /// every level decodes the same way. Above 9 is 9.
    ///
    /// ```
    /// # #[cfg(feature = "compression")] {
    /// use exi::{Coding, EventSink, Options, Reader, ReaderConfig, Step, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let deflate = Options::builder().coding(Coding::Compression).build()?;
    /// let write = |level: u32| -> exi::Result<Vec<u8>> {
    ///     let cfg = WriterConfig::new(deflate.clone(), &grammars).deflate_level(level);
    ///     let mut w = Writer::new(Vec::new(), cfg)?;
    ///     w.start_element("log")?;
    ///     for i in 0..300 {
    ///         w.start_element("line")?;
    ///         w.text(format!("event {} of the day", i % 7).as_str())?;
    ///         w.end_element()?;
    ///     }
    ///     w.end_element()?;
    ///     w.end_document()?;
    ///     Ok(w.into_sink())
    /// };
    /// let (stored, smallest) = (write(0)?, write(9)?);
    /// assert!(smallest.len() < stored.len());
    /// assert_eq!(write(99)?, smallest);                        // above 9 is 9
    /// let count = |bytes: &[u8]| -> exi::Result<usize> {
    ///     let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
    ///     r.feed(bytes);
    ///     r.end_input();
    ///     let mut n = 0;
    ///     while let Step::Event(..) = r.next()? { n += 1; }
    ///     Ok(n)
    /// };
    /// assert_eq!(count(&stored)?, count(&smallest)?);
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn deflate_level(self, level: u32) -> Self {
        WriterConfig { deflate_level: level.min(9), ..self }
    }
    /// Count what the schema does not declare (non-strict
    /// schema-informed streams): [`Writer::deviations`].
    ///
    /// ```
    /// use exi::{Coding, DeviationKind, EventSink, OwnedName, Options, Writer, WriterConfig};
    /// use exi::grammar::{GrammarSet, informed::{Prod, Representation}};
    /// # use exi::strings::{GlobalConfig, GlobalStrings};
    /// let mut b = GrammarSet::builder();
    /// let string = b.simple_type(None, Representation::String { charset: None });
    /// let ty = b.complex_type(None, vec![
    ///     vec![Prod::At { name: OwnedName::new("", "unit"), ty: string, next: 1 }],
    ///     vec![Prod::EE],
    /// ]);
    /// let reading = b.element(OwnedName::new("", "reading"), ty, false);
    /// b.global(reading);
    /// let set = b.build()?;
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let grammars = set.attach(&strings)?;
    /// let loose = Options::builder().coding(Coding::ByteAligned).build()?;  // not strict
    /// let cfg = WriterConfig::new(loose, &grammars).count_deviations(true);
    /// let mut w = Writer::new(Vec::new(), cfg)?;
    /// w.start_element("reading")?;
    /// w.attribute("unit", "C")?;
    /// w.attribute("where", "attic")?;                          // undeclared
    /// w.end_element()?;
    /// w.end_document()?;
    /// let d = w.deviations().unwrap();
    /// assert_eq!(d.iter().collect::<Vec<_>>(), [(DeviationKind::Attribute, "{}reading", "{}where", 1)]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn count_deviations(self, on: bool) -> Self {
        WriterConfig { count_deviations: on, ..self }
    }
    /// What the header holds: [`Header`]. Options in it by default.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let write = |h: Header| -> exi::Result<Vec<u8>> {
    ///     let mut w = Writer::new(Vec::new(), WriterConfig::new(byte.clone(), &grammars).header(h))?;
    ///     w.start_element("a")?;
    ///     w.end_element()?;
    ///     w.end_document()?;
    ///     Ok(w.into_sink())
    /// };
    /// let body = [0x01, 0x02, b'a', 0x00];
    /// assert_eq!(write(Header::Bare { cookie: false })?, [&[0x80][..], &body].concat());
    /// assert_eq!(write(Header::Bare { cookie: true })?, [&b"$EXI\x80"[..], &body].concat());
    /// assert_eq!(write(Header::WithOptions { cookie: false })?[..3], [0xA0, 0x00, 0x4A]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn header(self, h: Header) -> Self {
        WriterConfig { header: h, ..self }
    }
    /// How streams follow each other: [`Sequence`]. Frames are fragments, byte-aligned;
    /// [`Writer::end_frame`] has an example.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Options, Sequence, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let frag = Options::builder().coding(Coding::ByteAligned).fragment(true).build()?;
    /// let doc = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let frames = |o: Options| WriterConfig::new(o, &grammars).sequence(Sequence::Frames);
    /// assert!(Writer::new(Vec::new(), frames(frag)).is_ok());
    /// assert_eq!(Writer::new(Vec::new(), frames(doc)).err().unwrap().kind(), ErrorKind::Arg);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn sequence(self, s: Sequence) -> Self {
        WriterConfig { sequence: s, ..self }
    }
    /// The [`Limits`] events are checked against.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, EventSink, Limits, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let cfg = WriterConfig::new(byte, &grammars).limits(Limits::new().max_depth(2));
    /// let mut w = Writer::new(Vec::new(), cfg)?;
    /// w.start_element("a")?;
    /// w.start_element("b")?;
    /// assert_eq!(w.start_element("c").unwrap_err().kind(), ErrorKind::Limit);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn limits(self, l: Limits) -> Self {
        WriterConfig { limits: l, ..self }
    }
}

/// What a schema did not declare by name, by kind: names matched by a
/// wildcard or by EXI's added productions (§8.5.4.4.1), and content
/// only those could encode.
///
/// ```
/// use exi::{Coding, DeviationKind, EventSink, OwnedName, Options, Writer, WriterConfig};
/// use exi::grammar::{GrammarSet, informed::Prod};
/// # use exi::strings::{GlobalConfig, GlobalStrings};
/// let mut b = GrammarSet::builder();
/// let empty = b.complex_type(None, vec![vec![Prod::EE]]);
/// let root = b.element(OwnedName::new("", "root"), empty, false);
/// b.global(root);
/// let set = b.build()?;
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let grammars = set.attach(&strings)?;
/// let loose = Options::builder().coding(Coding::ByteAligned).build()?;
/// let mut w = Writer::new(Vec::new(), WriterConfig::new(loose, &grammars).count_deviations(true))?;
/// w.start_element("root")?;                                // declared empty
/// w.start_element("extra")?;
/// w.end_element()?;
/// w.text("note")?;
/// w.end_element()?;
/// w.end_document()?;
/// let kinds: Vec<DeviationKind> = w.deviations().unwrap().iter().map(|d| d.0).collect();
/// assert!(kinds.contains(&DeviationKind::Element));
/// assert!(kinds.contains(&DeviationKind::Text));
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviationKind {
    /// An element the parent's type does not name: SE(\*) or
    /// SE(uri:\*).
    Element,
    /// An attribute the element's type does not name: AT(\*) or
    /// AT(uri:\*).
    Attribute,
    /// A declared attribute whose value its type cannot hold (AT
    /// \[untyped value\]).
    UntypedValue,
    /// Text where the type declares none, or that its type cannot hold
    /// (CH \[untyped\]).
    Text,
    /// An element that ended before its content model allowed (EE).
    EarlyEnd,
}

/// Counts of [`DeviationKind`]s, by element and name: what a page
/// needed that the schema lacks, for proposing the next schema
/// version.
///
/// ```
/// use exi::{DeviationKind, Deviations};
/// let mut d = Deviations::default();
/// d.add(DeviationKind::Attribute, "{urn:x}p", "{}data-x");
/// d.add(DeviationKind::Attribute, "{urn:x}p", "{}data-x");
/// assert_eq!(d.iter().next(), Some((DeviationKind::Attribute, "{urn:x}p", "{}data-x", 2)));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Deviations {
    counts: alloc::collections::BTreeMap<(DeviationKind, String, String), u64>,
}

impl Deviations {
    /// One more: names in Clark notation (`{uri}local`); `name` empty
    /// for text and ends.
    ///
    /// ```
    /// use exi::{DeviationKind, Deviations};
    /// let mut d = Deviations::default();
    /// d.add(DeviationKind::Text, "{urn:x}p", "");
    /// d.add(DeviationKind::Text, "{urn:x}p", "");
    /// d.add(DeviationKind::Element, "{urn:x}p", "{urn:x}b");
    /// assert_eq!(d.total(), 3);
    /// assert_eq!(d.iter().count(), 2);                         // two distinct deviations
    /// ```
    pub fn add(&mut self, kind: DeviationKind, element: &str, name: &str) {
        *self
            .counts
            .entry((kind, element.into(), name.into()))
            .or_insert(0) += 1;
    }
    /// Adds another set's counts.
    ///
    /// ```
    /// use exi::{DeviationKind, Deviations};
    /// let mut first = Deviations::default();
    /// first.add(DeviationKind::Attribute, "{}item", "{}extra");
    /// let second = first.clone();
    /// first.merge(&second);
    /// assert_eq!(first.total(), 2);
    /// assert_eq!(second.total(), 1);
    /// ```
    pub fn merge(&mut self, other: &Deviations) {
        for (k, n) in &other.counts {
            *self.counts.entry(k.clone()).or_insert(0) += n;
        }
    }
    /// Every count, most frequent first.
    ///
    /// ```
    /// use exi::{DeviationKind, Deviations};
    /// let mut d = Deviations::default();
    /// d.add(DeviationKind::Attribute, "{}p", "{}a");
    /// for _ in 0..3 { d.add(DeviationKind::Element, "{}p", "{}b"); }
    /// let all: Vec<_> = d.iter().collect();
    /// assert_eq!(all, [
    ///     (DeviationKind::Element, "{}p", "{}b", 3),
    ///     (DeviationKind::Attribute, "{}p", "{}a", 1),
    /// ]);
    /// ```
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (DeviationKind, &str, &str, u64)> + '_ {
        let mut v: Vec<_> = self
            .counts
            .iter()
            .map(|((k, e, n), c)| (*k, e.as_str(), n.as_str(), *c))
            .collect();
        v.sort_by(|a, b| {
            b.3.cmp(&a.3).then((a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)))
        });
        v.into_iter()
    }
    /// All counts added up.
    ///
    /// ```
    /// use exi::{DeviationKind, Deviations};
    /// let mut d = Deviations::default();
    /// assert_eq!(d.total(), 0);
    /// d.add(DeviationKind::EarlyEnd, "{}p", "");
    /// d.add(DeviationKind::UntypedValue, "{}p", "{}n");
    /// assert_eq!(d.total(), 2);
    /// ```
    pub fn total(&self) -> u64 {
        self.counts.values().sum()
    }
}

/// Writes one EXI stream (or a sequence of frames) to a sink.
///
/// Setup used by the examples: schema-less grammars, byte-aligned
/// options agreed out of band, so the header is the single octet
/// `0x80`.
///
/// ```
/// use exi::{Writer, WriterConfig, Options, Coding, Header, EventSink};
/// use exi::grammar::GrammarSet;
/// use exi::strings::{GlobalStrings, GlobalConfig};
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let set = GrammarSet::schemaless();
/// let grammars = set.attach(&strings)?;
/// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// let cfg = || WriterConfig::new(byte.clone(), &grammars).header(Header::Bare { cookie: false });
///
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("a")?;
/// w.end_element()?;
/// w.end_document()?;
/// assert_eq!(w.into_sink(), [0x80, 0x01, 0x02, b'a', 0x00]);
///
/// // <a>hi</a>
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("a")?;
/// w.text("hi")?;
/// w.end_element()?;
/// w.end_document()?;
/// assert_eq!(w.into_sink(), [0x80, 0x01, 0x02, b'a', 0x03, 0x04, b'h', b'i', 0x00]);
///
/// // <r><a>x</a><a>x</a></r>: the second "x" is a local hit; the second <a> uses the
/// // productions r and a learned the first time.
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("r")?;
/// for _ in 0..2 { w.start_element("a")?; w.text("x")?; w.end_element()?; }
/// w.end_element()?;
/// w.end_document()?;
/// assert_eq!(w.into_sink(), [0x80, 0x01, 0x02, b'r', 0x02, 0x01, 0x02, b'a', 0x03, 0x03, b'x', 0x00,
///                            0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Order and state errors poison the writer.
///
/// ```
/// # use exi::{Writer, WriterConfig, Options, Coding, Header, EventSink, ErrorKind, Limits};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// # let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// # let cfg = || WriterConfig::new(byte.clone(), &grammars).header(Header::Bare { cookie: false });
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// assert_eq!(w.end_element().unwrap_err().kind(), ErrorKind::State);   // nothing open
/// assert_eq!(w.start_element("a").unwrap_err().kind(), ErrorKind::State); // poisoned
///
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("a")?;
/// assert_eq!(w.end_document().unwrap_err().kind(), ErrorKind::State);  // <a> still open
///
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("a")?;
/// w.end_element()?;
/// assert_eq!(w.start_element("b").unwrap_err().kind(), ErrorKind::State); // a second root
///
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("a")?;
/// w.text("x")?;
/// assert_eq!(w.attribute("late", "1").unwrap_err().kind(), ErrorKind::State); // after content
///
/// let mut w = Writer::new(Vec::new(), cfg().limits(Limits::new().max_depth(1)))?;
/// w.start_element("a")?;
/// assert_eq!(w.start_element("b").unwrap_err().kind(), ErrorKind::Limit);
///
/// let mut buf = [0u8; 2];
/// let mut w = Writer::new(exi::coding::SliceSink::new(&mut buf), cfg())?;
/// w.start_element("long-name")?;
/// assert_eq!(w.flush().unwrap_err().kind(), ErrorKind::BufferFull);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Events the options do not keep are dropped.
///
/// ```
/// # use exi::{Writer, WriterConfig, Options, Coding, Header, EventSink};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// # let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// # let cfg = || WriterConfig::new(byte.clone(), &grammars).header(Header::Bare { cookie: false });
/// let mut w = Writer::new(Vec::new(), cfg())?;
/// w.start_element("a")?;
/// w.comment("dropped")?;
/// w.processing_instruction("dropped", "")?;
/// w.namespace("p", "urn:x")?;
/// w.end_element()?;
/// w.end_document()?;
/// assert_eq!(w.into_sink(), [0x80, 0x01, 0x02, b'a', 0x00]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Strict schema-informed: `<reading
/// unit="C"><temp>21</temp></reading>` (the grammar of [`GrammarBuilder`](crate::grammar::informed::GrammarBuilder)'s example). Every code
/// has one alternative except DocContent's, so only values take octets.
/// A refused event changes nothing.
///
/// ```
/// use exi::{Writer, WriterConfig, Options, Coding, Header, EventSink, ErrorKind, OwnedName};
/// use exi::grammar::{GrammarSet, informed::{Prod, Representation, IntegerRep}};
/// use exi::strings::{GlobalStrings, GlobalConfig};
/// const NS: &str = "urn:example:sensor:1";
/// let mut b = GrammarSet::builder();
/// let int = b.simple_type(None, Representation::Integer(IntegerRep::Signed));
/// let string = b.simple_type(None, Representation::String { charset: None });
/// let temp_type = b.simple_content_type(None, int);
/// let temp = b.element(OwnedName::new(NS, "temp"), temp_type, false);
/// let reading_type = b.complex_type(None, vec![
///     vec![Prod::At { name: OwnedName::new("", "unit"), ty: string, next: 1 }],
///     vec![Prod::Se { element: temp, next: 2 }],
///     vec![Prod::EE],
/// ]);
/// let reading = b.element(OwnedName::new(NS, "reading"), reading_type, false);
/// b.global(reading);
/// let set = b.build()?;
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let grammars = set.attach(&strings)?;
/// let strict = Options::builder().coding(Coding::ByteAligned).strict(true).build()?;
/// let mut w = Writer::new(Vec::new(), WriterConfig::new(strict, &grammars).header(Header::Bare { cookie: false }))?;
/// w.start_element((NS, "reading"))?;
/// let err = w.start_element(("urn:other", "x")).unwrap_err();     // undeclared
/// assert!(err.kind() == ErrorKind::Unencodable && !err.is_fatal());
/// let err = w.start_element((NS, "temp")).unwrap_err();            // unit must come first
/// assert_eq!(err.kind(), ErrorKind::Unencodable);
/// w.attribute("unit", "C")?;
/// w.start_element((NS, "temp"))?;
/// assert_eq!(w.text("warm").unwrap_err().kind(), ErrorKind::Unencodable);  // not an integer
/// w.text("21")?;                                                    // text parsed as the type
/// w.end_element()?;
/// w.end_element()?;
/// w.end_document()?;
/// // DocContent SE(reading) 0 of 2; "C" literal 3, 'C'; 21 as sign 0, magnitude 21.
/// assert_eq!(w.into_sink(), [0x80, 0x00, 0x03, b'C', 0x00, 0x15]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Values and attributes strict grammars refuse, and a namespace declared too late.
///
/// ```
/// # #[cfg(feature = "xsd")] {
/// use exi::{Coding, ErrorKind, EventSink, Options, Preserve, Writer, WriterConfig};
/// use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalConfig, GlobalStrings};
/// const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// let set = GrammarSet::from_xsd(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
///   <xs:complexType name="Base"><xs:sequence>
///     <xs:element name="v" type="xs:string" minOccurs="0"/>
///   </xs:sequence></xs:complexType>
///   <xs:complexType name="Sub"><xs:complexContent><xs:extension base="Base"/></xs:complexContent></xs:complexType>
///   <xs:element name="d" type="xs:decimal"/>
///   <xs:element name="f" type="xs:double"/>
///   <xs:element name="b" type="xs:base64Binary"/>
///   <xs:element name="t" type="xs:dateTime"/>
///   <xs:element name="n" type="xs:int" nillable="true"/>
///   <xs:element name="k" type="Base"/>
/// </xs:schema>"#)?;
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let grammars = set.attach(&strings)?;
/// let strict = Options::builder().coding(Coding::ByteAligned).strict(true).build()?;
/// let writer = || Writer::new(Vec::new(), WriterConfig::new(strict.clone(), &grammars));
/// let text = |element: &str, text: &str| -> exi::Result<()> {
///     let mut w = writer()?;
///     w.start_element(element)?;
///     w.text(text)
/// };
/// let attr = |element: &str, name: (&str, &str), value: &str| -> exi::Result<()> {
///     let mut w = writer()?;
///     w.start_element(element)?;
///     w.attribute(name, value)?;
///     w.end_element()
/// };
/// let kind = |r: exi::Result<()>| r.unwrap_err().kind();
/// text("d", "1.5")?;
/// assert_eq!(kind(text("d", "one and a half")), ErrorKind::Unencodable);
/// assert_eq!(kind(text("f", "fast")), ErrorKind::Unencodable);
/// assert_eq!(kind(text("b", "not base64!")), ErrorKind::Unencodable);
/// assert_eq!(kind(text("t", "Tuesday")), ErrorKind::Unencodable);
/// attr("n", (XSI, "nil"), "true")?;
/// assert_eq!(kind(attr("n", (XSI, "nil"), "maybe")), ErrorKind::Unencodable);
/// assert_eq!(kind(attr("d", ("", "extra"), "1")), ErrorKind::Unencodable);   // undeclared
/// attr("k", (XSI, "type"), "Sub")?;
/// assert_eq!(kind(attr("k", (XSI, "type"), "p:Sub")), ErrorKind::Unencodable); // no prefix p
/// assert_eq!(kind(attr("k", (XSI, "type"), "Missing")), ErrorKind::Unencodable);
///
/// // With prefixes kept, a namespace must come before content.
/// let prefixes = Options::builder().coding(Coding::ByteAligned).preserve(Preserve::PREFIXES).build()?;
/// let schemaless = GrammarSet::schemaless();
/// let g = schemaless.attach(&strings)?;
/// let mut w = Writer::new(Vec::new(), WriterConfig::new(prefixes, &g))?;
/// w.start_element("a")?;
/// w.text("x")?;
/// assert_eq!(w.namespace("p", "urn:p").unwrap_err().kind(), ErrorKind::State);
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
pub struct Writer<'g, S: Sink> {
    enc: BodyEncoder<FrameSink<S>>,
    cfg: WriterConfig<'g>,
    st: State<'g>,
    tables: SessionTables<'g>,
    /// The header has been written.
    started: bool,
    poisoned: bool,
    /// A start tag not yet written: its SE was checked, its NS and AT
    /// events wait.
    pending: Option<Pending>,
    spare_attrs: Vec<(OwnedName, OwnedValue)>,
    /// The innermost open element has had no content yet: attributes
    /// may still come.
    in_start: bool,
    /// With `Preserve.prefixes`: the prefix and URI of that element
    /// while no NS event has been marked `local_element_ns` for it.
    unmarked: Option<(String, String)>,
    /// Namespace bindings in scope, for QName values given as text:
    /// prefix, URI, depth.
    bindings: Vec<(String, String, u32)>,
    events: u64,
    /// A top-level element just ended (frames).
    frame_ready: bool,
    deviations: Option<Deviations>,
    /// Canonical EXI: text waiting for the next event, and for each
    /// open element whether it has had a child element and whether
    /// `xml:space="preserve"` is in scope.
    ctext: Option<CText>,
    canonical_elements: Vec<CanonicalElement>,
}

struct CanonicalElement {
    has_child: bool,
    preserve_space: bool,
}

/// Canonical EXI: character data waiting for the next event. One value
/// stays as given (a typed value keeps all it holds); runs that merge
/// are text.
enum CText {
    Text(String),
    Value(crate::OwnedValue),
}

/// The sink as the encoder sees it: with `Sequence::Frames` output
/// collects in a frame buffer and reaches the sink at `end_frame`.
struct FrameSink<S: Sink> {
    sink: S,
    frames: bool,
    buf: Vec<u8>,
    last: Vec<u8>,
}

impl<S: Sink> Sink for FrameSink<S> {
    fn write(&mut self, octets: &[u8]) -> Result<()> {
        if self.frames {
            self.buf.extend_from_slice(octets);
            Ok(())
        } else {
            self.sink.write(octets)
        }
    }
}

impl<S: Sink> FrameSink<S> {
    fn push_frame(&mut self) -> Result<()> {
        self.sink.write(&self.buf)?;
        self.last.clear();
        core::mem::swap(&mut self.last, &mut self.buf);
        Ok(())
    }
}

struct Pending {
    // A declared SE already carries its name when prefixes and SC are
    // off.
    name: Option<OwnedName>,
    term: Terminal,
    code: WriteCode,
    /// The declaration the element enters, when schema-informed.
    decl: Option<ElementId>,
    ns: Vec<(String, String)>,
    attrs: Vec<(OwnedName, OwnedValue)>,
}

fn state(detail: &'static str) -> crate::Error {
    err(ErrorKind::State, detail)
}

fn unencodable(detail: &'static str) -> crate::Error {
    err(ErrorKind::Unencodable, detail)
}

fn is_xmlns(n: Name<'_>) -> Option<String> {
    if n.uri() == XMLNS_NS {
        return Some(if n.local_name() == "xmlns" {
            String::new()
        } else {
            n.local_name().into()
        });
    }
    if n.uri().is_empty() {
        if n.prefix() == Some("xmlns") {
            return Some(n.local_name().into());
        }
        if n.prefix().is_none() && n.local_name() == "xmlns" {
            return Some(String::new());
        }
    }
    None
}

impl<'g, S: Sink> Writer<'g, S> {
    /// # Errors
    ///
    /// `ErrorKind::Arg` for an inconsistent configuration (frames
    /// without `fragment` and byte alignment); `ErrorKind::Unsupported`
    /// for a coding whose feature is not built (`bit-packed`,
    /// `compression`), or schema-informed grammars without `strict` but
    /// with `selfContained`; `ErrorKind::Schema` when the options'
    /// `schemaId` contradicts the grammars (nil with schema-informed
    /// grammars).
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Header, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// assert!(Writer::new(Vec::new(), WriterConfig::new(byte.clone(), &grammars)).is_ok());
    /// // Canonical EXI has no cookie and no compression.
    /// let cookie = WriterConfig::new(byte, &grammars).canonical(true).header(Header::Bare { cookie: true });
    /// assert_eq!(Writer::new(Vec::new(), cookie).err().unwrap().kind(), ErrorKind::Arg);
    /// # #[cfg(feature = "compression")] {
    /// let deflate = Options::builder().coding(Coding::Compression).build()?;
    /// let cfg = WriterConfig::new(deflate, &grammars).canonical(true);
    /// assert_eq!(Writer::new(Vec::new(), cfg).err().unwrap().kind(), ErrorKind::Arg);
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(sink: S, cfg: WriterConfig<'g>) -> Result<Self> {
        cfg.sequence.check_options(&cfg.options)?;
        check_options(&cfg.options, cfg.grammars)?;
        if cfg.canonical {
            if matches!(
                cfg.header,
                Header::WithOptions { cookie: true }
                    | Header::Bare { cookie: true }
            ) {
                return Err(err(
                    ErrorKind::Arg,
                    "canonical EXI has no cookie",
                ));
            }
            if cfg.options.coding() == Coding::Compression {
                return Err(err(
                    ErrorKind::Arg,
                    "canonical EXI does not use compression: use pre-compression",
                ));
            }
        }
        let (st, tables) =
            engine::new_session(cfg.options.clone(), cfg.grammars, cfg.limits);
        let cfg_counts = cfg.count_deviations;
        Ok(Writer {
            enc: Self::encoder(sink, &cfg),
            cfg,
            st,
            tables,
            started: false,
            poisoned: false,
            pending: None,
            spare_attrs: Vec::new(),
            in_start: false,
            unmarked: None,
            bindings: Vec::new(),
            events: 0,
            frame_ready: false,
            deviations: if cfg_counts {
                Some(Deviations::default())
            } else {
                None
            },
            ctext: None,
            canonical_elements: Vec::new(),
        })
    }

    /// What the schema did not declare, when counted
    /// ([`WriterConfig::count_deviations`]).
    ///
    /// ```
    /// use exi::{Coding, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let w = Writer::new(Vec::new(), WriterConfig::new(byte.clone(), &grammars))?;
    /// assert!(w.deviations().is_none());                       // not counted
    /// let w = Writer::new(Vec::new(), WriterConfig::new(byte, &grammars).count_deviations(true))?;
    /// assert_eq!(w.deviations().unwrap().total(), 0);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn deviations(&self) -> Option<&Deviations> {
        self.deviations.as_ref()
    }
    /// ED, padding, flush.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, EventSink, Header, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// # #[cfg(feature = "bit-packed")] {
    /// let bits = Options::builder().coding(Coding::BitPacked).build()?;
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(bits, &grammars).header(Header::Bare { cookie: false }))?;
    /// w.start_element("a")?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// assert_eq!(w.position().bit, 0);                         // padded to an octet
    /// assert_eq!(w.end_document().unwrap_err().kind(), ErrorKind::State);  // once only
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_document(&mut self) -> Result<()> {
        self.guard(|w| w.end_doc())
    }
    /// Commits a buffered start tag and pushes complete octets; returns
    /// the bits still pending (always 0 for byte-aligned coding).
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let cfg = WriterConfig::new(opts, &grammars).header(Header::Bare { cookie: false });
    /// let mut writer = Writer::new(Vec::new(), cfg)?;
    /// writer.start_element("a")?;
    /// assert_eq!(writer.flush()?, 0);
    /// assert_eq!(writer.position().byte, 3); // body octets
    /// writer.end_element()?;
    /// writer.end_document()?;
    /// assert_eq!(writer.into_sink(), [0x80, 0x01, 0x02, b'a', 0x00]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn flush(&mut self) -> Result<u8> {
        self.guard(|w| {
            w.start()?;
            w.commit()?;
            w.enc.flush()
        })
    }
    /// `Sequence::Frames`: the octets of the frame that just ended.
    /// Valid until the next call on the writer.
    ///
    /// # Errors
    ///
    /// `ErrorKind::State` unless a top-level element has just ended.
    ///
    /// Frames: two messages on one connection, each handed over as soon
    /// as it ends.
    ///
    /// ```
    /// use exi::{Writer, WriterConfig, Options, Coding, Header, EventSink, Sequence};
    /// use exi::grammar::GrammarSet;
    /// use exi::strings::{GlobalStrings, GlobalConfig};
    /// let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let set = GrammarSet::schemaless();
    /// let grammars = set.attach(&strings)?;
    /// let frag = Options::builder().coding(Coding::ByteAligned).fragment(true).build()?;
    /// let cfg = WriterConfig::new(frag, &grammars).header(Header::Bare { cookie: false }).sequence(Sequence::Frames);
    /// let mut w = Writer::new(Vec::new(), cfg)?;
    /// w.start_element("m")?;
    /// w.end_element()?;
    /// // Header, FragmentContent SE(*) 0 of 2, the name, EE 0.0.
    /// assert_eq!(w.end_frame()?.to_vec(), [0x80, 0x00, 0x01, 0x02, b'm', 0x00]);
    /// w.start_element("m")?;
    /// w.end_element()?;
    /// // FragmentContent learned SE(m): SE(m) 0 of 3. m learned EE: 0 of 2.
    /// assert_eq!(w.end_frame()?.to_vec(), [0x00, 0x00]);
    /// assert_eq!(w.end_frame().unwrap_err().kind(), exi::ErrorKind::State);   // no element since
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_frame(&mut self) -> Result<&[u8]> {
        self.guard(|w| {
            if w.cfg.sequence != Sequence::Frames {
                return Err(state("end_frame without Sequence::Frames"));
            }
            if !w.frame_ready {
                return Err(state(
                    "end_frame is valid right after a top-level element ends",
                ));
            }
            w.frame_ready = false;
            w.enc.sink_mut().push_frame()
        })?;
        Ok(&self.enc.sink().last)
    }
    /// Where the next event starts: octets and bits of the body, and events written.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// # #[cfg(feature = "bit-packed")] {
    /// let bits = Options::builder().coding(Coding::BitPacked).build()?;
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(bits, &grammars).header(Header::Bare { cookie: false }))?;
    /// assert_eq!(w.position().event, 0);
    /// w.start_element("a")?;
    /// w.flush()?;
    /// let p = w.position();
    /// assert_eq!(p.event, 1);                                  // events given; SD is implied
    /// assert!(p.bit != 0);                                     // mid-octet
    /// # }
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn position(&self) -> Position {
        let bits = self.enc.bit_position();
        Position {
            byte: bits / 8,
            bit: (bits % 8) as u8,
            event: self.events,
            line: 0,
            column: 0,
        }
    }
    /// Starts a new stream with the same configuration and returns
    /// the old sink. Clears learned grammars, session tables, deviation
    /// counts, and any poisoned state.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut writer = Writer::new(Vec::new(), WriterConfig::new(opts, &grammars))?;
    /// let write = |writer: &mut Writer<'_, Vec<u8>>| -> exi::Result<()> {
    ///     writer.start_element("reading")?;
    ///     writer.text("warm")?;
    ///     writer.end_element()?;
    ///     writer.end_document()
    /// };
    /// write(&mut writer)?;
    /// let first = writer.reset(Vec::new());
    /// assert_eq!(writer.position().event, 0);
    /// write(&mut writer)?;
    /// assert_eq!(writer.into_sink(), first);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn reset(&mut self, sink: S) -> S {
        let fresh = Self::encoder(sink, &self.cfg);
        let old = core::mem::replace(&mut self.enc, fresh);
        let (st, tables) = engine::new_session(
            self.cfg.options.clone(),
            self.cfg.grammars,
            self.cfg.limits,
        );
        self.tables = tables;
        self.st = st;
        self.started = false;
        self.poisoned = false;
        if let Some(mut p) = self.pending.take() {
            p.attrs.clear();
            self.spare_attrs = p.attrs;
        }
        self.in_start = false;
        self.unmarked = None;
        self.bindings.clear();
        self.events = 0;
        self.frame_ready = false;
        self.ctext = None;
        self.canonical_elements.clear();
        if let Some(d) = &mut self.deviations {
            *d = Deviations::default();
        }
        old.into_sink().sink
    }
    /// The sink, with everything written to it.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut w = Writer::new(vec![0xFF], WriterConfig::new(byte, &grammars).header(Header::Bare { cookie: false }))?;
    /// w.start_element("a")?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// assert_eq!(w.into_sink(), [0xFF, 0x80, 0x01, 0x02, b'a', 0x00]);   // appended
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn into_sink(self) -> S {
        self.enc.into_sink().sink
    }
}

impl<S: Sink> EventSink for Writer<'_, S> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        self.guard(|w| w.handle(ev))
    }
}

impl<'g, S: Sink> Writer<'g, S> {
    /// The sink, for the C API's accumulating writer.
    #[cfg(feature = "capi")]
    pub(crate) fn sink_mut(&mut self) -> &mut S {
        &mut self.enc.sink_mut().sink
    }

    fn encoder(sink: S, cfg: &WriterConfig<'g>) -> BodyEncoder<FrameSink<S>> {
        let frames = cfg.sequence == Sequence::Frames;
        BodyEncoder::new(
            &cfg.options,
            cfg.deflate_level,
            FrameSink { sink, frames, buf: Vec::new(), last: Vec::new() },
        )
    }

    fn guard<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if self.poisoned {
            return Err(state("the writer is poisoned by an earlier error"));
        }
        let r = f(self);
        if let Err(e) = &r {
            if e.is_fatal() {
                self.poisoned = true;
            }
        }
        r
    }

    fn start(&mut self) -> Result<()> {
        if !self.started {
            if cfg!(feature = "bit-packed")
                && self.cfg.options.coding() == Coding::BitPacked
            {
                // The body continues in the header's last octet.
                crate::header::write_header_bits(
                    &mut self.enc,
                    self.cfg.header,
                    &self.cfg.options,
                )?;
            } else {
                write_header(
                    self.enc.sink_mut(),
                    self.cfg.header,
                    &self.cfg.options,
                )?;
            }
            self.started = true;
        }
        Ok(())
    }

    /// Writes the value of an AT or CH event, the last part of the
    /// event. With pre-compression and compression the value waits for
    /// the end of its block, and the block closes when it holds
    /// `blockSize` values (EXI §9.1).
    fn value(
        &mut self,
        name: NameId,
        v: Prepared,
        prefixes: bool,
    ) -> Result<()> {
        let v = match v {
            _ if self.cfg.canonical => {
                crate::canonical::value(v, self.cfg.utc_time)
            }
            // As EXIficient: 24:00:00 is written as 00:00:00 of the
            // next day.
            Prepared::DateTime(d) if d.hour == 24 => {
                Prepared::DateTime(crate::canonical::hour_24(d))
            }
            v => v,
        };
        match &mut self.enc {
            #[cfg(feature = "compression")]
            BodyEncoder::Block(b) => {
                if b.defer(name, v, prefixes) {
                    b.close(&mut self.tables)?;
                }
                Ok(())
            }
            _ => engine::emit_value(
                &mut self.enc,
                &mut self.tables,
                name,
                &v,
                prefixes,
            ),
        }
    }

    fn write_code(&mut self, matched: WriteCode) -> Result<()> {
        for (&part, width) in matched.code.parts().iter().zip(matched.widths) {
            self.enc.nbit(crate::coding::Channel::Structure, part, width)?;
        }
        Ok(())
    }

    fn find(&self, t: Terminal) -> Result<Option<WriteCode>> {
        let table = self.st.table()?;
        Ok(table.matched(t))
    }

    /// The production for a name: SE or AT with the name, then its URI,
    /// then any name.
    fn match_name(
        &self,
        uri: &str,
        local: &str,
        se: bool,
    ) -> Result<Option<(Terminal, WriteCode)>> {
        let table = self.st.table()?;
        #[cfg(feature = "std")]
        let id = self
            .st
            .schema
            .and_then(|_| self.cfg.grammars.set().name_id(uri, local));
        #[cfg(not(feature = "std"))]
        let id = None;
        if let Some(id) = id.or_else(|| self.tables.find_name(uri, local)) {
            let t = if se { Terminal::SE(id) } else { Terminal::AT(id) };
            if let Some(c) = table.matched(t) {
                return Ok(Some((t, c)));
            }
        }
        if let Some(u) = self.tables.find_uri_str(uri) {
            let t = if se { Terminal::SeUri(u) } else { Terminal::AtUri(u) };
            if let Some(c) = table.matched(t) {
                return Ok(Some((t, c)));
            }
        }
        let t = if se { Terminal::SeAny } else { Terminal::AtAny };
        Ok(table.matched(t).map(|c| (t, c)))
    }

    /// Writes the name part of an SE or AT event; returns its numbers.
    fn write_name(
        &mut self,
        t: Terminal,
        n: Name<'_>,
    ) -> Result<(NameId, Option<u32>)> {
        let prefixes = self.st.prefixes();
        let id = match t {
            Terminal::SE(id) | Terminal::AT(id) => {
                if prefixes {
                    names::write_prefix(
                        &mut self.enc,
                        &self.tables,
                        id.uri,
                        n.prefix(),
                    )?;
                }
                id
            }
            Terminal::SeUri(u) | Terminal::AtUri(u) => {
                let local = names::write_local(
                    &mut self.enc,
                    &mut self.tables,
                    u,
                    n.local_name(),
                )?;
                if prefixes {
                    names::write_prefix(
                        &mut self.enc,
                        &self.tables,
                        u,
                        n.prefix(),
                    )?;
                }
                NameId { uri: u, local }
            }
            _ => names::write_name(
                &mut self.enc,
                &mut self.tables,
                n,
                prefixes,
            )?,
        };
        let p = if prefixes {
            n.prefix().and_then(|p| {
                (0..self.tables.prefix_count(id.uri))
                    .find(|i| self.tables.prefix_str(id.uri, *i) == Some(p))
            })
        } else {
            None
        };
        Ok((id, p))
    }

    /// The added production with terminal `t` and a code of `len`
    /// parts, in the current state (non-strict informed states).
    fn find_added(&self, t: Terminal, len: u8) -> Result<Option<WriteCode>> {
        let table = self.st.table()?;
        Ok(table.matched_with_len(t, len))
    }

    /// The open element's name in Clark notation.
    fn clark(&self, id: NameId) -> String {
        alloc::format!(
            "{{{}}}{}",
            self.tables.uri_str(id.uri).unwrap_or(""),
            self.tables.local_str(id.uri, id.local).unwrap_or("")
        )
    }

    /// Counts a deviation in the open element, when counting: names
    /// matched by a wildcard or an added production; text and ends only
    /// through added productions.
    fn deviated(
        &mut self,
        kind: DeviationKind,
        code: EventCode,
        name: Option<Name<'_>>,
    ) {
        if self.deviations.is_none() || self.st.schema.is_none() {
            return;
        }
        if matches!(
            kind,
            DeviationKind::Text
                | DeviationKind::EarlyEnd
                | DeviationKind::UntypedValue
        ) && !self.st.deviates(code)
        {
            return;
        }
        // Inside declared elements only: SVG and the like are left to
        // built-in grammars.
        let Some(f) = self.st.stack.last() else {
            return;
        };
        if !matches!(f.kind, engine::FrameKind::Informed { .. }) {
            return;
        }
        let el = self.clark(f.name);
        let n = name
            .map(|n| alloc::format!("{{{}}}{}", n.uri(), n.local_name()))
            .unwrap_or_default();
        if let Some(d) = &mut self.deviations {
            d.add(kind, &el, &n);
        }
    }

    fn informed_non_strict(&self) -> bool {
        !self.cfg.options.strict()
            && matches!(
                self.st.stack.last().map(|f| f.kind),
                Some(engine::FrameKind::Informed { .. })
            )
    }

    fn handle(&mut self, ev: &Event<'_>) -> Result<()> {
        if !self.cfg.canonical {
            return self.handle_event(ev);
        }
        // Canonical EXI: text waits, so that runs merge and whitespace
        // can be judged.
        match ev {
            Event::Characters(v) => {
                self.ctext = Some(match self.ctext.take() {
                    None if matches!(v, Value::Str(_)) => {
                        CText::Text(engine::lexical_string(v)?)
                    }
                    None => CText::Value(v.to_owned()),
                    Some(CText::Text(mut s)) => {
                        s.push_str(&engine::lexical_string(v)?);
                        CText::Text(s)
                    }
                    Some(CText::Value(o)) => {
                        let mut s = engine::lexical_string(&o.as_value())?;
                        s.push_str(&engine::lexical_string(v)?);
                        CText::Text(s)
                    }
                });
                return Ok(());
            }
            // A comment or PI the options leave out is not there: text
            // around it is one.
            Event::Comment(_)
                if !self
                    .cfg
                    .options
                    .preserve()
                    .contains(Preserve::COMMENTS) =>
            {
                return Ok(());
            }
            Event::ProcessingInstruction { .. }
                if !self.cfg.options.preserve().contains(Preserve::PIS) =>
            {
                return Ok(());
            }
            _ => self.flush_text(ev)?,
        }
        match ev {
            Event::StartElement(_) => {
                let preserve_space =
                    self.canonical_elements.last_mut().is_some_and(|parent| {
                        parent.has_child = true;
                        parent.preserve_space
                    });
                self.handle_event(ev)?;
                self.canonical_elements.push(CanonicalElement {
                    has_child: false,
                    preserve_space,
                });
                Ok(())
            }
            Event::Attribute(n, v)
                if n.is("http://www.w3.org/XML/1998/namespace", "space") =>
            {
                let preserve = engine::lexical_string(v)? == "preserve";
                self.handle_event(ev)?;
                if let Some(element) = self.canonical_elements.last_mut() {
                    element.preserve_space = preserve;
                }
                Ok(())
            }
            Event::EndElement(_) => {
                self.handle_event(ev)?;
                self.canonical_elements.pop();
                Ok(())
            }
            _ => self.handle_event(ev),
        }
    }

    /// Canonical EXI: writes the waiting text before `next`, unless it
    /// is empty, or only whitespace in element content (C14N §4.3).
    fn flush_text(&mut self, next: &Event<'_>) -> Result<()> {
        let t = match self.ctext.take() {
            None => return Ok(()),
            Some(CText::Value(o)) => {
                return self.handle_event(&Event::Characters(o.as_value()));
            }
            Some(CText::Text(t)) => t,
        };
        if t.is_empty() {
            return Ok(());
        }
        let blank = t.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
        let element = self.canonical_elements.last();
        if blank && !element.is_some_and(|e| e.preserve_space) {
            let element_content =
                matches!(next, Event::StartElement(_))
                    || element.is_some_and(|e| e.has_child);
            if element_content {
                return Ok(());
            }
        }
        self.handle_event(&Event::Characters(Value::Str(&t)))
    }

    fn handle_event(&mut self, ev: &Event<'_>) -> Result<()> {
        match ev {
            Event::StartDocument => {
                if self.started {
                    return Err(state("StartDocument after other events"));
                }
                self.start()
            }
            Event::EndDocument => self.end_doc(),
            _ => {
                self.start()?;
                if self.st.top == Top::Done {
                    return Err(state("event after EndDocument"));
                }
                self.events += 1;
                self.frame_ready = false;
                self.body(ev)
            }
        }
    }

    fn start_production(
        &mut self,
        n: Name<'_>,
    ) -> Result<(Terminal, WriteCode)> {
        self.commit()?;
        if self.st.stack.is_empty() && self.st.top == Top::End {
            return Err(state("a second document element"));
        }
        let (term, code) =
            self.match_name(n.uri(), n.local_name(), true)?.ok_or_else(
                || unencodable("no production for this element here"),
            )?;
        self.st.check_depth()?;
        Ok((term, code))
    }

    /// The HTML parser has the whole tag and supplies its attributes
    /// in wire order. It can lend their values until they are written.
    #[cfg(all(feature = "html5ever", feature = "xhtml5"))]
    pub(crate) fn parsed_tag<'a>(
        &mut self,
        name: Name<'_>,
        attrs: impl Iterator<Item = (&'a str, alloc::borrow::Cow<'a, str>)>,
    ) -> Result<()> {
        let (term, code) = self.start_production(name)?;
        self.open_element(term, code, Some(name))?;
        self.unmarked = None; // the last commit's element is not this one
        self.events += 1;
        for (local, value) in attrs {
            self.write_attr(Name::local(local), &Value::Str(&value))?;
            self.events += 1;
        }
        Ok(())
    }

    fn body(&mut self, ev: &Event<'_>) -> Result<()> {
        match *ev {
            Event::StartElement(n) => {
                let (term, code) = self.start_production(n)?;
                let decl = if self.cfg.options.strict() {
                    let id = match term {
                        Terminal::SE(id) => id,
                        _ => self.tables.find_name(n.uri(), n.local_name())
                            .unwrap_or(NameId { uri: u32::MAX, local: u32::MAX }),
                    };
                    self.st.child_for(term, code.code, id)
                } else {
                    None
                };
                self.pending = Some(Pending {
                    name: (!matches!(term, Terminal::SE(_)) || self.st.prefixes()
                        || self.cfg.options.self_contained()).then(|| n.to_owned()),
                    term,
                    code,
                    decl,
                    ns: Vec::new(),
                    attrs: core::mem::take(&mut self.spare_attrs),
                });
                Ok(())
            }
            Event::Attribute(n, v) => {
                if let Some(prefix) = is_xmlns(n) {
                    let uri = engine::lexical_string(&v)?;
                    return self.body(&Event::Namespace(NsDecl {
                        prefix: &prefix,
                        uri: &uri,
                        local_element_ns: false,
                    }));
                }
                if let Some(p) = &self.pending {
                    self.check_pending_attr(p, n, &v)?;
                    let owned = (n.to_owned(), v.to_owned());
                    self.pending
                        .as_mut()
                        .expect("checked above")
                        .attrs
                        .push(owned);
                    return Ok(());
                }
                if self.in_start && !self.st.stack.is_empty() {
                    return self.write_attr(n, &v);
                }
                Err(state("attribute after content, or outside an element"))
            }
            Event::Namespace(ns) => {
                if let Some(p) = &mut self.pending {
                    p.ns.push((ns.prefix.into(), ns.uri.into()));
                    return Ok(());
                }
                if self.in_start && !self.st.stack.is_empty() {
                    self.bindings.push((
                        ns.prefix.into(),
                        ns.uri.into(),
                        self.st.depth(),
                    ));
                    if self.st.prefixes() {
                        // As in a buffered start tag: the first NS
                        // event for the element's own prefix and URI.
                        let local =
                            self.unmarked.as_ref().is_some_and(|(p, u)| {
                                p == ns.prefix && u == ns.uri
                            });
                        if local {
                            self.unmarked = None;
                        }
                        return self.write_ns(ns.prefix, ns.uri, local);
                    }
                    return Ok(());
                }
                Err(state(
                    "namespace declaration after content, or outside an element",
                ))
            }
            Event::Characters(v) => {
                self.commit()?;
                if self.st.stack.is_empty() {
                    return Err(state(
                        "character data outside the document element",
                    ));
                }
                let code = match self.find(Terminal::CH)? {
                    Some(c) => c,
                    // Rule 8: indentation in element-only content.
                    None if self.st.schema.is_some() && is_whitespace(&v) => {
                        return Ok(());
                    }
                    None => {
                        return Err(unencodable(
                            "no production for character data here",
                        ));
                    }
                };
                let slot = self.st.ch_slot(code.code);
                let (code, prepared) =
                    match engine::prepare(self.st.schema, slot, &v) {
                        Ok(p) => (code, p),
                        // Not strict: a value its type cannot hold goes
                        // as CH [untyped value].
                        Err(e)
                            if e.kind() == ErrorKind::Unencodable
                                && self.informed_non_strict() =>
                        {
                            let c =
                                self.find_added(Terminal::CH, 2)?.ok_or(e)?;
                            (c, engine::prepare(None, Slot::Str, &v)?)
                        }
                        Err(e) => return Err(e),
                    };
                let name = self.st.stack.last().expect("checked above").name;
                self.deviated(DeviationKind::Text, code.code, None);
                self.write_code(code)?;
                self.value(name, prepared, false)?;
                self.st.on_ch(code.code)?;
                self.in_start = false;
                Ok(())
            }
            Event::EndElement(n) => {
                self.commit()?;
                let f = *self.st.stack.last().ok_or_else(|| {
                    state("end_element with no element open")
                })?;
                if !(n.uri().is_empty() && n.local_name().is_empty())
                    && !n.is(
                        self.tables.uri_str(f.name.uri).unwrap_or(""),
                        self.tables
                            .local_str(f.name.uri, f.name.local)
                            .unwrap_or(""),
                    )
                {
                    return Err(state(
                        "EndElement names another element than the open one",
                    ));
                }
                let mut end = self.find(Terminal::EE)?;
                if end.is_none() && self.st.schema.is_some() {
                    if let Some(code) = self.find(Terminal::CH)? {
                        // Rule 9: supply an empty value if CH leads to
                        // EE.
                        let slot = self.st.ch_slot(code.code);
                        let prepared = engine::prepare(
                            self.st.schema,
                            slot,
                            &Value::Str(""),
                        )?;
                        if !self.st.can_end_after_characters(code.code) {
                            return Err(unencodable(
                                "the element cannot end here",
                            ));
                        }
                        self.write_code(code)?;
                        self.value(f.name, prepared, false)?;
                        self.st.on_ch(code.code)?;
                        end = self.find(Terminal::EE)?;
                    }
                }
                let code = end.ok_or_else(|| {
                    unencodable("the element cannot end here")
                })?;
                self.deviated(DeviationKind::EarlyEnd, code.code, None);
                self.write_code(code)?;
                self.st.on_ee()?;
                if self.st.stack.is_empty() && self.st.in_sc() {
                    // The self-contained element ended: its fragment's
                    // ED, then back outside.
                    let ed = self.find(Terminal::ED)?.ok_or_else(|| {
                        err(ErrorKind::Internal, "no ED production")
                    })?;
                    self.write_code(ed)?;
                    self.enc.align()?;
                    self.tables.end_self_contained()?;
                    self.st.end_sc()?;
                }
                let depth = self.st.depth();
                self.bindings.retain(|(_, _, d)| *d <= depth);
                self.in_start = false;
                if self.st.stack.is_empty()
                    && self.cfg.sequence == Sequence::Frames
                {
                    self.frame_ready = true;
                }
                Ok(())
            }
            Event::Comment(text) => {
                self.misc(Terminal::CM, Preserve::COMMENTS, &[text])
            }
            Event::ProcessingInstruction { target, data } => {
                self.misc(Terminal::PI, Preserve::PIS, &[target, data])
            }
            Event::Doctype(d) => self.misc(
                Terminal::DT,
                Preserve::DTD,
                &[d.name, d.public_id, d.system_id, d.text],
            ),
            Event::EntityReference(name) => {
                self.misc(Terminal::ER, Preserve::DTD, &[name])
            }
            Event::SelfContained => {
                if !self.cfg.options.self_contained() {
                    return Ok(());
                }
                let p = self.pending.as_ref().ok_or_else(|| {
                    state("SelfContained must follow StartElement")
                })?;
                if !p.ns.is_empty() || !p.attrs.is_empty() {
                    return Err(state(
                        "SelfContained must come before namespaces and attributes",
                    ));
                }
                let name = p.name.clone().expect("SC keeps the element name");
                self.commit()?;
                let code = self.find(Terminal::SC)?.ok_or_else(|| {
                    err(ErrorKind::Internal, "no SC production")
                })?;
                self.write_code(code)?;
                // The element again, as a fragment coded from fresh
                // tables and grammars.
                self.enc.align()?;
                self.tables.begin_self_contained();
                self.st.begin_sc();
                self.body(&Event::StartElement(name.as_name()))
            }
            Event::StartDocument | Event::EndDocument => {
                Err(err(ErrorKind::Internal, "handled before body"))
            }
        }
    }

    /// CM, PI, DT, ER: dropped when the options do not keep them.
    fn misc(
        &mut self,
        t: Terminal,
        flag: Preserve,
        texts: &[&str],
    ) -> Result<()> {
        if !self.cfg.options.preserve().contains(flag) {
            return Ok(());
        }
        self.commit()?;
        let code = self
            .find(t)?
            .ok_or_else(|| unencodable("no production for this event here"))?;
        self.write_code(code)?;
        for s in texts {
            write_string(&mut self.enc, crate::coding::Channel::Structure, s)?;
        }
        self.st.on_misc(t, code.code)?;
        if t != Terminal::NS && !self.st.stack.is_empty() {
            self.in_start = false;
        }
        Ok(())
    }

    fn end_doc(&mut self) -> Result<()> {
        if self.cfg.canonical {
            self.flush_text(&Event::EndDocument)?;
        }
        self.start()?;
        self.commit()?;
        if !self.st.stack.is_empty() {
            return Err(state("end_document while elements are open"));
        }
        if self.st.top == Top::Done {
            return Err(state("end_document twice"));
        }
        if !self.cfg.options.fragment() && self.st.top != Top::End {
            return Err(state("end_document before the document element"));
        }
        let code = self
            .find(Terminal::ED)?
            .ok_or_else(|| err(ErrorKind::Internal, "no ED production"))?;
        self.write_code(code)?;
        self.st.on_ed();
        self.enc.align()?;
        #[cfg(feature = "compression")]
        if let BodyEncoder::Block(b) = &mut self.enc {
            b.close(&mut self.tables)?;
        }
        self.enc.flush()?;
        if self.cfg.sequence == Sequence::Frames {
            self.enc.sink_mut().push_frame()?;
        }
        Ok(())
    }
}

/// All XML whitespace (space, tab, line feed, carriage return), or
/// empty.
fn is_whitespace(v: &Value<'_>) -> bool {
    matches!(v, Value::Str(s) if s.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r')))
}
