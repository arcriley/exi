// src/stream.rs
//
// Event sources, event sinks and push handlers.
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
//! Pull is the core (D12). [`Handler`] is the push form, expat-style
//! callbacks, and [`drive`] runs any source into a handler. A pull
//! reader needs no stop-and-resume machinery: the caller owns the loop.

use crate::event::{Doctype, Event, EventMeta, NsDecl, StartTag};
use crate::name::{AsName, Name};
use crate::value::{IntoValue, Value};
use crate::{Error, Result};

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
///
/// `end_element` sends `EndElement` with an empty name, meaning "the
/// open element". A sink receiving `EndElement` with a non-empty name
/// checks it against the open element and returns `ErrorKind::State`
/// when they differ.
pub trait EventSink {
    fn event(&mut self, ev: &Event<'_>) -> Result<()>;

    fn start_element(&mut self, name: impl AsName) -> Result<()> {
        self.event(&Event::StartElement(name.as_name()))
    }
    fn attribute(
        &mut self,
        name: impl AsName,
        v: impl IntoValue,
    ) -> Result<()> {
        self.event(&Event::Attribute(name.as_name(), v.to_value()))
    }
    fn namespace(&mut self, prefix: &str, uri: &str) -> Result<()> {
        self.event(&Event::Namespace(NsDecl {
            prefix,
            uri,
            local_element_ns: false,
        }))
    }
    fn text(&mut self, v: impl IntoValue) -> Result<()> {
        self.event(&Event::Characters(v.to_value()))
    }
    fn comment(&mut self, text: &str) -> Result<()> {
        self.event(&Event::Comment(text))
    }
    fn processing_instruction(
        &mut self,
        target: &str,
        data: &str,
    ) -> Result<()> {
        self.event(&Event::ProcessingInstruction { target, data })
    }
    fn end_element(&mut self) -> Result<()> {
        self.event(&Event::EndElement(Name::local("")))
    }
}

/// expat-style callbacks. Every method defaults to doing nothing.
/// [`Handler::event`] dispatches to the others; override it to see
/// every event in one place.
#[allow(unused_variables)] // the defaults ignore their arguments
pub trait Handler {
    type Error;
    fn event(
        &mut self,
        ev: &Event<'_>,
        meta: &EventMeta,
    ) -> core::result::Result<(), Self::Error> {
        match *ev {
            Event::StartDocument => self.start_document(),
            Event::EndDocument => self.end_document(),
            Event::StartElement(n) => self.start_element(n),
            Event::EndElement(n) => self.end_element(n),
            Event::Attribute(n, v) => self.attribute(n, v),
            Event::Characters(v) => self.characters(v),
            Event::Namespace(ns) => self.namespace(ns),
            Event::Comment(s) => self.comment(s),
            Event::ProcessingInstruction { target, data } => {
                self.processing_instruction(target, data)
            }
            Event::Doctype(d) => self.doctype(d),
            Event::EntityReference(s) => self.entity_reference(s),
            Event::SelfContained => self.self_contained(),
            Event::StartTag(t) => self.start_tag(t),
        }
    }
    fn start_document(&mut self) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn end_document(&mut self) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn start_element(
        &mut self,
        name: Name<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn end_element(
        &mut self,
        name: Name<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn attribute(
        &mut self,
        name: Name<'_>,
        v: Value<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn characters(
        &mut self,
        v: Value<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn namespace(
        &mut self,
        ns: NsDecl<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn comment(
        &mut self,
        text: &str,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn processing_instruction(
        &mut self,
        target: &str,
        data: &str,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn doctype(
        &mut self,
        dt: Doctype<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn entity_reference(
        &mut self,
        name: &str,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn self_contained(&mut self) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
    fn start_tag(
        &mut self,
        tag: StartTag<'_>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
}

/// An error from [`drive`]: from the source, or from the handler.
#[derive(Debug)]
pub enum DriveError<E> {
    Exi(Error),
    Handler(E),
}

/// Runs `src` into `h` until the source needs input or ends. A handler
/// error stops the run at once and comes back as `DriveError::Handler`;
/// the event that caused it is consumed.
///
/// ```
/// # #[cfg(feature = "xml")] {
/// use exi::{drive, DriveError, Handler, Name, Status};
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::xml::{XmlReader, XmlConfig};
/// # use exi::grammar::GrammarSet;
/// #[derive(Default)]
/// struct Log(Vec<String>);
/// impl Handler for Log {
///     type Error = core::convert::Infallible;
///     fn start_element(&mut self, n: Name<'_>) -> Result<(), Self::Error> {
///         self.0.push(format!("<{}", n.local_name()));
///         Ok(())
///     }
///     fn end_element(&mut self, n: Name<'_>) -> Result<(), Self::Error> {
///         self.0.push(format!("{}>", n.local_name()));
///         Ok(())
///     }
/// }
/// let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let set = GrammarSet::schemaless();
/// let grammars = set.attach(&strings)?;
/// let mut r: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
/// r.feed(b"<a><b/></a>");
/// r.end_input();
/// let mut log = Log::default();
/// assert!(matches!(drive(&mut r, &mut log), Ok(Status::End)));
/// assert_eq!(log.0, ["<a", "<b", "b>", "a>"]);       // EE carries the name (D13)
///
/// struct Stop;
/// impl Handler for Stop {
///     type Error = &'static str;
///     fn start_element(&mut self, _: Name<'_>) -> Result<(), &'static str> { Err("stop") }
/// }
/// let mut r: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
/// r.feed(b"<a/>");
/// r.end_input();
/// assert!(matches!(drive(&mut r, &mut Stop), Err(DriveError::Handler("stop"))));
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
pub fn drive<S: EventSource, H: Handler>(
    src: &mut S,
    h: &mut H,
) -> core::result::Result<Status, DriveError<H::Error>> {
    loop {
        match src.next().map_err(DriveError::Exi)? {
            Step::Event(ev, meta) => {
                h.event(&ev, &meta).map_err(DriveError::Handler)?
            }
            Step::NeedInput => return Ok(Status::NeedInput),
            Step::End => return Ok(Status::End),
        }
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
pub fn transcode<S: EventSource, K: EventSink>(
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
