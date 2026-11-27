// src/stream.rs
//
// Event sources and event sinks.
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Every front end is an [`EventSource`] (the EXI [`Reader`](crate::Reader), the XML text
//! reader), and every back end an [`EventSink`] (the EXI [`Writer`](crate::Writer), the XML
//! text writer). Converting one format into another is one loop:
//! [`transcode`].
//!
//! Sources are pulled, not pushed: the caller owns the loop and calls
//! `next` when it wants an event. A reader then needs no machinery to
//! stop and resume a callback mid-document; it returns `NeedInput` and
//! continues on the next call. A push (callback) style is a few lines
//! over `next` where an application wants one.
//!
//! Call [`EventSource::next`] directly to inspect event metadata or
//! stop after a particular event.
//!
//! ```
//! use exi::{Coding, Event, Options, Reader, ReaderConfig, Step};
//! # use exi::{grammar::GrammarSet, strings::{GlobalConfig, GlobalStrings}};
//! # let strings = GlobalStrings::new(GlobalConfig::new())?;
//! # let set = GrammarSet::schemaless();
//! # let grammars = set.attach(&strings)?;
//! let options = Options::builder().coding(Coding::ByteAligned).build()?;
//! let mut reader: Reader = Reader::new(ReaderConfig::new(&grammars).options(options))?;
//! let mut input = [&[0x80, 0x01, 0x02][..], &[b'a', 0x00][..]].into_iter();
//! let mut root_depth = None;
//! loop {
//!     match reader.next()? {
//!         Step::Event(Event::StartElement(_), meta) => root_depth = Some(meta.depth),
//!         Step::Event(..) => {}
//!         Step::NeedInput => match input.next() {
//!             Some(chunk) => assert_eq!(reader.feed(chunk), chunk.len()),
//!             None => reader.end_input(),
//!         },
//!         Step::End => break,
//!     }
//! }
//! assert_eq!(root_depth, Some(1));
//! # Ok::<(), exi::Error>(())
//! ```

use crate::event::{Event, EventMeta, NsDecl};
use crate::name::{AsName, Name};
use crate::value::IntoValue;
use crate::Result;

/// What a source returns from `next`.
#[derive(Debug)]
pub enum Step<'r> {
    /// An event, borrowing the source until the next call.
    Event(Event<'r>, EventMeta),
    /// The input given so far is used up, inside or between events.
    /// Give more input and call `next` again; no state is lost.
    NeedInput,
    /// The stream ended (after ED), and no more input will come.
    End,
}

/// Where a run of events stopped.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NeedInput,
    End,
}

/// Anything that produces events.
///
/// The returned event borrows the source, so a source cannot be an
/// `Iterator`; collect owned events with `OwnedEvent` when needed.
pub trait EventSource {
    fn next(&mut self) -> Result<Step<'_>>;
}

/// Anything that consumes events.
///
/// The provided methods build an event and pass it to [`event`](EventSink::event).
/// They need a sized sink, so that `dyn EventSink` is possible. A
/// `&mut dyn EventSink` calls `event` directly, and reaches the
/// provided methods through the impl for `&mut K`:
///
/// ```
/// use exi::{Event, EventSink, OwnedEvent};
/// let mut v: Vec<OwnedEvent> = Vec::new();
/// let mut sink: &mut dyn EventSink = &mut v;
/// sink.event(&Event::StartDocument)?;
/// (&mut sink).start_element("a")?;
/// (&mut sink).end_element()?;
/// (&mut sink).end_document()?;
/// assert_eq!(v.len(), 4);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// `end_element` sends `EndElement` with an empty name, meaning "the
/// open element". A sink receiving `EndElement` with a non-empty name
/// checks it against the open element and returns `ErrorKind::State`
/// when they differ.
pub trait EventSink {
    fn event(&mut self, ev: &Event<'_>) -> Result<()>;

    fn start_document(&mut self) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::StartDocument)
    }
    fn end_document(&mut self) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::EndDocument)
    }
    fn start_element(&mut self, name: impl AsName) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::StartElement(name.as_name()))
    }
    fn attribute(&mut self, name: impl AsName, v: impl IntoValue) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::Attribute(name.as_name(), v.to_value()))
    }
    /// Sends NS with `local_element_ns` false. To declare the prefix of
    /// the element just started, send `Event::Namespace` with
    /// `local_element_ns` true, so that the element keeps its prefix
    /// (`crate::coding` names rules).
    fn namespace(&mut self, prefix: &str, uri: &str) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::Namespace(NsDecl {
            prefix,
            uri,
            local_element_ns: false,
        }))
    }
    fn text(&mut self, v: impl IntoValue) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::Characters(v.to_value()))
    }
    fn comment(&mut self, text: &str) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::Comment(text))
    }
    fn processing_instruction(
        &mut self,
        target: &str,
        data: &str,
    ) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::ProcessingInstruction { target, data })
    }
    fn end_element(&mut self) -> Result<()>
    where
        Self: Sized,
    {
        self.event(&Event::EndElement(Name::local("")))
    }
}

/// A borrowed source: lend a source to a function that takes one by
/// value, or use `&mut dyn EventSource`.
impl<S: EventSource + ?Sized> EventSource for &mut S {
    fn next(&mut self) -> Result<Step<'_>> {
        (**self).next()
    }
}

impl<S: EventSource + ?Sized> EventSource for alloc::boxed::Box<S> {
    fn next(&mut self) -> Result<Step<'_>> {
        (**self).next()
    }
}

/// A borrowed sink: lend a sink to a function that takes one by value,
/// and use it again afterwards.
///
/// ```
/// use exi::{EventSink, OwnedEvent};
/// fn root(mut sink: impl EventSink) -> exi::Result<()> {
///     sink.start_element("a")?;
///     sink.end_element()
/// }
/// let mut v: Vec<OwnedEvent> = Vec::new();
/// root(&mut v)?;
/// root(&mut v)?;
/// assert_eq!(v.len(), 4);
/// # Ok::<(), exi::Error>(())
/// ```
impl<K: EventSink + ?Sized> EventSink for &mut K {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        (**self).event(ev)
    }
}

impl<K: EventSink + ?Sized> EventSink for alloc::boxed::Box<K> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        (**self).event(ev)
    }
}

/// Runs `src` into `sink` until the source needs input or ends. The
/// source's ED ends the sink's output too (a writer's `end_document`).
///
/// ```
/// # #[cfg(feature = "xml")] {
/// use exi::{transcode, Status, Writer, WriterConfig, Options, Coding};
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::xml::{XmlReader, XmlConfig};
/// # use exi::grammar::GrammarSet;
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let set = GrammarSet::schemaless();
/// let grammars = set.attach(&strings)?;
/// let mut xml: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
/// xml.feed(b"<a/>");
/// xml.end_input();
/// let opts = Options::builder().coding(Coding::ByteAligned).build()?;
/// let cfg = WriterConfig::new(opts, &grammars).header(exi::Header::Bare { cookie: false });
/// let mut w = Writer::new(Vec::new(), cfg)?;
/// assert_eq!(transcode(&mut xml, &mut w)?, Status::End);
/// assert_eq!(w.into_sink(), [0x80, 0x01, 0x02, b'a', 0x00]);   // see writer.rs
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
pub fn transcode<S: EventSource + ?Sized, K: EventSink + ?Sized>(
    src: &mut S,
    sink: &mut K,
) -> Result<Status> {
    loop {
        match src.next()? {
            Step::Event(ev, _) => sink.event(&ev)?,
            Step::NeedInput => return Ok(Status::NeedInput),
            Step::End => return Ok(Status::End),
        }
    }
}

/// Collects events: every event, owned, in order.
///
/// ```
/// use exi::{EventSink, OwnedEvent};
/// let mut v: Vec<OwnedEvent> = Vec::new();
/// v.start_element("a")?;
/// v.end_element()?;
/// assert_eq!(v.len(), 2);
/// # Ok::<(), exi::Error>(())
/// ```
impl EventSink for alloc::vec::Vec<crate::OwnedEvent> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        self.push(ev.to_owned());
        Ok(())
    }
}
