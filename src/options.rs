// src/options.rs
//
// Options for EXI stream encoding
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Options travel in the stream header ([`crate::header`]) or are
//! agreed out of band. Both sides must use the same options: they
//! change the grammars, the string tables and the coding.
//!
//! | Option | Default | Effect |
//! |---|---|---|
//! | `alignment` / `compression` ([`Coding`]) | `BitPacked` | How event codes and values are laid out in octets |
//! | `strict` | off | Schema-informed grammars accept only what the schema declares, which shortens event codes |
//! | `fragment` | off | The body is a fragment: zero or more top-level elements, instead of one document element |
//! | `selfContained` | off | Elements may be self-contained (SC): coded from fresh tables, so they can be read on their own |
//! | `preserve` ([`Preserve`]) | nothing | Which kinds of event and form the stream keeps |
//! | `blockSize` | 1 000 000 | Values per compression block (compression and pre-compression only) |
//! | `valueMaxLength` | none | Longest value, in characters, added to the value string tables |
//! | `valuePartitionCapacity` | none | Most values in the global value partition; the oldest is replaced when full |
//! | `schemaId` ([`SchemaId`]) | absent | Which schema the stream is informed by |
//!
//! # Preserve flags and what they keep
//!
//! | Flag | Keeps | Without it |
//! |---|---|---|
//! | `DTD` | DOCTYPE (DT) and entity reference (ER) events | dropped |
//! | `PREFIXES` | namespace declarations (NS events) and the prefix of every name | dropped; readers generate prefixes |
//! | `LEXICAL_VALUES` | the exact text of every value: values are carried as strings | typed values are carried in their datatype and printed canonically |
//! | `COMMENTS` | comments (CM) | dropped |
//! | `PIS` | processing instructions (PI) | dropped |
//!
//! A grammar contains a production for an event kind only when the
//! options keep that kind (see [`crate::grammar`]); this is what
//! "pruning" means, and it changes event codes.
//!
//! # Combinations that are not allowed
//!
//! [`OptionsBuilder::build`] returns `ErrorKind::Arg` for:
//!
//! | Combination | Why |
//! |---|---|
//! | `strict` with `Preserve::DTD`, `PREFIXES`, `COMMENTS` or `PIS` | strict grammars have no productions for those events |
//! | `strict` with `selfContained` | strict grammars have no SC production |
//! | `selfContained` with `Compression` or `PreCompression` | a self-contained element must be byte-aligned on its own, which channels prevent |
//! | `blockSize` 0 | a block holds at least one value |
//!
//! `strict` with `LEXICAL_VALUES` is allowed.
//!
//! # What this build supports
//!
//! Every option value can be represented, read from a header and
//! written to one. Bodies are coded in every `Coding`: `BitPacked` with
//! the feature `bit-packed`, `PreCompression` and `Compression` with
//! the feature `compression` (both default); a reader or writer given a
//! coding its build lacks returns `ErrorKind::Unsupported`.
//! Options documents carrying user meta-data (for example EXI Profile
//! parameters) or a `datatypeRepresentationMap` are
//! `ErrorKind::Unsupported` for now.

use alloc::boxed::Box;

use crate::{Error, ErrorKind, Result};

/// How event codes and values are laid out.
///
/// | Coding | Layout |
/// |---|---|
/// | `BitPacked` | every value takes exactly its bits; nothing is aligned to octets |
/// | `ByteAligned` | every event-code part and value starts on an octet and takes whole octets |
/// | `PreCompression` | byte-aligned, with values regrouped into channels |
/// | `Compression` | pre-compression, then DEFLATE |
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Coding {
    #[default]
    BitPacked,
    ByteAligned,
    PreCompression,
    Compression,
}

/// Fidelity flags (module documentation).
///
/// ```
/// use exi::Preserve;
/// let p = Preserve::COMMENTS | Preserve::PIS;
/// assert!(p.contains(Preserve::PIS) && !p.contains(Preserve::DTD));
/// assert_eq!(Preserve::empty(), Preserve::default());
/// assert_eq!(Preserve::all().bits(), 0b1_1111);
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Preserve(u8);

impl Preserve {
    pub const DTD: Preserve = Preserve(1);
    pub const PREFIXES: Preserve = Preserve(2);
    pub const LEXICAL_VALUES: Preserve = Preserve(4);
    pub const COMMENTS: Preserve = Preserve(8);
    pub const PIS: Preserve = Preserve(16);
    pub const fn empty() -> Self {
        Preserve(0)
    }
    pub const fn all() -> Self {
        Preserve(0b1_1111)
    }
    pub const fn bits(self) -> u8 {
        self.0
    }
    pub const fn contains(self, other: Preserve) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for Preserve {
    type Output = Preserve;
    fn bitor(self, rhs: Preserve) -> Preserve {
        Preserve(self.0 | rhs.0)
    }
}

/// Which schema informs the stream.
///
/// | Value | In the options document | Meaning |
/// |---|---|---|
/// | `Absent` | no `schemaId` element | no statement: the reader uses the grammars it was given |
/// | `Nil` | `<schemaId xsi:nil="true"/>` | no schema: built-in grammars only |
/// | `BuiltinTypes` | `<schemaId></schemaId>` (empty) | the XML Schema built-in types only |
/// | `Id(s)` | `<schemaId>s</schemaId>`, `s` not empty | the schema `s` names; resolved by the application |
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum SchemaId {
    #[default]
    Absent,
    Nil,
    BuiltinTypes,
    Id(Box<str>),
}

/// Validated options. Build with [`Options::builder`].
///
/// ```
/// use exi::{Options, Coding, Preserve, SchemaId};
/// let o = Options::default();
/// assert_eq!((o.coding(), o.strict(), o.fragment(), o.self_contained()), (Coding::BitPacked, false, false, false));
/// assert_eq!((o.preserve(), o.block_size()), (Preserve::empty(), 1_000_000));
/// assert_eq!((o.value_max_length(), o.value_partition_capacity()), (None, None));
/// assert_eq!(o.schema_id(), &SchemaId::Absent);
/// assert!(o.is_default());
/// assert!(!Options::builder().fragment(true).build()?.is_default());
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    coding: Coding,
    strict: bool,
    fragment: bool,
    self_contained: bool,
    preserve: Preserve,
    block_size: u32,
    value_max_length: Option<u32>,
    value_partition_capacity: Option<u32>,
    schema_id: SchemaId,
}

impl Options {
    pub fn builder() -> OptionsBuilder {
        OptionsBuilder { options: Options::default() }
    }
    pub fn coding(&self) -> Coding {
        self.coding
    }
    pub fn strict(&self) -> bool {
        self.strict
    }
    pub fn fragment(&self) -> bool {
        self.fragment
    }
    pub fn self_contained(&self) -> bool {
        self.self_contained
    }
    pub fn preserve(&self) -> Preserve {
        self.preserve
    }
    pub fn block_size(&self) -> u32 {
        self.block_size
    }
    pub fn value_max_length(&self) -> Option<u32> {
        self.value_max_length
    }
    pub fn value_partition_capacity(&self) -> Option<u32> {
        self.value_partition_capacity
    }
    pub fn schema_id(&self) -> &SchemaId {
        &self.schema_id
    }
    /// True when every option has its default value: an options
    /// document for these options is empty (`<header/>`).
    pub fn is_default(&self) -> bool {
        *self == Options::default()
    }
    /// A builder starting from these options.
    pub fn to_builder(&self) -> OptionsBuilder {
        OptionsBuilder { options: self.clone() }
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            coding: Coding::BitPacked,
            strict: false,
            fragment: false,
            self_contained: false,
            preserve: Preserve::empty(),
            block_size: 1_000_000,
            value_max_length: None,
            value_partition_capacity: None,
            schema_id: SchemaId::Absent,
        }
    }
}

/// Builds [`Options`], checking the combinations in the module
/// documentation.
///
/// ```
/// use exi::{Options, Preserve, Coding, ErrorKind};
/// let bad = |b: exi::OptionsBuilder| b.build().unwrap_err().kind();
/// for p in [Preserve::DTD, Preserve::PREFIXES, Preserve::COMMENTS, Preserve::PIS] {
///     assert_eq!(bad(Options::builder().strict(true).preserve(p)), ErrorKind::Arg);
/// }
/// assert!(Options::builder().strict(true).preserve(Preserve::LEXICAL_VALUES).build().is_ok());
/// assert_eq!(bad(Options::builder().strict(true).self_contained(true)), ErrorKind::Arg);
/// assert_eq!(bad(Options::builder().self_contained(true).coding(Coding::Compression)), ErrorKind::Arg);
/// assert_eq!(bad(Options::builder().self_contained(true).coding(Coding::PreCompression)), ErrorKind::Arg);
/// assert!(Options::builder().self_contained(true).coding(Coding::ByteAligned).build().is_ok());
/// assert_eq!(bad(Options::builder().block_size(0)), ErrorKind::Arg);
/// assert!(Options::builder().block_size(1).build().is_ok());
/// assert!(Options::builder().block_size(u32::MAX).build().is_ok());
/// assert!(Options::builder().value_max_length(Some(0)).value_partition_capacity(Some(0)).build().is_ok());
/// ```
#[derive(Clone, Debug)]
pub struct OptionsBuilder {
    options: Options,
}

impl OptionsBuilder {
    pub fn coding(self, c: Coding) -> Self {
        OptionsBuilder { options: Options { coding: c, ..self.options } }
    }
    pub fn strict(self, on: bool) -> Self {
        OptionsBuilder { options: Options { strict: on, ..self.options } }
    }
    pub fn fragment(self, on: bool) -> Self {
        OptionsBuilder { options: Options { fragment: on, ..self.options } }
    }
    pub fn self_contained(self, on: bool) -> Self {
        OptionsBuilder {
            options: Options { self_contained: on, ..self.options },
        }
    }
    /// Replaces the preserve flags.
    pub fn preserve(self, p: Preserve) -> Self {
        OptionsBuilder { options: Options { preserve: p, ..self.options } }
    }
    pub fn block_size(self, n: u32) -> Self {
        OptionsBuilder { options: Options { block_size: n, ..self.options } }
    }
    pub fn value_max_length(self, n: Option<u32>) -> Self {
        OptionsBuilder {
            options: Options { value_max_length: n, ..self.options },
        }
    }
    pub fn value_partition_capacity(self, n: Option<u32>) -> Self {
        OptionsBuilder {
            options: Options { value_partition_capacity: n, ..self.options },
        }
    }
    pub fn schema_id(self, id: SchemaId) -> Self {
        OptionsBuilder { options: Options { schema_id: id, ..self.options } }
    }
    /// # Errors
    ///
    /// `ErrorKind::Arg` for a combination in the module documentation's
    /// table.
    pub fn build(self) -> Result<Options> {
        let o = self.options;
        let arg = |d| Err(Error::new(ErrorKind::Arg, d));
        let unstrict = Preserve::DTD
            | Preserve::PREFIXES
            | Preserve::COMMENTS
            | Preserve::PIS;
        if o.strict && o.preserve.0 & unstrict.0 != 0 {
            return arg(
                "strict excludes preserving DTDs, prefixes, comments and PIs",
            );
        }
        if o.strict && o.self_contained {
            return arg("strict excludes selfContained");
        }
        if o.self_contained
            && matches!(o.coding, Coding::Compression | Coding::PreCompression)
        {
            return arg(
                "selfContained excludes compression and pre-compression",
            );
        }
        if o.block_size == 0 {
            return arg("blockSize must be at least 1");
        }
        Ok(o)
    }
}
