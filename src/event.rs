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
//! Every EXI stream is:
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
//!
//! # Attribute order
//!
//! Schema-less streams keep the document's attribute order. Under
//! schema-informed grammars declared attributes are sent in the order
//! the grammar fixes: sorted by local name, then by URI. The original
//! order is not transmitted, so no reader can restore it.

use alloc::boxed::Box;

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
}

impl OwnedEvent {
    /// Borrows the event.
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
        }
    }
}
