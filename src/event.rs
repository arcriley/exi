// src/event.rs
//
// Events which readers produce and writers consume
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # The event set
//!
//! EXI's events, plus [`Event::StartTag`] (start-tag mode only). Every
//! stream is:
//!
//! ```text
//! SD  (top-level content)  ED
//! ```
//!
//! A document has exactly one top-level element; a fragment (option
//! `fragment`) has zero or more. Comments and processing instructions
//! may also appear at top level, and a DOCTYPE before the element of a
//! document.
//!
//! The events of one element come in this order:
//!
//! ```text
//! SE  [SC]  NS*  [AT(xsi:type)]  [AT(xsi:nil)]  AT*  content*  EE
//! ```
//!
//! where content is CH, SE…EE, CM, PI and ER. Namespace declarations
//! are always NS events, never AT events. Which events a stream may
//! contain depends on its options:
//!
//! | Event | Present when |
//! |---|---|
//! | SD, ED, SE, EE, AT, CH | always |
//! | NS | `Preserve.prefixes` |
//! | CM | `Preserve.comments` |
//! | PI | `Preserve.pis` |
//! | DT, ER | `Preserve.dtd` |
//! | SC | `selfContained` |
//!
//! A writer given an event its options exclude drops it without error,
//! as EXI does; a reader never produces one.
//!
//! # libexi additions
//!
//! - `EndElement` carries the element's name (D13). EXI's EE has none;
//!   the reader keeps an element stack anyway and fills it in.
//! - `StartTag` replaces one SE, its NS events and its AT events in
//!   start-tag mode (`ReaderConfig::start_tag(true)`), as expat and
//!   libxml2 deliver a start tag. EXI marks the end of an attribute
//!   list only with the next non-attribute event, so start-tag mode
//!   delivers the tag one event later.
//!
//! # Attribute order
//!
//! Schema-less streams keep the document's attribute order. Under
//! schema-informed grammars declared attributes are sent in the order
//! the grammar fixes: sorted by local name, then by URI. The original
//! order is not transmitted, so no reader can restore it.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::error::Position;
use crate::name::{Name, OwnedName};
use crate::value::{OwnedValue, Value, ValueFlags};

/// One event.
///
/// ```
/// use exi::{Event, EventKind, Name, Value};
/// let e = Event::Attribute(Name::local("id"), Value::Str("a1"));
/// assert_eq!(e.kind(), EventKind::AT);
/// let owned = e.to_owned();
/// assert_eq!(owned.as_event(), e);
/// assert_eq!(Event::StartDocument.kind(), EventKind::SD);
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Event<'a> {
    StartDocument,
    EndDocument,
    StartElement(Name<'a>),
    /// The name of the element that ends (D13).
    EndElement(Name<'a>),
    Attribute(Name<'a>, Value<'a>),
    Characters(Value<'a>),
    Namespace(NsDecl<'a>),
    Comment(&'a str),
    ProcessingInstruction {
        target: &'a str,
        data: &'a str,
    },
    Doctype(Doctype<'a>),
    EntityReference(&'a str),
    /// The element just started is self-contained.
    SelfContained,
    /// Start-tag mode only.
    StartTag(StartTag<'a>),
}

impl Event<'_> {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::StartDocument => EventKind::SD,
            Event::EndDocument => EventKind::ED,
            Event::StartElement(_) => EventKind::SE,
            Event::EndElement(_) => EventKind::EE,
            Event::Attribute(..) => EventKind::AT,
            Event::Characters(_) => EventKind::CH,
            Event::Namespace(_) => EventKind::NS,
            Event::Comment(_) => EventKind::CM,
            Event::ProcessingInstruction { .. } => EventKind::PI,
            Event::Doctype(_) => EventKind::DT,
            Event::EntityReference(_) => EventKind::ER,
            Event::SelfContained => EventKind::SC,
            Event::StartTag(_) => EventKind::StartTag,
        }
    }
    pub fn to_owned(&self) -> OwnedEvent {
        match *self {
            Event::StartDocument => OwnedEvent::StartDocument,
            Event::EndDocument => OwnedEvent::EndDocument,
            Event::StartElement(n) => OwnedEvent::StartElement(n.to_owned()),
            Event::EndElement(n) => OwnedEvent::EndElement(n.to_owned()),
            Event::Attribute(n, v) => {
                OwnedEvent::Attribute(n.to_owned(), v.to_owned())
            }
            Event::Characters(v) => OwnedEvent::Characters(v.to_owned()),
            Event::Namespace(ns) => OwnedEvent::Namespace {
                prefix: ns.prefix.into(),
                uri: ns.uri.into(),
                local_element_ns: ns.local_element_ns,
            },
            Event::Comment(s) => OwnedEvent::Comment(s.into()),
            Event::ProcessingInstruction { target, data } => {
                OwnedEvent::ProcessingInstruction {
                    target: target.into(),
                    data: data.into(),
                }
            }
            Event::Doctype(d) => OwnedEvent::Doctype {
                name: d.name.into(),
                public_id: d.public_id.into(),
                system_id: d.system_id.into(),
                text: d.text.into(),
            },
            Event::EntityReference(s) => OwnedEvent::EntityReference(s.into()),
            Event::SelfContained => OwnedEvent::SelfContained,
            Event::StartTag(t) => OwnedEvent::StartTag {
                name: t.name.to_owned(),
                ns: t
                    .ns
                    .iter()
                    .map(|n| {
                        (n.prefix.into(), n.uri.into(), n.local_element_ns)
                    })
                    .collect(),
                attrs: t
                    .attrs
                    .iter()
                    .map(|(n, v)| (n.to_owned(), v.to_owned()))
                    .collect(),
            },
        }
    }
}

/// A namespace declaration. `local_element_ns` is true on the
/// declaration, if any, that gives the prefix of the element it belongs
/// to; at most one NS event per element sets it, and its `uri` is then
/// the element's URI.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NsDecl<'a> {
    pub prefix: &'a str,
    pub uri: &'a str,
    pub local_element_ns: bool,
}

/// A document type declaration: its name, public and system identifiers
/// (empty when absent) and the internal subset text without its
/// brackets (empty when absent).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Doctype<'a> {
    pub name: &'a str,
    pub public_id: &'a str,
    pub system_id: &'a str,
    pub text: &'a str,
}

/// A whole start tag (start-tag mode).
///
/// ```
/// use exi::{Name, NsDecl, StartTag, Value};
/// let attrs = [(Name::local("id"), Value::Str("a1")), (Name::new("urn:x", "id"), Value::Int(2))];
/// let tag = StartTag { name: Name::local("p"), ns: &[], attrs: &attrs };
/// assert_eq!(tag.attr("", "id"), Some(Value::Str("a1")));
/// assert_eq!(tag.attr("urn:x", "id"), Some(Value::Int(2)));
/// assert_eq!(tag.attr("urn:y", "id"), None);
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StartTag<'a> {
    pub name: Name<'a>,
    pub ns: &'a [NsDecl<'a>],
    pub attrs: &'a [(Name<'a>, Value<'a>)],
}

impl<'a> StartTag<'a> {
    /// The value of the attribute with this URI and local name.
    pub fn attr(&self, uri: &str, local: &str) -> Option<Value<'a>> {
        self.attrs.iter().find(|(n, _)| n.is(uri, local)).map(|(_, v)| *v)
    }
}

/// Facts about an event that are not part of it: the element depth
/// after the event (SE at top level has depth 1, its EE depth 0), where
/// it was decoded, and how its value was carried.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EventMeta {
    pub depth: u32,
    pub position: Position,
    pub flags: ValueFlags,
}

/// The kind of an event, with EXI's abbreviations.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    SD,
    ED,
    SE,
    EE,
    AT,
    CH,
    NS,
    CM,
    PI,
    DT,
    ER,
    SC,
    StartTag,
}

/// [`Event`] with owned data.
#[derive(Clone, Debug, PartialEq)]
pub enum OwnedEvent {
    StartDocument,
    EndDocument,
    StartElement(OwnedName),
    EndElement(OwnedName),
    Attribute(OwnedName, OwnedValue),
    Characters(OwnedValue),
    Namespace {
        prefix: Box<str>,
        uri: Box<str>,
        local_element_ns: bool,
    },
    Comment(Box<str>),
    ProcessingInstruction {
        target: Box<str>,
        data: Box<str>,
    },
    Doctype {
        name: Box<str>,
        public_id: Box<str>,
        system_id: Box<str>,
        text: Box<str>,
    },
    EntityReference(Box<str>),
    SelfContained,
    StartTag {
        name: OwnedName,
        ns: Vec<(Box<str>, Box<str>, bool)>,
        attrs: Vec<(OwnedName, OwnedValue)>,
    },
}

impl OwnedEvent {
    /// Borrows the event. A `StartTag` needs storage for its borrowed
    /// slices, so it is the one event this cannot return: it returns
    /// `StartElement` with the tag's name, and [`OwnedEvent::expand`]
    /// gives the full sequence.
    pub fn as_event(&self) -> Event<'_> {
        match self {
            OwnedEvent::StartDocument => Event::StartDocument,
            OwnedEvent::EndDocument => Event::EndDocument,
            OwnedEvent::StartElement(n) => Event::StartElement(n.as_name()),
            OwnedEvent::EndElement(n) => Event::EndElement(n.as_name()),
            OwnedEvent::Attribute(n, v) => {
                Event::Attribute(n.as_name(), v.as_value())
            }
            OwnedEvent::Characters(v) => Event::Characters(v.as_value()),
            OwnedEvent::Namespace { prefix, uri, local_element_ns } => {
                Event::Namespace(NsDecl {
                    prefix,
                    uri,
                    local_element_ns: *local_element_ns,
                })
            }
            OwnedEvent::Comment(s) => Event::Comment(s),
            OwnedEvent::ProcessingInstruction { target, data } => {
                Event::ProcessingInstruction { target, data }
            }
            OwnedEvent::Doctype { name, public_id, system_id, text } => {
                Event::Doctype(Doctype { name, public_id, system_id, text })
            }
            OwnedEvent::EntityReference(s) => Event::EntityReference(s),
            OwnedEvent::SelfContained => Event::SelfContained,
            OwnedEvent::StartTag { name, .. } => {
                Event::StartElement(name.as_name())
            }
        }
    }
    /// The events this one stands for: a `StartTag` becomes SE, its NS
    /// events and its AT events; any other event is itself.
    pub fn expand(&self) -> Vec<Event<'_>> {
        match self {
            OwnedEvent::StartTag { name, ns, attrs } => {
                let mut v = Vec::with_capacity(1 + ns.len() + attrs.len());
                v.push(Event::StartElement(name.as_name()));
                for (prefix, uri, local_element_ns) in ns {
                    v.push(Event::Namespace(NsDecl {
                        prefix,
                        uri,
                        local_element_ns: *local_element_ns,
                    }));
                }
                for (n, val) in attrs {
                    v.push(Event::Attribute(n.as_name(), val.as_value()));
                }
                v
            }
            other => alloc::vec![other.as_event()],
        }
    }
}
