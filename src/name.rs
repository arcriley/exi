// src/name.rs
//
// Names of elements and attributes
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Identity
//!
//! A name is a namespace URI and a local name. Two names are equal when
//! both match; the prefix plays no part. A name in no namespace has the
//! empty string `""` as its URI, and an unprefixed attribute is in no
//! namespace.
//!
//! # Three forms
//!
//! | Form | Holds | Compared | Valid for |
//! |---|---|---|---|
//! | [`Name`] | borrowed strings, plus the two id forms when known | by `(uri, local)` | the borrow |
//! | [`NameId`] | the name's numbers in one session's string tables | as integers | one stream (see below) |
//! | [`GlobalName`] | the name's ids in the global tables | as integers | while the strings are referenced |
//!
//! A `NameId` names the same name for the whole stream, because name
//! partitions only grow. Inside a self-contained element the tables
//! restart from their initial entries, so a `NameId` reported there
//! means something else outside it; the reader reports `id: None`
//! inside self-contained elements for names that are not initial
//! entries.
//!
//! A `GlobalName` compares names across every stream that shares one
//! [`GlobalStrings`](crate::strings::GlobalStrings). The reader sets it on every name it
//! reports.

use alloc::boxed::Box;

use crate::strings::{LocalId, UriId};
use crate::{Error, ErrorKind, Result};

/// A name as the numbers one session gives it: `uri` is its number in
/// the session's URI partition, `local` its number in that URI's
/// local-name partition. Both come from [`SessionTables`](crate::tables::SessionTables).
///
/// Meaningful only in the session that produced it: two sessions can
/// give different numbers to the same name, and the same numbers to
/// different names. Names declared by a schema are initial entries, so
/// they have the same `NameId` in every session that uses the same grammars ([`GrammarSet::name_id`](crate::grammar::GrammarSet::name_id)).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NameId {
    pub uri: u32,
    pub local: u32,
}

/// A name as ids in the global tables: equal across every session
/// sharing those tables.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalName {
    pub uri: UriId,
    pub local: LocalId,
}

/// A borrowed name.
///
/// ```
/// use exi::Name;
/// let a = Name::new("http://www.w3.org/2000/svg", "g");
/// let b = Name::new("http://www.w3.org/2000/svg", "g").with_prefix("svg");
/// assert_eq!(a, b);                                  // the prefix is not part of identity
/// assert_eq!(b.prefix(), Some("svg"));
/// assert!(a.is("http://www.w3.org/2000/svg", "g"));
/// assert_ne!(a, Name::local("g"));                   // different URI
/// assert_eq!(Name::local("g").uri(), "");            // no namespace
/// assert_eq!((a.id(), a.global()), (None, None));    // only names from a session carry ids
/// ```
#[derive(Copy, Clone, Debug)]
pub struct Name<'a> {
    uri: &'a str,
    local: &'a str,
    prefix: Option<&'a str>,
    id: Option<NameId>,
    global: Option<GlobalName>,
}

impl<'a> Name<'a> {
    pub const fn new(uri: &'a str, local: &'a str) -> Self {
        Name { uri, local, prefix: None, id: None, global: None }
    }
    /// A name in no namespace.
    pub const fn local(local: &'a str) -> Self {
        Name { uri: "", local, prefix: None, id: None, global: None }
    }
    pub const fn with_prefix(self, prefix: &'a str) -> Self {
        Name { prefix: Some(prefix), ..self }
    }
    /// Used by readers to attach the ids they know.
    pub const fn with_ids(
        self,
        id: Option<NameId>,
        global: Option<GlobalName>,
    ) -> Self {
        Name { id, global, ..self }
    }
    pub const fn uri(&self) -> &'a str {
        self.uri
    }
    pub const fn local_name(&self) -> &'a str {
        self.local
    }
    pub const fn prefix(&self) -> Option<&'a str> {
        self.prefix
    }
    pub const fn id(&self) -> Option<NameId> {
        self.id
    }
    pub const fn global(&self) -> Option<GlobalName> {
        self.global
    }
    pub fn is(&self, uri: &str, local: &str) -> bool {
        self.uri == uri && self.local == local
    }
    pub fn to_owned(&self) -> OwnedName {
        OwnedName {
            uri: self.uri.into(),
            local: self.local.into(),
            prefix: self.prefix.map(Into::into),
        }
    }
}

/// Compares `(uri, local)` only.
impl PartialEq for Name<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri && self.local == other.local
    }
}
impl Eq for Name<'_> {}

/// A name that owns its strings.
///
/// [`parse_clark`](OwnedName::parse_clark) reads James Clark's notation: `"{uri}local"`, or
/// `"local"` for no namespace.
///
/// ```
/// use exi::OwnedName;
/// let n = OwnedName::parse_clark("{urn:example}note")?;
/// assert!(n.as_name().is("urn:example", "note"));
/// assert!(OwnedName::parse_clark("note")?.as_name().is("", "note"));
/// assert!(OwnedName::parse_clark("{}note")?.as_name().is("", "note"));   // empty URI: no namespace
/// for bad in ["", "{urn:x", "{urn:x}", "a{b}c", "{a}b}c"] {
///     assert_eq!(OwnedName::parse_clark(bad).unwrap_err().kind(), exi::ErrorKind::Arg, "{bad}");
/// }
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OwnedName {
    pub(crate) uri: Box<str>,
    pub(crate) local: Box<str>,
    pub(crate) prefix: Option<Box<str>>,
}

impl OwnedName {
    pub fn new(uri: &str, local: &str) -> Self {
        OwnedName { uri: uri.into(), local: local.into(), prefix: None }
    }
    pub fn with_prefix(self, prefix: &str) -> Self {
        OwnedName { prefix: Some(prefix.into()), ..self }
    }
    /// # Errors
    ///
    /// `ErrorKind::Arg` when the local name is empty, a `{` is not
    /// closed, or `{` or `}` appears in the local name.
    pub fn parse_clark(s: &str) -> Result<Self> {
        let bad =
            || Error::new(ErrorKind::Arg, "not a name in Clark notation");
        let (uri, local) = match s.strip_prefix('{') {
            Some(rest) => {
                let end = rest.find('}').ok_or_else(bad)?;
                (&rest[..end], &rest[end + 1..])
            }
            None => ("", s),
        };
        if local.is_empty() || local.contains(['{', '}']) {
            return Err(bad());
        }
        Ok(OwnedName::new(uri, local))
    }
    pub fn as_name(&self) -> Name<'_> {
        let n = Name::new(&self.uri, &self.local);
        match &self.prefix {
            Some(p) => n.with_prefix(p),
            None => n,
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Anything usable as a name in writer calls. Sealed. Implemented for
/// [`Name`], `&OwnedName`, `&str` (a local name in no namespace) and
/// `(&str, &str)` as `(uri, local)`.
///
/// ```
/// use exi::{AsName, Name, OwnedName};
/// assert_eq!("note".as_name(), Name::local("note"));
/// assert_eq!(("urn:x", "note").as_name(), Name::new("urn:x", "note"));
/// let owned = OwnedName::new("urn:x", "note");
/// assert_eq!((&owned).as_name(), Name::new("urn:x", "note"));
/// ```
pub trait AsName: sealed::Sealed {
    fn as_name(&self) -> Name<'_>;
}

impl sealed::Sealed for Name<'_> {}
impl sealed::Sealed for &OwnedName {}
impl sealed::Sealed for &str {}
impl sealed::Sealed for (&str, &str) {}

impl AsName for Name<'_> {
    fn as_name(&self) -> Name<'_> {
        *self
    }
}
impl AsName for &OwnedName {
    fn as_name(&self) -> Name<'_> {
        OwnedName::as_name(self)
    }
}
impl AsName for &str {
    fn as_name(&self) -> Name<'_> {
        Name::local(self)
    }
}
impl AsName for (&str, &str) {
    fn as_name(&self) -> Name<'_> {
        Name::new(self.0, self.1)
    }
}
