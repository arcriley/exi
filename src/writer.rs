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
//! 5. **Names** use the session's string tables
//!    ([`crate::coding::names`]), and learned productions are always
//!    used when they exist.
//! 6. **Typed values** (schema-informed): a value given as text for a
//!    typed slot is parsed into the slot's datatype; a typed value is
//!    checked against it.
//! 7. **Refused events** (D23): under `strict`, an event that fits no
//!    production — an undeclared element or attribute, a value the type
//!    cannot hold, a missing required attribute before content —
//!    returns `ErrorKind::Unencodable`. Nothing is written and nothing
//!    changes; the writer stays usable. The caller skips the event (for
//!    a start element, the whole element) and carries on.
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
//!     §9, `crate::block`). The DEFLATE level is
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
use crate::coding::datatypes::{write_event_code, write_string};
use crate::coding::{Encoder, Sink, names};
use crate::engine::{
    self, Prepared, Slot, State, Top, check_options, err, value_rules,
};
use crate::error::Position;
use crate::event::{Event, NsDecl};
use crate::grammar::informed::{ElementId, TypeId};
use crate::grammar::{EventCode, Grammars, Terminal};
use crate::header::{Header, write_header};
use crate::name::{Name, OwnedName};
use crate::reader::Sequence;
use crate::stream::EventSink;
use crate::tables::SessionTables;
use crate::value::{OwnedValue, Value};
use crate::{Coding, ErrorKind, Limits, NameId, Options, Preserve, Result};

const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// Configuration for a [`Writer`].
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
    /// Canonical EXI ([`crate::canonical`]): the same events, options
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
    pub fn utc_time(self, on: bool) -> Self {
        WriterConfig { utc_time: on, ..self }
    }
    /// The DEFLATE level, 0 (store) to 9 (smallest), for
    /// `Coding::Compression`; 6 when not set. Readers need no setting:
    /// every level decodes the same way. Above 9 is 9.
    pub fn deflate_level(self, level: u32) -> Self {
        WriterConfig { deflate_level: level.min(9), ..self }
    }
    /// Count what the schema does not declare (non-strict
    /// schema-informed streams): [`Writer::deviations`].
    pub fn count_deviations(self, on: bool) -> Self {
        WriterConfig { count_deviations: on, ..self }
    }
    pub fn header(self, h: Header) -> Self {
        WriterConfig { header: h, ..self }
    }
    pub fn sequence(self, s: Sequence) -> Self {
        WriterConfig { sequence: s, ..self }
    }
    pub fn limits(self, l: Limits) -> Self {
        WriterConfig { limits: l, ..self }
    }
}

/// What a schema did not declare by name, by kind: names matched by a
/// wildcard or by EXI's added productions (§8.5.4.4.1), and content
/// only those could encode.
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
/// needed that the schema lacks, for proposing the next schema version
/// (Appendix D.7).
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
    pub fn add(&mut self, kind: DeviationKind, element: &str, name: &str) {
        *self
            .counts
            .entry((kind, element.into(), name.into()))
            .or_insert(0) += 1;
    }
    /// Adds another set's counts.
    pub fn merge(&mut self, other: &Deviations) {
        for (k, n) in &other.counts {
            *self.counts.entry(k.clone()).or_insert(0) += n;
        }
    }
    /// Every count, most frequent first.
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
/// A refused event changes nothing (D23).
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
pub struct Writer<'g, S: Sink> {
    enc: BodyEncoder<FrameSink<S>>,
    cfg: WriterConfig<'g>,
    st: State<'g>,
    tables: SessionTables<'g>,
    /// The header has been written.
    started: bool,
    /// An explicit StartDocument, or any event, has been seen.
    begun: bool,
    poisoned: bool,
    /// A start tag not yet written: its SE was checked, its NS and AT
    /// events wait.
    pending: Option<Pending>,
    /// The innermost open element has had no content yet: attributes
    /// may still come.
    in_start: bool,
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
    cchild: Vec<bool>,
    cspace: Vec<bool>,
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
    name: OwnedName,
    term: Terminal,
    code: EventCode,
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
    pub fn new(sink: S, cfg: WriterConfig<'g>) -> Result<Self> {
        if cfg.sequence == Sequence::Frames
            && !(cfg.options.fragment()
                && cfg.options.coding() == Coding::ByteAligned)
        {
            return Err(err(
                ErrorKind::Arg,
                "frames need fragment and byte alignment",
            ));
        }
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
        let (st, tables) = Self::fresh(&cfg);
        let frames = cfg.sequence == Sequence::Frames;
        let cfg_counts = cfg.count_deviations;
        Ok(Writer {
            enc: BodyEncoder::new(
                &cfg.options,
                cfg.deflate_level,
                FrameSink { sink, frames, buf: Vec::new(), last: Vec::new() },
            ),
            cfg,
            st,
            tables,
            started: false,
            begun: false,
            poisoned: false,
            pending: None,
            in_start: false,
            bindings: Vec::new(),
            events: 0,
            frame_ready: false,
            deviations: if cfg_counts {
                Some(Deviations::default())
            } else {
                None
            },
            ctext: None,
            cchild: Vec::new(),
            cspace: Vec::new(),
        })
    }

    /// What the schema did not declare, when counted
    /// ([`WriterConfig::count_deviations`]).
    pub fn deviations(&self) -> Option<&Deviations> {
        self.deviations.as_ref()
    }
    /// ED, padding, flush.
    pub fn end_document(&mut self) -> Result<()> {
        self.guard(|w| w.end_doc())
    }
    /// Commits a buffered start tag and pushes complete octets; returns
    /// the bits still pending (always 0 for byte-aligned coding).
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
    /// Starts a new stream with the same configuration on a new sink,
    /// keeping allocations; returns the old sink. Clears a poisoned
    /// state.
    pub fn reset(&mut self, sink: S) -> S {
        let (st, tables) = Self::fresh(&self.cfg);
        let frames = self.cfg.sequence == Sequence::Frames;
        let fresh = BodyEncoder::new(
            &self.cfg.options,
            self.cfg.deflate_level,
            FrameSink { sink, frames, buf: Vec::new(), last: Vec::new() },
        );
        let old = core::mem::replace(&mut self.enc, fresh);
        // The new state and tables first: the old tables drop before
        // the new ones exist twice.
        self.tables = tables;
        self.st = st;
        self.started = false;
        self.begun = false;
        self.poisoned = false;
        self.pending = None;
        self.in_start = false;
        self.bindings.clear();
        self.events = 0;
        self.frame_ready = false;
        self.ctext = None;
        self.cchild.clear();
        self.cspace.clear();
        old.into_sink().sink
    }
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

    fn fresh(cfg: &WriterConfig<'g>) -> (State<'g>, SessionTables<'g>) {
        let st = State::new(cfg.options.clone(), cfg.grammars, cfg.limits);
        let tables = SessionTables::new(
            cfg.grammars.initial(),
            value_rules(&cfg.options),
            &cfg.limits,
        );
        (st, tables)
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
            if self.cfg.options.coding() == Coding::BitPacked {
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
        self.begun = true;
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

    fn write_code(&mut self, code: EventCode) -> Result<()> {
        let mut parts = [(0, 0); 3];
        let values = code.parts();
        let table = self.st.table()?;
        for (i, &value) in values.iter().enumerate() {
            parts[i] = (value, table.count(&values[..i]));
        }
        write_event_code(&mut self.enc, &parts[..values.len()])
    }

    fn find(&self, t: Terminal) -> Result<Option<EventCode>> {
        Ok(self.st.table()?.code(t))
    }

    /// The production for a name: SE or AT with the name, then its URI,
    /// then any name.
    fn match_name(
        &self,
        uri: &str,
        local: &str,
        se: bool,
    ) -> Result<Option<(Terminal, EventCode)>> {
        let table = self.st.table()?;
        if let Some(id) = self.tables.find_name(uri, local) {
            let t = if se { Terminal::SE(id) } else { Terminal::AT(id) };
            if let Some(c) = table.code(t) {
                return Ok(Some((t, c)));
            }
        }
        if let Some(u) = self.tables.find_uri_str(uri) {
            let t = if se { Terminal::SeUri(u) } else { Terminal::AtUri(u) };
            if let Some(c) = table.code(t) {
                return Ok(Some((t, c)));
            }
        }
        let t = if se { Terminal::SeAny } else { Terminal::AtAny };
        Ok(table.code(t).map(|c| (t, c)))
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
                    engine::write_prefix(
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
                    engine::write_prefix(
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

    fn resolve_qname(&self, text: &str) -> Option<OwnedName> {
        let text = text.trim();
        let (prefix, local) = match text.split_once(':') {
            Some((p, l)) => (p, l),
            None => ("", text),
        };
        let uri = match prefix {
            "xml" => "http://www.w3.org/XML/1998/namespace",
            _ => self
                .pending
                .iter()
                .flat_map(|p| {
                    p.ns.iter().rev().map(|(p, u)| (p.as_str(), u.as_str()))
                })
                .chain(
                    self.bindings
                        .iter()
                        .rev()
                        .map(|(p, u, _)| (p.as_str(), u.as_str())),
                )
                .find(|(p, _)| *p == prefix)
                .map(|(_, u)| u)
                .or(if prefix.is_empty() { Some("") } else { None })?,
        };
        let n = OwnedName::new(uri, local);
        Some(if prefix.is_empty() { n } else { n.with_prefix(prefix) })
    }

    /// A value for a QName slot: a QName, or text resolved with the
    /// bindings in scope.
    fn qname_value(&self, v: &Value<'_>) -> Result<OwnedValue> {
        match v {
            Value::QName(n) => Ok(OwnedValue::QName(n.to_owned())),
            _ => {
                let text = engine::lexical_string(v)?;
                self.resolve_qname(&text).map(OwnedValue::QName).ok_or_else(
                    || unencodable("xsi:type value with an undeclared prefix"),
                )
            }
        }
    }

    /// The slot and prepared value of an attribute against the current
    /// state; refuses (`Unencodable`) before anything is written.
    fn prepare_attr(
        &self,
        t: Terminal,
        code: EventCode,
        n: Name<'_>,
        v: &Value<'_>,
    ) -> Result<(Slot, Prepared, Option<TypeId>)> {
        let id = self
            .tables
            .find_name(n.uri(), n.local_name())
            .unwrap_or(NameId { uri: u32::MAX, local: u32::MAX });
        let slot = self.st.at_slot(t, code, id);
        let owned;
        let v = match slot {
            Slot::QName | Slot::TypeName => {
                owned = self.qname_value(v)?;
                owned.as_value()
            }
            _ => *v,
        };
        let prepared = engine::prepare(self.st.schema, slot, &v)?;
        let mut ty = None;
        if slot == Slot::TypeName {
            let schema = self
                .st
                .schema
                .ok_or_else(|| err(ErrorKind::Internal, "no schema"))?;
            let Prepared::QName(uri, local, _) = &prepared else {
                return Err(err(ErrorKind::Internal, "QName expected"));
            };
            let found = schema.type_named(uri, local).ok_or_else(|| {
                unencodable("xsi:type names no type of the schema")
            })?;
            let cur = match self.st.stack.last().map(|f| f.kind) {
                Some(engine::FrameKind::Informed { ty, .. }) => ty,
                _ => {
                    return Err(err(
                        ErrorKind::Internal,
                        "xsi:type outside an informed element",
                    ));
                }
            };
            if !schema.ty(cur).casts.contains(&found) {
                return Err(unencodable(
                    "xsi:type names a type that does not derive from the element's type",
                ));
            }
            ty = Some(found);
        }
        Ok((slot, prepared, ty))
    }

    /// The added production with terminal `t` and a code of `len`
    /// parts, in the current state (non-strict informed states).
    fn find_added(&self, t: Terminal, len: u8) -> Result<Option<EventCode>> {
        Ok(self.st.table()?.code_with_len(t, len))
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

    /// Writes an attribute now, in the current state.
    fn write_attr(&mut self, n: Name<'_>, v: &Value<'_>) -> Result<()> {
        let loose = self.informed_non_strict();
        let informed = matches!(
            self.st.stack.last().map(|f| f.kind),
            Some(engine::FrameKind::Informed { .. })
        );
        if informed
            && !loose
            && n.is(XSI_NS, "nil")
            && !self.st.lexical()
            && matches!(
                engine::prepare(None, Slot::Nil, v),
                Ok(Prepared::Bool(false))
            )
        {
            // As EXIficient, strict too and in Canonical EXI (whose
            // §4.4 says to keep it): xsi:nil="false" says nothing and
            // is left out.
            return Ok(());
        }
        if loose && n.is(XSI_NS, "nil") && !self.st.lexical() {
            // As EXIficient: xsi:nil="false" says nothing and is left
            // out; a value that is not a Boolean goes through AT(*)
            // [untyped value].
            match engine::prepare(None, Slot::Nil, v) {
                Ok(Prepared::Bool(false)) => return Ok(()),
                Ok(_) => {}
                Err(_) => {
                    let code = self
                        .find_added(Terminal::AtAny, 3)?
                        .ok_or_else(|| {
                            err(
                                ErrorKind::Internal,
                                "no AT(*) [untyped value] production",
                            )
                        })?;
                    return self.write_attr_as(
                        Terminal::AtAny,
                        code,
                        Slot::Str,
                        engine::prepare(None, Slot::Str, v)?,
                        None,
                        n,
                    );
                }
            }
        }
        let (t, code) =
            self.match_name(n.uri(), n.local_name(), false)?.ok_or_else(
                || unencodable("no production for this attribute here"),
            )?;
        let (t, code, slot, prepared, ty) =
            match self.prepare_attr(t, code, n, v) {
                Ok((slot, prepared, ty)) => (t, code, slot, prepared, ty),
                // Not strict: a value its type cannot hold goes
                // untyped, through the declared attribute's AT [untyped
                // value] production, or AT(*)'s.
                Err(e) if e.kind() == ErrorKind::Unencodable && loose => {
                    let (t2, code2) = match self.find_added(t, 3)? {
                        Some(c) if matches!(t, Terminal::AT(_)) => (t, c),
                        _ => (
                            Terminal::AtAny,
                            self.find_added(Terminal::AtAny, 3)?.ok_or(e)?,
                        ),
                    };
                    (
                        t2,
                        code2,
                        Slot::Str,
                        engine::prepare(None, Slot::Str, v)?,
                        None,
                    )
                }
                Err(e) => return Err(e),
            };
        self.write_attr_as(t, code, slot, prepared, ty, n)
    }

    fn write_attr_as(
        &mut self,
        t: Terminal,
        code: EventCode,
        slot: Slot,
        prepared: Prepared,
        ty: Option<TypeId>,
        n: Name<'_>,
    ) -> Result<()> {
        match (t, code.len()) {
            (Terminal::AT(_), 3) => {
                self.deviated(DeviationKind::UntypedValue, code, Some(n))
            }
            (Terminal::AtAny | Terminal::AtUri(_), _) if n.uri() != XSI_NS => {
                self.deviated(DeviationKind::Attribute, code, Some(n))
            }
            _ => {}
        }
        self.write_code(code)?;
        let (id, _) = self.write_name(t, n)?;
        let prefixes = self.st.prefixes();
        match slot {
            Slot::Nil => {
                let Prepared::Bool(b) = prepared else {
                    return Err(err(ErrorKind::Internal, "Boolean expected"));
                };
                // A typed xsi:nil decides the grammar, so it travels
                // with the structure.
                crate::coding::datatypes::write_bool(
                    &mut self.enc,
                    crate::coding::Channel::Structure,
                    b,
                )?;
                self.st.on_xsi_nil(b)
            }
            Slot::NilText => {
                let Prepared::Str(s, _) = &prepared else {
                    return Err(err(ErrorKind::Internal, "text expected"));
                };
                let b =
                    crate::lexical::parse_bool(s).map(|x| x.0).ok_or_else(
                        || err(ErrorKind::Internal, "Boolean text expected"),
                    )?;
                // Structure, as a typed xsi:nil.
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                self.st.on_xsi_nil(b)
            }
            Slot::TypeName => {
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                self.st.on_xsi_type(Some(ty.ok_or_else(|| {
                    err(ErrorKind::Internal, "type expected")
                })?))
            }
            Slot::QName
                if t == Terminal::AT(engine::XSI_TYPE)
                    && self.st.deviates(code) =>
            {
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                let named = match (&prepared, self.st.schema) {
                    (Prepared::QName(uri, local, _), Some(s)) => {
                        s.type_named(uri, local)
                    }
                    _ => None,
                };
                self.st.on_xsi_type(named)
            }
            // A lexical xsi:type is a string, but still part of the
            // structure.
            _ if id == engine::XSI_TYPE => {
                engine::emit_value(
                    &mut self.enc,
                    &mut self.tables,
                    id,
                    &prepared,
                    prefixes,
                )?;
                self.st.on_at(t, code, id)
            }
            _ => {
                self.value(id, prepared, prefixes)?;
                self.st.on_at(t, code, id)
            }
        }
    }

    /// Writes an NS event now.
    fn write_ns(
        &mut self,
        prefix: &str,
        uri: &str,
        local_element_ns: bool,
    ) -> Result<()> {
        let code = self
            .find(Terminal::NS)?
            .ok_or_else(|| state("namespace declaration after content"))?;
        self.write_code(code)?;
        names::write_ns(
            &mut self.enc,
            &mut self.tables,
            NsDecl { prefix, uri, local_element_ns },
        )?;
        self.st.on_misc(Terminal::NS, code)
    }

    /// Checks an attribute of a pending start tag against the
    /// declaration it enters.
    fn check_pending_attr(
        &self,
        p: &Pending,
        n: Name<'_>,
        v: &Value<'_>,
    ) -> Result<()> {
        let (Some(schema), Some(decl)) = (self.st.schema, p.decl) else {
            return Ok(());
        };
        if !self.cfg.options.strict() {
            return Ok(()); // every attribute can be encoded
        }
        let ed = schema.elem(decl);
        let mut td = schema.ty(ed.ty);
        if n.uri() == XSI_NS && n.local_name() == "type" && td.castable {
            return self.qname_value(v).map(|_| ());
        }
        // An xsi:type already given changes the type the other
        // attributes belong to. When it may still come, the check waits
        // for the start tag to be written.
        let cast =
            p.attrs.iter().find(|(an, _)| an.as_name().is(XSI_NS, "type"));
        match cast {
            Some((_, tv)) => {
                if let Ok(OwnedValue::QName(q)) =
                    self.qname_value(&tv.as_value())
                {
                    if let Some(t) = schema.type_named(
                        q.as_name().uri(),
                        q.as_name().local_name(),
                    ) {
                        td = schema.ty(t);
                    }
                }
            }
            None if td.castable => return Ok(()),
            None => {}
        }
        if n.uri() == XSI_NS && n.local_name() == "nil" {
            // xsi:nil="false" is left out (as EXIficient), so it fits
            // any element.
            let p = engine::prepare(None, Slot::Nil, v);
            if ed.nillable || matches!(p, Ok(Prepared::Bool(false))) {
                return p.map(|_| ());
            }
        }
        let id = self.tables.find_name(n.uri(), n.local_name());
        let uri = self.tables.find_uri_str(n.uri());
        for st in &td.states[..td.content as usize] {
            for pr in st {
                let hit = match pr.term {
                    Terminal::AT(x) => Some(x) == id,
                    Terminal::AtUri(u) => Some(u) == uri,
                    Terminal::AtAny => true,
                    _ => false,
                };
                if hit {
                    return match pr.ty {
                        Some(ty) if !self.st.lexical() => {
                            engine::prepare(Some(schema), Slot::Typed(ty), v)
                                .map(|_| ())
                        }
                        _ => Ok(()),
                    };
                }
            }
        }
        Err(unencodable("attribute not declared for this element"))
    }

    /// Writes a buffered start tag: SE, NS events, then attributes in
    /// EXI's order.
    fn commit(&mut self) -> Result<()> {
        let Some(mut p) = self.pending.take() else { return Ok(()) };
        if matches!(p.term, Terminal::SeAny | Terminal::SeUri(_)) {
            self.deviated(
                DeviationKind::Element,
                p.code,
                Some(p.name.as_name()),
            );
        }
        self.write_code(p.code)?;
        let (id, prefix) = self.write_name(p.term, p.name.as_name())?;
        self.st.on_se(p.term, p.code, id, prefix)?;
        self.in_start = true;
        let depth = self.st.depth();
        for (pre, uri) in &p.ns {
            self.bindings.push((pre.clone(), uri.clone(), depth));
        }
        if self.cfg.canonical {
            p.ns.sort_by(|a, b| a.0.cmp(&b.0));
        }
        if self.st.prefixes() {
            let el_prefix = p.name.prefix.as_deref().unwrap_or("");
            let mut marked = false;
            for (pre, uri) in core::mem::take(&mut p.ns) {
                let local = !marked && pre == el_prefix && *uri == *p.name.uri;
                marked |= local;
                self.write_ns(&pre, &uri, local)?;
            }
        }
        // Schema-informed streams sort attributes, undeclared elements'
        // too (EXIficient does; any order is valid in built-in
        // grammars).
        if self.st.schema.is_some() || self.cfg.canonical {
            let key = |n: &OwnedName| {
                let first = match (&*n.uri, &*n.local) {
                    (XSI_NS, "type") => 0,
                    (XSI_NS, "nil") => 1,
                    _ => 2,
                };
                (first, n.local.clone(), n.uri.clone())
            };
            p.attrs.sort_by_key(|(n, _)| key(n));
        }
        for (n, v) in &p.attrs {
            self.write_attr(n.as_name(), &v.as_value())?;
        }
        Ok(())
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
            Event::StartTag(_) => return self.handle_event(ev),
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
                if let Some(c) = self.cchild.last_mut() {
                    *c = true;
                }
                let space = self.cspace.last().copied().unwrap_or(false);
                self.handle_event(ev)?;
                self.cchild.push(false);
                self.cspace.push(space);
                Ok(())
            }
            Event::Attribute(n, v)
                if n.is("http://www.w3.org/XML/1998/namespace", "space") =>
            {
                let preserve = engine::lexical_string(v)? == "preserve";
                self.handle_event(ev)?;
                if let Some(s) = self.cspace.last_mut() {
                    *s = preserve;
                }
                Ok(())
            }
            Event::EndElement(_) => {
                self.handle_event(ev)?;
                self.cchild.pop();
                self.cspace.pop();
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
        if blank && !self.cspace.last().copied().unwrap_or(false) {
            let element_content =
                matches!(next, Event::StartElement(_) | Event::StartTag(_))
                    || self.cchild.last().copied().unwrap_or(false);
            if element_content {
                return Ok(());
            }
        }
        self.handle_event(&Event::Characters(Value::Str(&t)))
    }

    fn handle_event(&mut self, ev: &Event<'_>) -> Result<()> {
        match ev {
            Event::StartDocument => {
                if self.begun {
                    return Err(state("StartDocument after other events"));
                }
                self.start()
            }
            Event::EndDocument => self.end_doc(),
            Event::StartTag(tag) => {
                self.handle(&Event::StartElement(tag.name))?;
                for ns in tag.ns {
                    self.handle(&Event::Namespace(*ns))?;
                }
                for (n, v) in tag.attrs {
                    self.handle(&Event::Attribute(*n, *v))?;
                }
                Ok(())
            }
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

    fn body(&mut self, ev: &Event<'_>) -> Result<()> {
        match *ev {
            Event::StartElement(n) => {
                self.commit()?;
                if self.st.stack.is_empty() && self.st.top == Top::End {
                    return Err(state("a second document element"));
                }
                let (term, code) = self
                    .match_name(n.uri(), n.local_name(), true)?
                    .ok_or_else(|| {
                        unencodable("no production for this element here")
                    })?;
                self.st.check_depth()?;
                let id = self
                    .tables
                    .find_name(n.uri(), n.local_name())
                    .unwrap_or(NameId { uri: u32::MAX, local: u32::MAX });
                let decl = self.st.child_for(term, code, id);
                self.pending = Some(Pending {
                    name: n.to_owned(),
                    term,
                    code,
                    decl,
                    ns: Vec::new(),
                    attrs: Vec::new(),
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
                        return self.write_ns(
                            ns.prefix,
                            ns.uri,
                            ns.local_element_ns,
                        );
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
                let slot = self.st.ch_slot(code);
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
                self.deviated(DeviationKind::Text, code, None);
                self.write_code(code)?;
                self.value(name, prepared, false)?;
                self.st.on_ch(code)?;
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
                if self.find(Terminal::EE)?.is_none()
                    && self.st.schema.is_some()
                    && self.find(Terminal::CH)?.is_some()
                {
                    // Rule 9: an empty value, when the state takes CH
                    // and then allows EE.
                    let code =
                        self.find(Terminal::CH)?.expect("checked above");
                    let slot = self.st.ch_slot(code);
                    let prepared = engine::prepare(
                        self.st.schema,
                        slot,
                        &Value::Str(""),
                    )?;
                    let after = self.st.iprod(Terminal::CH).map(|p| p.next);
                    let ends =
                        match (after, self.st.stack.last().map(|f| f.kind)) {
                            (
                                Some(n),
                                Some(engine::FrameKind::Informed {
                                    ty, ..
                                }),
                            ) => self.st.schema.is_some_and(|s| {
                                s.ty(ty).states.get(n as usize).is_some_and(
                                    |st| {
                                        st.iter()
                                            .any(|p| p.term == Terminal::EE)
                                    },
                                )
                            }),
                            _ => false,
                        };
                    if !ends {
                        return Err(unencodable(
                            "the element cannot end here",
                        ));
                    }
                    self.write_code(code)?;
                    self.value(f.name, prepared, false)?;
                    self.st.on_ch(code)?;
                }
                let code = self.find(Terminal::EE)?.ok_or_else(|| {
                    unencodable("the element cannot end here")
                })?;
                self.deviated(DeviationKind::EarlyEnd, code, None);
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
                let name = p.name.clone();
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
            Event::StartDocument | Event::EndDocument | Event::StartTag(_) => {
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
        self.st.on_misc(t, code)?;
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
