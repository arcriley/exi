// src/reader.rs
//
// Pull parser for EXI Streams
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Use
//!
//! Give input with [`Reader::feed`] (or [`Reader::buffer`] and
//! [`Reader::commit`]), call [`Reader::end_input`] after the last of
//! it, and call [`Reader::next`] until it returns `Step::NeedInput`
//! (give more) or `Step::End`. Each event is returned as soon as all
//! its octets have been given; the reader never waits for the end of
//! the document. With pre-compression and compression, events come a
//! block at a time: a block's values follow its structure, so its
//! events are returned once the whole block has arrived (EXI §9).
//!
//! # The header
//!
//! The reader reads the header first ([`crate::header`]). Options in
//! the header govern; the options given with [`ReaderConfig::options`]
//! are used only when the header has none. With neither,
//! `ErrorKind::Header`.
//!
//! | Options of the stream | Result |
//! |---|---|
//! | `Coding::ByteAligned` | read |
//! | `Coding::BitPacked` | read with the feature `bit-packed` (default); else `ErrorKind::Unsupported` |
//! | `Coding::PreCompression`, `Coding::Compression` | read with the feature `compression` (default); else `ErrorKind::Unsupported` |
//! | schema-informed, not `strict`, and `selfContained` | `ErrorKind::Unsupported` |
//! | `schemaId` given but the grammars are schema-less, or `schemaId` nil but the grammars are schema-informed | `ErrorKind::Schema` |
//!
//! A `schemaId` that names a schema is not resolved: the reader trusts
//! the grammars it was given. Giving the wrong schema-informed grammars
//! does not fail; it decodes wrong data.
//!
//! # What the reader reports
//!
//! - Names carry their [`NameId`] (except names learned inside a
//!   self-contained element) and their [`GlobalName`].
//! - Prefixes: with `Preserve.prefixes`, names carry their prefixes and
//!   NS events arrive. Without it, names have no prefix; writers of XML
//!   text generate them.
//! - `EndElement` carries the element's name.
//! - Values are typed by their schema type;
//!   [`EventMeta::flags`](crate::EventMeta) says when a value is
//!   lexical.
//! - Strings in events borrow the reader until the next call to `next`.
//!
//! # Stream sequences
//!
//! | [`Sequence`] | Input | After ED |
//! |---|---|---|
//! | `Single` | one stream | `End` once input has ended |
//! | `Streams` | complete streams back to back, each with its own header | the next stream's SD, `NeedInput`, or `End` once input has ended |
//! | `Frames` | one fragment stream given one frame at a time: each frame is fed, then [`Reader::end_frame`] | (see below) |
//!
//! With `Frames` the options must set `fragment` and byte alignment, or
//! [`Reader::new`] fails with `ErrorKind::Arg`. The first frame begins
//! with the header. After each `end_frame` the reader returns that
//! frame's events, then `NeedInput`. A frame must hold exactly one
//! complete top-level element: a frame that ends inside one, or holds
//! octets after it, is `ErrorKind::Grammar`. `end_input` after a
//! complete frame ends the stream normally without an ED (a libexi
//! convention: long connections close without one).
//!
//! # Limits and errors
//!
//! Every [`Limits`] field the reader checks is checked before
//! allocating. An event larger than the ring is held whole until it is
//! complete, up to 4 × `max_value_chars` octets (or `max_binary_bytes`)
//! plus 64 KiB; a larger one, such as a DOCTYPE with several long
//! parts, is `ErrorKind::Limit`. Input that ends inside an event after
//! `end_input` is `ErrorKind::Truncated`; events already returned
//! remain valid. After any error the reader is poisoned: every later
//! call to `next` returns `ErrorKind::State`.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::coding::Decoder;
#[cfg(feature = "compression")]
use crate::coding::block::BlockRead;
use crate::coding::body::BodyDecoder;
use crate::coding::names::ValueRead;
use crate::engine::{
    self, DVal, Decoded, Probe, Slot, State, Top, TryTables, check_options, err,
};
use crate::error::Position;
use crate::event::{Doctype, Event, EventMeta, NsDecl};
use crate::grammar::{Grammars, Terminal};
use crate::header::parse_header_split;
use crate::input::InputRing;
use crate::name::{GlobalName, Name};
use crate::stream::{EventSink, EventSource, Status, Step};
use crate::strings::ValueId;
use crate::tables::SessionTables;
use crate::value::Value;
use crate::value::{OwnedValue, ValueFlags};
use crate::{Coding, ErrorKind, Limits, NameId, Options, Result};

#[cfg(feature = "compression")]
mod blocks;

/// How streams follow each other (module documentation).
///
/// ```
/// use exi::Sequence;
/// assert_eq!(Sequence::default(), Sequence::Single);
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Sequence {
    #[default]
    Single,
    Streams,
    Frames,
}

impl Sequence {
    pub(crate) fn check_options(self, opts: &Options) -> Result<()> {
        if self == Self::Frames
            && !(opts.fragment() && opts.coding() == Coding::ByteAligned)
        {
            return Err(err(
                ErrorKind::Arg,
                "frames need fragment and byte alignment",
            ));
        }
        Ok(())
    }
}

/// Configuration for a [`Reader`].
///
/// One configuration reads every coding: the header says which. Pre-compression and
/// compression streams arrive a block at a time.
///
/// ```
/// # #[cfg(all(feature = "bit-packed", feature = "compression"))] {
/// use exi::{Coding, EventSink, Options, OwnedEvent, Reader, ReaderConfig, Step, Writer, WriterConfig};
/// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let read = |cfg: ReaderConfig<'_>, bytes: &[u8]| -> exi::Result<Vec<OwnedEvent>> {
///     let mut r: Reader = Reader::new(cfg)?;
///     r.feed(bytes);
///     r.end_input();
///     let mut out = Vec::new();
///     while let Step::Event(e, _) = r.next()? { out.push(e.to_owned()); }
///     Ok(out)
/// };
/// let mut seen = Vec::new();
/// for coding in [Coding::BitPacked, Coding::ByteAligned, Coding::PreCompression, Coding::Compression] {
///     let opts = Options::builder().coding(coding).build()?;
///     let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &grammars))?;
///     w.start_element("log")?;
///     for i in 0..200 {
///         w.start_element("entry")?;
///         w.attribute("n", i.to_string().as_str())?;
///         w.text("ok")?;
///         w.end_element()?;
///     }
///     w.end_element()?;
///     w.end_document()?;
///     let events = read(ReaderConfig::new(&grammars), &w.into_sink())?;
///     assert_eq!(events.len(), 2 + 2 + 200 * 4);
///     seen.push(events);
/// }
/// assert!(seen.windows(2).all(|p| p[0] == p[1]));      // every coding, the same events
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Clone)]
pub struct ReaderConfig<'g> {
    grammars: &'g Grammars<'g>,
    options: Option<Options>,
    sequence: Sequence,
    limits: Limits,
}

impl<'g> ReaderConfig<'g> {
    /// Reads streams informed by `grammars`, with the header's options,
    /// as one stream, in normal mode, with default limits.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// // <a/> with its options in the header: byte-aligned.
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
    /// r.feed(&[0xA0, 0x00, 0x4A, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// let mut n = 0;
    /// while let Step::Event(..) = r.next()? { n += 1; }
    /// assert_eq!(n, 4);                                        // SD SE EE ED
    /// assert_eq!(r.options().unwrap().coding(), Coding::ByteAligned);
    ///
    /// // A bare header, and no options given: nothing says how to read the body.
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
    /// r.feed(&[0x80, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::Header);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(grammars: &'g Grammars<'g>) -> Self {
        ReaderConfig {
            grammars,
            options: None,
            sequence: Sequence::Single,
            limits: Limits::new(),
        }
    }
    /// Options agreed out of band, used only when the header carries
    /// none.
    ///
    /// ```
    /// use exi::{Coding, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let bits = Options::builder().coding(Coding::BitPacked).build()?;
    /// // A bare header: the given options are used.
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
    /// r.feed(&[0x80, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// r.next()?;
    /// assert_eq!(r.options().unwrap().coding(), Coding::ByteAligned);
    /// // Options in the header govern over the given ones.
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(bits))?;
    /// r.feed(&[0xA0, 0x00, 0x4A, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// r.next()?;
    /// assert_eq!(r.options().unwrap().coding(), Coding::ByteAligned);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn options(self, o: Options) -> Self {
        ReaderConfig { options: Some(o), ..self }
    }
    /// How streams follow each other: [`Sequence`].
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Event, Options, Reader, ReaderConfig, Sequence, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// // Two <a/> streams back to back, each with its own header.
    /// let cfg = ReaderConfig::new(&grammars).options(byte.clone()).sequence(Sequence::Streams);
    /// let mut r: Reader = Reader::new(cfg)?;
    /// r.feed(&[0x80, 0x01, 0x02, b'a', 0x00, 0x80, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// let mut documents = 0;
    /// loop {
    ///     match r.next()? {
    ///         Step::Event(Event::EndDocument, _) => documents += 1,
    ///         Step::Event(..) => {}
    ///         _ => break,
    ///     }
    /// }
    /// assert_eq!(documents, 2);
    /// // Frames are fragments, byte-aligned.
    /// let cfg = ReaderConfig::new(&grammars).options(byte).sequence(Sequence::Frames);
    /// assert_eq!(Reader::<64>::new(cfg).err().unwrap().kind(), ErrorKind::Arg);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn sequence(self, s: Sequence) -> Self {
        ReaderConfig { sequence: s, ..self }
    }
    /// The [`Limits`] input is checked against.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Limits, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let read = |limits: Limits| -> exi::Result<()> {
    ///     let cfg = ReaderConfig::new(&grammars).options(byte.clone()).limits(limits);
    ///     let mut r: Reader = Reader::new(cfg)?;
    ///     // <r><a/></r>: depth 2.
    ///     r.feed(&[0x80, 0x01, 0x02, b'r', 0x02, 0x01, 0x02, b'a', 0x00, 0x00]);
    ///     r.end_input();
    ///     while let Step::Event(..) = r.next()? {}
    ///     Ok(())
    /// };
    /// read(Limits::new().max_depth(2))?;
    /// assert_eq!(read(Limits::new().max_depth(1)).unwrap_err().kind(), ErrorKind::Limit);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn limits(self, l: Limits) -> Self {
        ReaderConfig { limits: l, ..self }
    }
}

/// A reader with an input ring of `N` octets (a power of two, at least
/// 64).
///
/// Setup used by the examples: global tables, schema-less grammars, and
/// options agreed out of band (byte-aligned), so each stream starts
/// with the bare header `0x80`.
///
/// ```
/// use exi::{Reader, ReaderConfig, Step, OwnedEvent, OwnedName, OwnedValue, Options, Coding};
/// use exi::grammar::GrammarSet;
/// use exi::strings::{GlobalStrings, GlobalConfig};
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let set = GrammarSet::schemaless();
/// let grammars = set.attach(&strings)?;
/// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
/// // <a>hi</a>: SE(*) with URI 1 ("" hit), local miss "a"; CH as 0.3; "hi" literal; EE 0.
/// r.feed(&[0x80, 0x01, 0x02, b'a', 0x03, 0x04, b'h', b'i', 0x00]);
/// r.end_input();
/// let mut events = Vec::new();
/// loop {
///     match r.next()? {
///         Step::Event(e, _) => events.push(e.to_owned()),
///         Step::NeedInput => unreachable!("all input was given"),
///         Step::End => break,
///     }
/// }
/// let a = OwnedName::new("", "a");
/// assert_eq!(events, [
///     OwnedEvent::StartDocument,
///     OwnedEvent::StartElement(a.clone()),
///     OwnedEvent::Characters(OwnedValue::Str("hi".into())),
///     OwnedEvent::EndElement(a),
///     OwnedEvent::EndDocument,
/// ]);
/// assert!(matches!(r.next()?, Step::End));          // End again: nothing more
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Learned grammars: `<r><a/><a/></r>`. The second `a` is a hit in the
/// local-name table and uses the production `r` learned for it; `a`
/// learned its EE.
///
/// ```
/// # use exi::{Reader, ReaderConfig, Step, Options, Coding, Event};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// # let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
/// r.feed(&[0x80, 0x01, 0x02, b'r', 0x02, 0x01, 0x02, b'a', 0x00,
///          0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01]);
/// r.end_input();
/// let mut names = Vec::new();
/// while let Step::Event(e, meta) = r.next()? {
///     if let Event::StartElement(n) = e { names.push((n.local_name().to_owned(), n.id(), meta.depth)); }
/// }
/// assert_eq!(names, [
///     ("r".into(), Some(exi::NameId { uri: 0, local: 0 }), 1),
///     ("a".into(), Some(exi::NameId { uri: 0, local: 1 }), 2),
///     ("a".into(), Some(exi::NameId { uri: 0, local: 1 }), 2),
/// ]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Input in pieces: every split point gives the same events, with
/// `NeedInput` between.
///
/// ```
/// # use exi::{Reader, ReaderConfig, Step, Options, Coding};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// # let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// let stream = [0x80, 0x01, 0x02, b'a', 0x03, 0x04, b'h', b'i', 0x00];
/// let read = |chunks: &[&[u8]]| -> exi::Result<Vec<exi::OwnedEvent>> {
///     let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte.clone()))?;
///     let mut out = Vec::new();
///     for (i, c) in chunks.iter().enumerate() {
///         assert_eq!(r.feed(c), c.len());
///         if i + 1 == chunks.len() { r.end_input(); }
///         loop {
///             match r.next()? {
///                 Step::Event(e, _) => out.push(e.to_owned()),
///                 Step::NeedInput | Step::End => break,
///             }
///         }
///     }
///     Ok(out)
/// };
/// let whole = read(&[&stream])?;
/// for cut in 1..stream.len() {
///     assert_eq!(read(&[&stream[..cut], &stream[cut..]])?, whole, "cut at {cut}");
/// }
/// let bytes: Vec<&[u8]> = stream.iter().map(core::slice::from_ref).collect();
/// assert_eq!(read(&bytes)?, whole);                       // one octet at a time
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Errors: truncated input, an out-of-range value, poisoning, missing
/// options, and a limit.
///
/// ```
/// # use exi::{Reader, ReaderConfig, Step, Options, Coding, ErrorKind, Limits};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// # let byte = Options::builder().coding(Coding::ByteAligned).build()?;
/// let run = |cfg: ReaderConfig<'_>, input: &[u8]| -> exi::Result<()> {
///     let mut r: Reader = Reader::new(cfg)?;
///     r.feed(input);
///     r.end_input();
///     loop { if let Step::End = r.next()? { return Ok(()); } }
/// };
/// let cfg = || ReaderConfig::new(&grammars).options(byte.clone());
/// assert_eq!(run(cfg(), &[0x80, 0x01, 0x02, b'a']).unwrap_err().kind(), ErrorKind::Truncated);
/// assert_eq!(run(cfg(), &[0x80, 0x04]).unwrap_err().kind(), ErrorKind::Grammar);  // URI 4 of 2 bits
/// assert_eq!(run(ReaderConfig::new(&grammars), &[0x80]).unwrap_err().kind(), ErrorKind::Header);
/// let shallow = cfg().limits(Limits::new().max_depth(1));
/// // <r><a/></r> needs depth 2.
/// let nested = [0x80, 0x01, 0x02, b'r', 0x02, 0x01, 0x02, b'a', 0x00, 0x00];
/// assert_eq!(run(shallow, &nested).unwrap_err().kind(), ErrorKind::Limit);
///
/// let mut r: Reader = Reader::new(cfg())?;
/// r.feed(&[0x80, 0x04]);
/// r.end_input();
/// let _ = r.next();                                        // SD
/// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::Grammar);
/// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::State);   // poisoned
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Options in the header govern; the stream below says byte-aligned in
/// its own header.
///
/// ```
/// # use exi::{Reader, ReaderConfig, Step, Coding};
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings)?;
/// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
/// r.feed(&[0xA0, 0x00, 0x4A, 0x01, 0x02, b'a', 0x00]);
/// r.end_input();
/// assert!(r.options().is_none());                          // the header is not read yet
/// assert!(matches!(r.next()?, Step::Event(exi::Event::StartDocument, _)));
/// assert_eq!(r.options().unwrap().coding(), Coding::ByteAligned);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// A ring size that is not a power of two does not compile:
///
/// ```compile_fail
/// # use exi::grammar::GrammarSet;
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
/// # let set = GrammarSet::schemaless();
/// # let grammars = set.attach(&strings).unwrap();
/// let r: exi::Reader<'_, 100> = exi::Reader::new(exi::ReaderConfig::new(&grammars)).unwrap();
/// ```
pub struct Reader<'g, const N: usize = 4096> {
    ring: InputRing<N>,
    cfg: ReaderConfig<'g>,
    /// Input that did not fit the ring while one event was incomplete:
    /// decoding reads from here until it is used up, and new input is
    /// added here meanwhile.
    spill: Spill,
    spilling: bool,
    ended: bool,
    poisoned: bool,
    phase: Phase,
    sess: Option<Session<'g>>,
    dec: Decoded,
    out: Out,
    /// Octets consumed and fed since the reader started (frame ends use
    /// the same count).
    pos: u64,
    /// Bits already read of the first unconsumed octet: 0 unless the
    /// body is bit-packed.
    bit: u8,
    fed: u64,
    frame_ends: VecDeque<u64>,
    events: u64,
    depth_out: u32,
    skip_to: Option<u32>,
    /// Decoded events not yet returned.
    queue: VecDeque<(Out, EventMeta)>,
    /// Literal strings referenced by queued `OutVal::Lit` events, cleared
    /// whenever `queue` empties.
    str_buf: String,
    /// Pre-compression and compression: the block being read, and how
    /// many events at the front of the queue belong to blocks whose
    /// values are all read.
    #[cfg(feature = "compression")]
    block: Option<BlockRead>,
    #[cfg(feature = "compression")]
    released: usize,
    /// An SE is waiting for its NS events; it is at `collect_at` in the
    /// queue.
    collecting: bool,
    collect_at: usize,
    /// SC started while `collecting` still held the enclosing SE (and
    /// any NS events) in `queue`; resolve those from the saved outer
    /// partition until `Out::SC` is popped.
    sc_collect: bool,
    /// Self-contained elements: the next SE repeats the element (not
    /// returned); the next ED ends its fragment (not returned); the
    /// event just decoded is not returned.
    sc_se: bool,
    sc_ed: bool,
    suppress: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Header,
    Body,
    /// After ED of the last stream.
    End,
}

struct Session<'g> {
    st: State<'g>,
    tables: SessionTables<'g>,
}

/// A value carried by `Out`: string-table hits and single-event literals
/// stay zero-copy without allocating a `Box<str>`.
#[derive(Debug, Default)]
pub(super) enum OutVal {
    #[default]
    None,
    /// A string value held in `SessionTables` / `GlobalStrings::values()`.
    Hit(ValueId),
    /// A string literal held in `Reader::dec.text` (non-queued fast path).
    DecText,
    /// A string literal copied into `Reader::str_buf` (queued path).
    Lit(usize, usize),
    /// A non-string typed value or QName.
    Typed(OwnedValue),
}

impl OutVal {
    fn as_value<'a>(
        &'a self,
        t: &'a SessionTables<'_>,
        dec_text: &'a str,
        str_buf: &'a str,
    ) -> Value<'a> {
        match self {
            OutVal::None => Value::Str(""),
            OutVal::Hit(id) => Value::Str(t.global_value_text(*id)),
            OutVal::DecText => Value::Str(dec_text),
            OutVal::Lit(a, b) => Value::Str(&str_buf[*a..*b]),
            OutVal::Typed(v) => v.as_value(),
        }
    }
}

/// The event `next` returns, in numbers and owned data, until the next
/// call.
#[derive(Debug, Default)]
enum Out {
    #[default]
    None,
    SD,
    ED,
    SE(NameId, Option<u32>),
    EE(NameId, Option<u32>),
    AT(NameId, Option<u32>, OutVal),
    CH(OutVal),
    NS(u32, u32, bool),
    CM(String),
    PI(String, String),
    DT(String, String, String, String),
    ER(String),
    SC,
}

/// Input that did not fit the ring: a buffer read from `pos`. Taking
/// octets from the front moves `pos`; the buffer is compacted only when
/// most of it has been read, so reading a long spilled input stays
/// linear.
#[derive(Default)]
struct Spill {
    buf: Vec<u8>,
    pos: usize,
}

impl Spill {
    fn data(&self) -> &[u8] {
        &self.buf[self.pos..]
    }
    fn len(&self) -> usize {
        self.buf.len() - self.pos
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn extend(&mut self, d: &[u8]) {
        self.buf.extend_from_slice(d);
    }
    fn consume(&mut self, n: usize) {
        self.pos += n;
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos >= 1 << 16 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }
    fn clear(&mut self) {
        self.buf.clear();
        self.pos = 0;
    }
}

/// The octets a decode sees: `n` octets of the input (the ring, or the
/// spill buffer), or with compression the unread part of the inflated
/// stream `z`.
fn input_view<'a, const N: usize>(
    ring: &'a InputRing<N>,
    spill: &'a [u8],
    spilling: bool,
    z: Option<&'a [u8]>,
    n: usize,
) -> (&'a [u8], &'a [u8]) {
    if let Some(z) = z {
        return (z, &[]);
    }
    let (a, b) = if spilling { (spill, &[][..]) } else { ring.filled() };
    if n <= a.len() { (&a[..n], &[]) } else { (a, &b[..n - a.len()]) }
}

/// A decoded value as the event carries it, avoiding `Box<str>` allocations
/// for string-table hits and single-event literals.
pub(super) fn out_val(
    t: &SessionTables<'_>,
    v: DVal,
    text: &str,
    str_buf: &mut String,
    can_borrow_dec: bool,
    can_pin_hit: bool,
) -> OutVal {
    match v {
        DVal::Str(r) => match r {
            ValueRead::Hit(id) | ValueRead::Literal(Some(id))
                if can_pin_hit =>
            {
                OutVal::Hit(id)
            }
            ValueRead::Hit(id) => {
                let s = t.global_value_text(id);
                let a = str_buf.len();
                str_buf.push_str(s);
                OutVal::Lit(a, str_buf.len())
            }
            ValueRead::Literal(_) => {
                if can_borrow_dec {
                    OutVal::DecText
                } else {
                    let a = str_buf.len();
                    str_buf.push_str(text);
                    OutVal::Lit(a, str_buf.len())
                }
            }
        },
        DVal::QName(id, p) => {
            OutVal::Typed(OwnedValue::QName(engine::owned_name(t, id, p)))
        }
        DVal::Nil(b) => OutVal::Typed(OwnedValue::Bool(b)),
        DVal::Typed(v) => OutVal::Typed(v),
        // A value still in its channel.
        DVal::None => OutVal::None,
    }
}

fn name<'a>(
    t: &'a SessionTables<'_>,
    outer: bool,
    id: NameId,
    prefix: Option<u32>,
) -> Name<'a> {
    let (uid, u, lid, l) = t.resolve_name_at(outer, id);
    let n = Name::new(u, l);
    // The empty prefix is the default namespace: no prefix.
    let n = match prefix.and_then(|p| t.prefix_str_at(outer, id.uri, p)) {
        Some(p) if !p.is_empty() => n.with_prefix(p),
        _ => n,
    };
    match (uid, lid) {
        (Some(uri), Some(local)) => {
            n.with_ids(Some(id), Some(GlobalName { uri, local }))
                .with_pool(t.global_ref())
        }
        _ => n.with_ids(Some(id), None),
    }
}

impl<'g, const N: usize> Reader<'g, N> {
    /// # Errors
    ///
    /// `ErrorKind::Arg` when the configuration is inconsistent:
    /// `Frames` with out-of-band options that do not set `fragment` and
    /// byte alignment.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Options, Reader, ReaderConfig, Sequence};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let small = Reader::<64>::new(ReaderConfig::new(&grammars))?;   // a 64-octet ring
    /// let usual: Reader = Reader::new(ReaderConfig::new(&grammars))?; // 4096 octets
    /// assert_eq!((small.depth(), usual.depth()), (0, 0));
    ///
    /// let frag = Options::builder().coding(Coding::ByteAligned).fragment(true).build()?;
    /// let frames = ReaderConfig::new(&grammars).sequence(Sequence::Frames);
    /// assert!(Reader::<64>::new(frames.clone().options(frag)).is_ok());
    /// let bits = Options::builder().fragment(true).build()?;              // bit-packed
    /// assert_eq!(Reader::<64>::new(frames.options(bits)).err().unwrap().kind(), ErrorKind::Arg);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(cfg: ReaderConfig<'g>) -> Result<Self> {
        let () = InputRing::<N>::VALID;
        if let Some(opts) = &cfg.options {
            cfg.sequence.check_options(opts)?;
        }
        Ok(Reader {
            ring: InputRing::new(),
            cfg,
            spill: Spill::default(),
            spilling: false,
            ended: false,
            poisoned: false,
            phase: Phase::Header,
            sess: None,
            dec: Decoded::default(),
            out: Out::None,
            pos: 0,
            bit: 0,
            fed: 0,
            frame_ends: VecDeque::new(),
            events: 0,
            depth_out: 0,
            skip_to: None,
            queue: VecDeque::new(),
            str_buf: String::new(),
            #[cfg(feature = "compression")]
            block: None,
            #[cfg(feature = "compression")]
            released: 0,
            collecting: false,
            collect_at: 0,
            sc_collect: false,
            sc_se: false,
            sc_ed: false,
            suppress: false,
        })
    }
    /// Copies as much of `data` as fits into the ring and returns how
    /// much. The caller's buffer is free as soon as this returns; feed
    /// the rest after calling `next`.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, OwnedEvent, OwnedValue, Reader, ReaderConfig, Step, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let text = "x".repeat(300);
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(byte.clone(), &grammars).header(Header::Bare { cookie: false }))?;
    /// w.start_element("a")?;
    /// w.text(text.as_str())?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// let stream = w.into_sink();
    ///
    /// let mut r = Reader::<64>::new(ReaderConfig::new(&grammars).options(byte))?;
    /// let mut rest = &stream[..];
    /// let n = r.feed(rest);
    /// assert_eq!(n, 64);                                       // only the ring's worth
    /// rest = &rest[n..];
    /// let mut events = Vec::new();
    /// loop {
    ///     rest = &rest[r.feed(rest)..];
    ///     if rest.is_empty() { r.end_input(); }
    ///     match r.next()? {
    ///         Step::Event(e, _) => events.push(e.to_owned()),
    ///         Step::NeedInput => {}
    ///         Step::End => break,
    ///     }
    /// }
    /// assert_eq!(events[2], OwnedEvent::Characters(OwnedValue::Str(text.into())));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn feed(&mut self, data: &[u8]) -> usize {
        let n = if self.spilling {
            self.spill.extend(data);
            data.len()
        } else {
            self.ring.feed(data)
        };
        self.fed += n as u64;
        n
    }
    /// Contiguous free input space to read into directly. Call
    /// [`Self::commit`] with the number of bytes read.
    ///
    /// ```
    /// use exi::{Coding, Event, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let options = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut reader = Reader::<64>::new(ReaderConfig::new(&grammars).options(options.clone()))?;
    /// assert!(reader.options().is_none());
    /// let document = [0x80, 0x01, 0x02, b'a', 0x00];
    /// reader.buffer()[..document.len()].copy_from_slice(&document);
    /// reader.commit(document.len());
    /// reader.end_input();
    /// assert!(matches!(reader.next()?, Step::Event(Event::StartDocument, _)));
    /// assert_eq!(reader.options(), Some(&options));
    /// assert_eq!(reader.position().byte, 1);
    /// assert!(matches!(reader.next()?, Step::Event(Event::StartElement(_), _)));
    /// assert_eq!(reader.depth(), 1);
    /// assert!(matches!(reader.next()?, Step::Event(Event::EndElement(_), _)));
    /// assert_eq!(reader.depth(), 0);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn buffer(&mut self) -> &mut [u8] {
        self.ring.buffer()
    }
    /// Makes `n` octets written into `buffer()` visible to `next`.
    ///
    /// ```
    /// use exi::{Coding, Event, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut r = Reader::<64>::new(ReaderConfig::new(&grammars).options(byte))?;
    /// // <a/> in two pieces. Octets written but not committed are not read.
    /// r.buffer()[0] = 0x80;
    /// assert!(matches!(r.next()?, Step::NeedInput));
    /// r.commit(1);
    /// assert!(matches!(r.next()?, Step::Event(Event::StartDocument, _)));
    /// r.buffer()[..4].copy_from_slice(&[0x01, 0x02, b'a', 0x00]);
    /// r.commit(4);
    /// r.end_input();
    /// assert!(matches!(r.next()?, Step::Event(Event::StartElement(n), _) if n.local_name() == "a"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn commit(&mut self, n: usize) {
        self.ring.commit(n);
        self.fed += n as u64;
        if self.spilling {
            let (a, b) = self.ring.filled();
            self.spill.extend(a);
            self.spill.extend(b);
            self.ring.clear();
        }
    }
    /// `Sequence::Frames`: the octets fed since the last call complete
    /// one frame.
    ///
    /// ```
    /// use exi::{Coding, Event, EventSink, Header, Options, Reader, ReaderConfig, Sequence, Step, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let frag = Options::builder().coding(Coding::ByteAligned).fragment(true).build()?;
    /// let cfg = WriterConfig::new(frag.clone(), &grammars)
    ///     .header(Header::Bare { cookie: false })
    ///     .sequence(Sequence::Frames);
    /// let mut w = Writer::new(Vec::new(), cfg)?;
    /// let mut frames = Vec::new();
    /// for body in ["one", "two"] {
    ///     w.start_element("m")?;
    ///     w.text(body)?;
    ///     w.end_element()?;
    ///     frames.push(w.end_frame()?.to_vec());
    /// }
    ///
    /// let cfg = ReaderConfig::new(&grammars).options(frag).sequence(Sequence::Frames);
    /// let mut r: Reader = Reader::new(cfg)?;
    /// let mut texts = Vec::new();
    /// for f in &frames {
    ///     r.feed(f);
    ///     r.end_frame();
    ///     while let Step::Event(e, _) = r.next()? {           // to NeedInput
    ///         if let Event::Characters(v) = e { texts.push(v.as_str().unwrap().to_owned()); }
    ///     }
    /// }
    /// r.end_input();
    /// assert!(matches!(r.next()?, Step::End));                 // no ED: a libexi convention
    /// assert_eq!(texts, ["one", "two"]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_frame(&mut self) {
        self.frame_ends.push_back(self.fed);
    }
    /// No more input will come.
    ///
    /// ```
    /// use exi::{Coding, Event, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
    /// r.feed(&[0x80, 0x01, 0x02, b'a', 0x00]);
    /// while let Step::Event(..) = r.next()? {}
    /// // After ED, `End` waits for the end of input.
    /// assert!(matches!(r.next()?, Step::NeedInput));
    /// r.end_input();
    /// assert!(matches!(r.next()?, Step::End));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_input(&mut self) {
        self.ended = true;
    }
    /// The next event, `NeedInput`, or `End`.
    ///
    /// # Errors
    ///
    /// As the module documentation describes. After an error,
    /// `ErrorKind::State`.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Event, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
    /// r.feed(&[0x80, 0x01, 0x02]);                             // <a> cut inside its name
    /// assert!(matches!(r.next()?, Step::Event(Event::StartDocument, _)));
    /// assert!(matches!(r.next()?, Step::NeedInput));           // more may come
    /// r.end_input();
    /// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::Truncated);  // it will not
    /// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::State);
    /// # Ok::<(), exi::Error>(())
    /// ```
    #[allow(clippy::should_implement_trait)] // events borrow the reader: not an Iterator
    pub fn next(&mut self) -> Result<Step<'_>> {
        self.skip_to = None;
        self.step()
    }
    /// Feeds all of `data`, calls [`end_input`](Self::end_input), and
    /// sends every event to `sink` until `Step::End`.
    ///
    /// Handles inputs larger than the ring `N` by draining events as
    /// octets are fed. When `data` fits in the ring, `end_input` is
    /// marked before decoding begins.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, OwnedEvent, Reader, ReaderConfig, Writer, WriterConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let text = "x".repeat(200);
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(byte.clone(), &grammars).header(Header::Bare { cookie: false }))?;
    /// w.start_element("a")?;
    /// w.text(text.as_str())?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// let stream = w.into_sink();
    ///
    /// let mut r = Reader::<64>::new(ReaderConfig::new(&grammars).options(byte))?;
    /// let mut events: Vec<OwnedEvent> = Vec::new();
    /// r.decode_all(&stream, &mut events)?;
    /// assert_eq!(events.len(), 5);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn decode_all<K: EventSink + ?Sized>(
        &mut self,
        data: &[u8],
        sink: &mut K,
    ) -> Result<()> {
        let mut rest = data;
        while !rest.is_empty() {
            let n = self.feed(rest);
            rest = &rest[n..];
            if rest.is_empty() {
                break;
            }
            loop {
                match self.next()? {
                    Step::Event(ev, _) => sink.event(&ev)?,
                    Step::NeedInput => break,
                    Step::End => return Ok(()),
                }
            }
            if n == 0 {
                let m = self.feed(rest);
                if m == 0 {
                    break;
                }
                rest = &rest[m..];
            }
        }
        self.end_input();
        loop {
            match self.next()? {
                Step::Event(ev, _) => sink.event(&ev)?,
                Step::NeedInput => {
                    return Err(err(
                        ErrorKind::Internal,
                        "reader asked for input after end_input",
                    ));
                }
                Step::End => return Ok(()),
            }
        }
    }
    /// The stream's options, once the header has been read.
    ///
    /// ```
    /// use exi::{Coding, Options, Reader, ReaderConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
    /// // Options in the header: byte-aligned.
    /// r.feed(&[0xA0, 0x00, 0x4A, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// assert_eq!(r.options(), None);
    /// r.next()?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// assert_eq!(r.options(), Some(&byte));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn options(&self) -> Option<&Options> {
        self.sess.as_ref().map(|s| &s.st.opts)
    }
    /// Where the next event starts.
    ///
    /// ```
    /// use exi::{Coding, Options, Reader, ReaderConfig};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
    /// r.feed(&[0x80, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// assert_eq!((r.position().byte, r.position().event), (0, 0));
    /// r.next()?;                                               // SD, after the header
    /// assert_eq!((r.position().byte, r.position().event), (1, 1));
    /// r.next()?;                                               // SE(a), 3 octets
    /// assert_eq!((r.position().byte, r.position().event), (4, 2));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn position(&self) -> Position {
        self.position_now()
    }
    /// Open elements.
    ///
    /// ```
    /// use exi::{Coding, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
    /// // <r><a/></r>
    /// r.feed(&[0x80, 0x01, 0x02, b'r', 0x02, 0x01, 0x02, b'a', 0x00, 0x00]);
    /// r.end_input();
    /// let mut depths = Vec::new();
    /// while let Step::Event(..) = r.next()? { depths.push(r.depth()); }
    /// assert_eq!(depths, [0, 1, 2, 1, 0, 0]);                 // SD SE SE EE EE ED
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn depth(&self) -> u32 {
        self.depth_out
    }
    /// Decodes the rest of the current element without returning its
    /// events. Values are still decoded, to keep the string tables in
    /// step, so this saves conversion, not decoding.
    ///
    /// Returns `Status::End` once the element's EE has been read (or
    /// the stream ended), and `Status::NeedInput` when input runs out
    /// first: give more and call it again. Calling `next` instead gives
    /// up the skip.
    ///
    /// ```
    /// use exi::{Coding, Event, Options, Reader, ReaderConfig, Status, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let options = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut reader = Reader::<64>::new(ReaderConfig::new(&grammars).options(options))?;
    /// reader.feed(&[0x80, 0x01, 0x02, b'a']);
    /// assert!(matches!(reader.next()?, Step::Event(Event::StartDocument, _)));
    /// assert!(matches!(reader.next()?, Step::Event(Event::StartElement(_), _)));
    /// assert_eq!(reader.skip_element()?, Status::NeedInput);
    /// reader.feed(&[0x00]);
    /// reader.end_input();
    /// assert_eq!(reader.skip_element()?, Status::End);
    /// assert!(matches!(reader.next()?, Step::Event(Event::EndDocument, _)));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn skip_element(&mut self) -> Result<Status> {
        // Returns `End` once the element is skipped (or the stream
        // ended), `NeedInput` to be called again after more input.
        let target = match self.skip_to {
            Some(t) => t,
            None => {
                let d = self.depth();
                if d == 0 {
                    return Err(err(
                        ErrorKind::State,
                        "skip_element outside an element",
                    ));
                }
                self.skip_to = Some(d - 1);
                d - 1
            }
        };
        loop {
            match self.step()? {
                Step::Event(..) => {
                    if self.depth() <= target
                        && matches!(self.out, Out::EE(..))
                    {
                        self.skip_to = None;
                        return Ok(Status::End);
                    }
                }
                Step::NeedInput => return Ok(Status::NeedInput),
                Step::End => {
                    self.skip_to = None;
                    return Ok(Status::End);
                }
            }
        }
    }
    /// Starts a new stream with the same configuration, keeping
    /// allocations: for servers that read many short streams. Clears a
    /// poisoned state.
    ///
    /// ```
    /// use exi::{Coding, ErrorKind, Options, Reader, ReaderConfig, Step};
    /// # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// # let set = GrammarSet::schemaless();
    /// # let grammars = set.attach(&strings)?;
    /// let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    /// let mut r: Reader = Reader::new(ReaderConfig::new(&grammars).options(byte))?;
    /// r.feed(&[0x80, 0x04]);                                   // a URI that does not exist
    /// r.end_input();
    /// r.next()?;
    /// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::Grammar);
    /// assert_eq!(r.next().unwrap_err().kind(), ErrorKind::State);
    /// r.reset();
    /// r.feed(&[0x80, 0x01, 0x02, b'a', 0x00]);
    /// r.end_input();
    /// let mut n = 0;
    /// while let Step::Event(..) = r.next()? { n += 1; }
    /// assert_eq!(n, 4);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.ring.clear();
        self.spill.clear();
        self.spilling = false;
        self.ended = false;
        self.poisoned = false;
        self.phase = Phase::Header;
        self.sess = None;
        self.dec.clear();
        self.out = Out::None;
        self.pos = 0;
        self.bit = 0;
        self.fed = 0;
        self.frame_ends.clear();
        self.events = 0;
        self.depth_out = 0;
        self.skip_to = None;
        self.queue.clear();
        self.str_buf.clear();
        #[cfg(feature = "compression")]
        {
            self.block = None;
            self.released = 0;
        }
        self.collecting = false;
        self.sc_collect = false;
        self.collect_at = 0;
        self.sc_se = false;
        self.sc_ed = false;
        self.suppress = false;
    }
    /// Starts a new stream with `grammars`, keeping the rest of the
    /// configuration and allocations. Clears a poisoned state.
    ///
    /// ```
    /// use exi::{Coding, EventSink, Header, Options, OwnedEvent, OwnedName, Reader, ReaderConfig, Writer, WriterConfig};
    /// use exi::grammar::{GrammarSet, informed::Prod};
    /// # use exi::{strings::{GlobalConfig, GlobalStrings}};
    /// # let strings = GlobalStrings::new(GlobalConfig::new())?;
    /// let schemaless = GrammarSet::schemaless();
    /// let mut b = GrammarSet::builder();
    /// let empty = b.complex_type(None, vec![vec![Prod::EE]]);
    /// let ping = b.element(OwnedName::new("", "ping"), empty, false);
    /// b.global(ping);
    /// let informed = b.build()?;
    /// let (g_less, g_inf) = (schemaless.attach(&strings)?, informed.attach(&strings)?);
    ///
    /// let strict = Options::builder().coding(Coding::ByteAligned).strict(true).build()?;
    /// let mut w = Writer::new(Vec::new(), WriterConfig::new(strict, &g_inf).header(Header::WithOptions { cookie: false }))?;
    /// w.start_element("ping")?;
    /// w.end_element()?;
    /// w.end_document()?;
    /// let strict_bytes = w.into_sink();
    ///
    /// let mut r = Reader::<64>::new(ReaderConfig::new(&g_less))?;
    /// let mut events: Vec<OwnedEvent> = Vec::new();
    /// r.decode_all(&[0xA0, 0x00, 0x4A, 0x01, 0x02, b'a', 0x00], &mut events)?;
    /// assert_eq!(events.len(), 4);
    ///
    /// r.reset_with_grammars(&g_inf);
    /// events.clear();
    /// r.decode_all(&strict_bytes, &mut events)?;
    /// assert!(matches!(&events[1], OwnedEvent::StartElement(n) if n.as_name().local_name() == "ping"));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn reset_with_grammars(&mut self, grammars: &'g Grammars<'g>) {
        self.cfg.grammars = grammars;
        self.reset();
    }
}

impl<const N: usize> EventSource for Reader<'_, N> {
    fn next(&mut self) -> Result<Step<'_>> {
        Reader::next(self)
    }
}

impl<'g, const N: usize> Reader<'g, N> {
    /// The most input one incomplete event may hold (module docs): the
    /// longest string value in UTF-8, or the longest binary value, with
    /// room for the event's code and names.
    fn spill_cap(&self) -> usize {
        let l = &self.cfg.limits;
        (l.get_max_value_chars() as usize)
            .saturating_mul(4)
            .max(l.get_max_binary_bytes() as usize)
            .saturating_add(1 << 16)
    }

    fn input(&self) -> (&[u8], &[u8]) {
        if self.spilling {
            (self.spill.data(), &[])
        } else {
            self.ring.filled()
        }
    }

    fn available(&self) -> usize {
        if self.spilling { self.spill.len() } else { self.ring.len() }
    }

    fn consume(&mut self, n: usize) {
        if self.spilling {
            self.spill.consume(n);
            if self.spill.is_empty() {
                self.spilling = false;
            }
        } else {
            self.ring.consume(n);
        }
        self.pos += n as u64;
    }

    fn position_now(&self) -> Position {
        Position {
            byte: self.pos,
            bit: self.bit,
            event: self.events,
            line: 0,
            column: 0,
        }
    }

    /// Consumes the input used by an event or value.
    fn advance_input(&mut self, used_bits: u64, inflated: bool) {
        let octets = (used_bits / 8) as usize;
        if inflated {
            #[cfg(feature = "compression")]
            {
                self.block
                    .as_mut()
                    .expect("inflated implies a block")
                    .z
                    .pos += octets;
                return;
            }
        }
        self.consume(octets);
        self.bit = (used_bits % 8) as u8;
    }

    /// How much input the next decode may see: up to the end of the
    /// current frame with `Frames`; `None` means "wait for more"
    /// (frames only).
    fn limit(&self) -> Option<(usize, bool)> {
        let avail = self.available();
        if self.cfg.sequence != Sequence::Frames {
            return Some((avail, self.ended));
        }
        match self.frame_ends.front() {
            Some(&end) => Some((((end - self.pos) as usize).min(avail), true)),
            None if self.ended => Some((avail, true)),
            None => None,
        }
    }

    fn limited(&self, n: usize) -> (&[u8], &[u8]) {
        let (a, b) = self.input();
        if n <= a.len() { (&a[..n], &[]) } else { (a, &b[..n - a.len()]) }
    }

    fn spill_if_full(&mut self) {
        if self.ring.free() == 0 {
            self.spill_now();
        }
    }

    /// Ring full while an event is incomplete: move the input to the
    /// spill buffer.
    fn spill_now(&mut self) {
        if !self.spilling {
            let (a, b) = self.ring.filled();
            self.spill.clear();
            self.spill.extend(a);
            self.spill.extend(b);
            self.ring.clear();
            self.spilling = true;
        }
    }

    fn read_header(&mut self) -> Result<Option<()>> {
        let Some((n, final_input)) = self.limit() else { return Ok(None) };
        let (a, b) = self.limited(n);
        let Some(h) = parse_header_split(a, b, &self.cfg.limits)? else {
            if final_input {
                return Err(err(
                    ErrorKind::Truncated,
                    "input ends inside the header",
                ));
            }
            self.spill_if_full();
            return Ok(None);
        };
        let Some(opts) = h.options.or_else(|| self.cfg.options.clone()) else {
            return Err(err(
                ErrorKind::Header,
                "no options in the header and none given",
            ));
        };
        self.cfg.sequence.check_options(&opts)?;
        check_options(&opts, self.cfg.grammars)?;
        self.consume((h.body_offset_bits / 8) as usize);
        // A bit-packed body starts inside the header's last octet.
        self.bit = (h.body_offset_bits % 8) as u8;
        #[cfg(feature = "compression")]
        {
            self.block = match opts.coding() {
                Coding::PreCompression => {
                    Some(BlockRead::new(false, opts.block_size()))
                }
                Coding::Compression => {
                    Some(BlockRead::new(true, opts.block_size()))
                }
                _ => None,
            };
        }
        self.sess = None;
        let (st, tables) = engine::new_reader_session(
            opts,
            self.cfg.grammars,
            self.cfg.limits,
        );
        self.sess = Some(Session { st, tables });
        self.depth_out = 0;
        self.sc_collect = false;
        self.phase = Phase::Body;
        self.out = Out::SD;
        Ok(Some(()))
    }

    /// Decodes one body event into `self.out`; `None` when more input
    /// is needed.
    fn read_event(&mut self) -> Result<Option<()>> {
        if self.spilling && self.spill.len() > self.spill_cap() {
            return Err(err(
                ErrorKind::Limit,
                "one event is larger than the limits allow",
            ));
        }
        let frames = self.cfg.sequence == Sequence::Frames;
        let sess = self.sess.as_ref().ok_or_else(|| {
            err(ErrorKind::Internal, "body without a session")
        })?;
        let coding = sess.st.opts.coding();
        let at_top = sess.st.stack.is_empty();
        if frames
            && at_top
            && self.frame_ends.is_empty()
            && self.ended
            && self.available() == 0
        {
            // A libexi convention: a frame sequence may end without ED.
            self.phase = Phase::End;
            return Ok(None);
        }
        // Compression: the block's structure is in a DEFLATE stream,
        // inflated whole first.
        let inflated = self.inflated();
        if inflated && !self.inflate_stream()? {
            return Ok(None);
        }
        let defer = self.blocks();
        let (n, final_input) = if inflated {
            (0, true)
        } else {
            let Some(l) = self.limit() else { return Ok(None) };
            l
        };
        let frame_bounded = frames && !self.frame_ends.is_empty();
        let bit = self.bit;
        let Some(used) = ('decode: {
            let Reader {
                ring,
                spill,
                spilling,
                sess,
                dec,
                #[cfg(feature = "compression")]
                block,
                ..
            } = self;
            #[cfg(feature = "compression")]
            let z = block
                .as_ref()
                .filter(|_| inflated)
                .map(|b| &b.z.out[b.z.pos..]);
            #[cfg(not(feature = "compression"))]
            let z = None;
            let (a, b) = input_view(ring, spill.data(), *spilling, z, n);
            let sess = sess.as_mut().expect("checked above");
            if !final_input {
                // Try single-pass decode directly on `sess.tables`. Hits and
                // string values cannot leave partial state on `Truncated`;
                // only new URI/local/prefix additions trigger the 2-pass probe.
                let mut d = BodyDecoder::new(coding, a, b, bit);
                let mut tt = TryTables::new(&mut sess.tables);
                dec.clear();
                match engine::decode(&sess.st, &mut d, &mut tt, dec, defer) {
                    Ok(()) => break 'decode Some(d.bit_position()),
                    Err(e)
                        if e.kind() == ErrorKind::Truncated
                            && !tt.need_probe =>
                    {
                        break 'decode None;
                    }
                    Err(e) if !tt.need_probe => return Err(e),
                    Err(_) => {
                        let mut d = BodyDecoder::new(coding, a, b, bit);
                        let mut probe = Probe::new(&sess.tables);
                        dec.clear();
                        match engine::decode(
                            &sess.st, &mut d, &mut probe, dec, defer,
                        ) {
                            Ok(()) => {}
                            Err(e) if e.kind() == ErrorKind::Truncated => {
                                break 'decode None;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
            let mut d = BodyDecoder::new(coding, a, b, bit);
            dec.clear();
            match engine::decode(
                &sess.st,
                &mut d,
                &mut sess.tables,
                dec,
                defer,
            ) {
                Ok(()) => {}
                Err(e)
                    if e.kind() == ErrorKind::Truncated && frame_bounded =>
                {
                    return Err(err(
                        ErrorKind::Grammar,
                        "a frame ends inside an element",
                    ));
                }
                Err(e) => return Err(e),
            }
            Some(d.bit_position())
        }) else {
            self.spill_if_full();
            return Ok(None);
        };
        self.advance_input(used, inflated);
        self.note_deferred();
        self.apply()?;
        // A self-contained element's fragment starts and ends on an
        // octet, and a stream ends with padding to one.
        if matches!(self.out, Out::SC | Out::ED) && self.bit != 0 {
            self.consume(1);
            self.bit = 0;
        }
        if frames {
            let sess = self.sess.as_ref().expect("checked above");
            if matches!(self.out, Out::EE(..)) && sess.st.stack.is_empty() {
                match self.frame_ends.front() {
                    Some(&end) if end == self.pos => {
                        self.frame_ends.pop_front();
                    }
                    Some(_) => {
                        return Err(err(
                            ErrorKind::Grammar,
                            "octets after the element in a frame",
                        ));
                    }
                    None => {}
                }
            }
        }
        Ok(Some(()))
    }

    /// Moves the grammar state for the decoded event and fills
    /// `self.out`.
    fn apply(&mut self) -> Result<()> {
        let direct = self.queue.is_empty() && !self.collecting && !self.blocks();
        let sess = self.sess.as_mut().expect("body has a session");
        let dec = &mut self.dec;
        let str_buf = &mut self.str_buf;
        let term = dec
            .term
            .ok_or_else(|| err(ErrorKind::Internal, "no terminal decoded"))?;
        let code = dec.code.ok_or_else(|| {
            err(ErrorKind::Internal, "no event code decoded")
        })?;
        let can_pin_hit = direct
            || (sess.tables.rules().get_capacity().is_none()
                && !sess.st.in_sc());
        let value = |t: &SessionTables<'_>,
                     dec: &mut Decoded,
                     str_buf: &mut String|
         -> OutVal {
            out_val(
                t,
                core::mem::take(&mut dec.value),
                &dec.text,
                str_buf,
                direct,
                can_pin_hit,
            )
        };
        self.out = match term {
            Terminal::SE(_) | Terminal::SeUri(_) | Terminal::SeAny => {
                let (id, p) = dec.name.ok_or_else(|| {
                    err(ErrorKind::Internal, "SE without a name")
                })?;
                sess.st.on_se(term, code, id, p)?;
                if self.sc_se {
                    self.sc_se = false;
                    self.suppress = true;
                }
                Out::SE(id, p)
            }
            Terminal::AT(_) | Terminal::AtUri(_) | Terminal::AtAny => {
                let (id, p) = dec.name.ok_or_else(|| {
                    err(ErrorKind::Internal, "AT without a name")
                })?;
                match dec.slot {
                    Some(Slot::Nil) => {
                        let DVal::Nil(b) = dec.value else {
                            return Err(err(
                                ErrorKind::Internal,
                                "nil value expected",
                            ));
                        };
                        sess.st.on_xsi_nil(b)?;
                    }
                    Some(Slot::NilText) => {
                        let DVal::Str(r) = &dec.value else {
                            return Err(err(
                                ErrorKind::Internal,
                                "nil text expected",
                            ));
                        };
                        let s = engine::value_str(&sess.tables, r, &dec.text);
                        let b = crate::lexical::parse_bool(s)
                            .map(|x| x.0)
                            .ok_or_else(|| {
                                err(
                                    ErrorKind::Grammar,
                                    "xsi:nil is not a Boolean",
                                )
                            })?;
                        sess.st.on_xsi_nil(b)?;
                    }
                    // Non-strict xsi:type changes the grammar only
                    // when it names a type of the schema.
                    Some(Slot::TypeName | Slot::QName)
                        if dec.slot == Some(Slot::TypeName)
                            || (term == Terminal::AT(engine::XSI_TYPE)
                                && sess.st.deviates(code)) =>
                    {
                        let DVal::QName(tid, _) = dec.value else {
                            return Err(err(
                                ErrorKind::Internal,
                                "type name expected",
                            ));
                        };
                        let uri = sess.tables.uri_str(tid.uri).unwrap_or("");
                        let local = sess
                            .tables
                            .local_str(tid.uri, tid.local)
                            .unwrap_or("");
                        let ty = if dec.slot == Some(Slot::TypeName) {
                            Some(sess.st.strict_xsi_type(
                                uri,
                                local,
                                ErrorKind::Grammar,
                            )?)
                        } else {
                            let schema = sess.st.schema.ok_or_else(|| {
                                err(ErrorKind::Internal, "no schema")
                            })?;
                            schema.type_named(uri, local)
                        };
                        sess.st.on_xsi_type(ty)?;
                    }
                    _ => sess.st.on_at(term, code, id)?,
                }
                Out::AT(id, p, value(&sess.tables, dec, str_buf))
            }
            Terminal::CH => {
                sess.st.on_ch(code)?;
                Out::CH(value(&sess.tables, dec, str_buf))
            }
            Terminal::EE => {
                let f = sess.st.on_ee()?;
                if sess.st.in_sc() && sess.st.stack.is_empty() {
                    self.sc_ed = true;
                }
                Out::EE(f.name, f.prefix)
            }
            Terminal::ED => {
                sess.st.on_ed();
                if self.sc_ed {
                    self.sc_ed = false;
                    self.suppress = true;
                    sess.tables.end_self_contained()?;
                    sess.st.end_sc()?;
                }
                Out::ED
            }
            Terminal::SC => {
                if self.collecting {
                    self.sc_collect = true;
                }
                sess.tables.begin_self_contained();
                sess.st.begin_sc();
                self.sc_se = true;
                Out::SC
            }
            Terminal::NS => {
                sess.st.on_misc(term, code)?;
                let (u, p, l) = dec.ns.ok_or_else(|| {
                    err(ErrorKind::Internal, "NS without content")
                })?;
                Out::NS(u, p, l)
            }
            Terminal::CM | Terminal::PI | Terminal::DT | Terminal::ER => {
                sess.st.on_misc(term, code)?;
                let mut t = core::mem::take(&mut dec.texts).into_iter();
                let mut next = || t.next().unwrap_or_default();
                match term {
                    Terminal::CM => Out::CM(next()),
                    Terminal::PI => Out::PI(next(), next()),
                    Terminal::DT => Out::DT(next(), next(), next(), next()),
                    _ => Out::ER(next()),
                }
            }
        };
        Ok(())
    }

    /// Advances to the next event; `Ok(false)` for need-input,
    /// `Ok(true)` with `self.out` set, or `self.phase == End`.
    fn advance(&mut self) -> Result<bool> {
        loop {
            match self.phase {
                Phase::End => return Ok(true),
                Phase::Header => {
                    if self.cfg.sequence == Sequence::Streams
                        && self.sess.is_some()
                        && self.ended
                        && self.available() == 0
                    {
                        self.phase = Phase::End;
                        continue;
                    }
                    return Ok(self.read_header()?.is_some());
                }
                Phase::Body => {
                    let done = self
                        .sess
                        .as_ref()
                        .is_some_and(|s| s.st.top == Top::Done);
                    if done {
                        match self.cfg.sequence {
                            Sequence::Streams => {
                                self.phase = Phase::Header;
                                continue;
                            }
                            _ => {
                                if self.ended {
                                    self.phase = Phase::End;
                                    continue;
                                }
                                return Ok(false);
                            }
                        }
                    }
                    match self.read_event()? {
                        Some(()) => return Ok(true),
                        None if self.phase == Phase::End => continue,
                        None => return Ok(false),
                    }
                }
            }
        }
    }

    fn event_meta(&self, out: &Out, pos: Position) -> EventMeta {
        let sess = self.sess.as_ref();
        let mut depth = sess.map_or(0, |s| s.st.depth());
        if matches!(out, Out::SC) {
            // SC belongs to the element it follows; the grammars have
            // just moved that element aside, so it is not counted yet.
            depth += 1;
        }
        let lexical =
            sess.is_some_and(|s| s.st.lexical() && s.st.schema.is_some());
        let flags = if lexical && matches!(out, Out::AT(..) | Out::CH(..)) {
            ValueFlags::LEXICAL
        } else {
            ValueFlags::empty()
        };
        EventMeta { depth, position: pos, flags }
    }

    /// Adds a decoded event to the queue. With `Preserve.prefixes` an
    /// SE waits for its NS events: the one marked `local_element_ns`
    /// gives the element its prefix.
    fn enqueue(&mut self, out: Out, pos: Position) {
        let meta = self.event_meta(&out, pos);
        let sess = self.sess.as_mut();
        let prefixes = sess.as_ref().is_some_and(|s| s.st.prefixes());
        if self.collecting {
            if let Out::NS(_, p, true) = out {
                if let Some((Out::SE(_, sp), _)) =
                    self.queue.get_mut(self.collect_at)
                {
                    *sp = Some(p);
                }
                if let Some(s) = sess {
                    if let Some(f) = s.st.stack.last_mut() {
                        f.prefix = Some(p);
                    }
                }
            }
            if !matches!(out, Out::NS(..)) {
                self.collecting = false;
            }
        }
        // Also an SE right after its parent's SE or NS events, which
        // ended the parent's wait above.
        if !self.collecting && prefixes && matches!(out, Out::SE(..)) {
            self.collecting = true;
            self.collect_at = self.queue.len();
        }
        self.queue.push_back((out, meta));
    }

    fn fail(&mut self, e: crate::Error, pos: Position) -> crate::Error {
        self.poisoned = true;
        if e.position() == Position::default() { e.at(pos) } else { e }
    }

    fn step(&mut self) -> Result<Step<'_>> {
        if self.poisoned {
            return Err(err(
                ErrorKind::State,
                "the reader is poisoned by an earlier error",
            ));
        }
        if self.queue.is_empty() {
            self.str_buf.clear();
        }
        let mut outer = false;
        let meta = loop {
            if !self.collecting {
                if self.can_release() {
                    if let Some((o, meta)) = self.queue.pop_front() {
                        self.popped();
                        self.depth_out = meta.depth;
                        if matches!(o, Out::SC) {
                            self.sc_collect = false;
                        } else {
                            outer = self.sc_collect;
                        }
                        self.out = o;
                        break meta;
                    }
                }
                if self.phase == Phase::End && self.queue.is_empty() {
                    return Ok(Step::End);
                }
            }
            let pos = self.position_now();
            if self.in_values() {
                // A block's structure is complete: its values next.
                if self.read_values().map_err(|e| self.fail(e, pos))? {
                    continue;
                }
                return Ok(Step::NeedInput);
            }
            match self.advance().map_err(|e| self.fail(e, pos))? {
                false => return Ok(Step::NeedInput),
                true if self.phase == Phase::End => self.collecting = false,
                true if self.suppress => self.suppress = false,
                true => {
                    let prefixes =
                        self.sess.as_ref().is_some_and(|s| s.st.prefixes());
                    if self.queue.is_empty()
                        && !self.collecting
                        && !self.blocks()
                        && !(prefixes && matches!(self.out, Out::SE(..)))
                    {
                        let meta = self.event_meta(&self.out, pos);
                        self.depth_out = meta.depth;
                        break meta;
                    }
                    let out = core::mem::take(&mut self.out);
                    let is_ed = matches!(out, Out::ED);
                    self.enqueue(out, pos);
                    self.after_enqueue(is_ed)
                        .map_err(|e| self.fail(e.at(pos), pos))?;
                }
            }
        };
        self.events += 1;
        let this: &Self = self;
        let s = this.sess.as_ref().ok_or_else(|| {
            err(ErrorKind::Internal, "an event without a session")
        })?;
        let t = &s.tables;
        let ev = match &this.out {
            Out::None => return Err(err(ErrorKind::Internal, "no event")),
            Out::SD => Event::StartDocument,
            Out::ED => Event::EndDocument,
            Out::SE(id, p) => Event::StartElement(name(t, outer, *id, *p)),
            Out::EE(id, p) => Event::EndElement(name(t, false, *id, *p)),
            Out::AT(id, p, v) => Event::Attribute(
                name(t, false, *id, *p),
                v.as_value(t, &this.dec.text, &this.str_buf),
            ),
            Out::CH(v) => Event::Characters(
                v.as_value(t, &this.dec.text, &this.str_buf),
            ),
            Out::NS(u, p, l) => Event::Namespace(NsDecl {
                prefix: t.prefix_str_at(outer, *u, *p).unwrap_or(""),
                uri: t.uri_str_at(outer, *u).unwrap_or(""),
                local_element_ns: *l,
            }),
            Out::CM(c) => Event::Comment(c),
            Out::PI(target, data) => {
                Event::ProcessingInstruction { target, data }
            }
            Out::DT(n, p, sy, tx) => Event::Doctype(Doctype {
                name: n,
                public_id: p,
                system_id: sy,
                text: tx,
            }),
            Out::ER(n) => Event::EntityReference(n),
            Out::SC => Event::SelfContained,
        };
        Ok(Step::Event(ev, meta))
    }
}

#[cfg(not(feature = "compression"))]
impl<'g, const N: usize> Reader<'g, N> {
    fn blocks(&self) -> bool {
        false
    }
    fn inflated(&self) -> bool {
        false
    }
    fn in_values(&self) -> bool {
        false
    }
    fn can_release(&self) -> bool {
        true
    }
    fn popped(&mut self) {}
    fn note_deferred(&mut self) {}
    fn after_enqueue(&mut self, _is_ed: bool) -> Result<()> {
        Ok(())
    }
    fn inflate_stream(&mut self) -> Result<bool> {
        Ok(true)
    }
    fn read_values(&mut self) -> Result<bool> {
        Ok(true)
    }
}
