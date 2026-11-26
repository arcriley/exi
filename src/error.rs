// src/error.rs
//
// Errors
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Every fallible function returns [`Result`]. An [`Error`] carries an
//! [`ErrorKind`], a [`Position`] and a fixed description of the
//! specific cause. `ErrorKind` maps one-to-one onto the C status codes
//! (`exi::capi`), so the stable ABI stays small; specific reasons
//! travel in [`Error::detail`].
//!
//! # Fatal and non-fatal errors (A5, D23)
//!
//! After an error, a reader or writer is **poisoned**: every later call
//! except dropping it returns `ErrorKind::State`. The one exception is
//! `ErrorKind::Unencodable` from a writer: the event fits no production
//! of a strict grammar, nothing was written and nothing changed, so the
//! writer stays usable and the caller skips that event.
//! [`Error::is_fatal`] tells the two apart.
//!
//! Objects that are not readers or writers (global tables, session
//! tables, options builders) are never poisoned: on error they change
//! nothing.

use core::fmt;

/// `Result` with [`Error`] as the default error type.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// The broad category of an error.
///
/// | Kind | Meaning | C status |
/// |---|---|---|
/// | `Arg` | An argument is out of range or inconsistent | `EXI_E_ARG` (−1) |
/// | `State` | The call is not allowed now: the object is poisoned, or the call is out of order | `EXI_E_STATE` (−2) |
/// | `NoMem` | An allocation failed | `EXI_E_NOMEM` (−3) |
/// | `Io` | The sink or an I/O adapter failed | `EXI_E_IO` (−4) |
/// | `Header` | The EXI header is malformed, or no options were given in the stream or out of band | `EXI_E_HEADER` (−5) |
/// | `Grammar` | The input does not follow the grammar: an event code out of range, an event in the wrong place | `EXI_E_GRAMMAR` (−6) |
/// | `Value` | A value is malformed or out of range for its datatype | `EXI_E_VALUE` (−7) |
/// | `Truncated` | Input ended inside an event after `end_input` | `EXI_E_TRUNCATED` (−8) |
/// | `Limit` | A [`Limits`](crate::Limits) or table limit would be exceeded | `EXI_E_LIMIT` (−9) |
/// | `Schema` | The grammars do not match the stream, or are malformed | `EXI_E_SCHEMA` (−10) |
/// | `Unsupported` | A valid feature this build does not implement, such as bit-packed bodies | `EXI_E_UNSUPPORTED` (−11) |
/// | `Xml` | XML text is not well-formed | `EXI_E_XML` (−12) |
/// | `BufferFull` | A fixed output buffer is full | `EXI_E_BUFFER_FULL` (−15) |
/// | `Aborted` | A handler callback asked to stop | `EXI_E_ABORTED` (−14) |
/// | `Internal` | A bug: a caught panic or a broken invariant | `EXI_E_INTERNAL` (−13) |
/// | `Unencodable` | Writer only: the event fits no production; nothing was written; not fatal | `EXI_E_UNENCODABLE` (−16) |
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Arg,
    State,
    NoMem,
    Io,
    Header,
    Grammar,
    Value,
    Truncated,
    Limit,
    Schema,
    Unsupported,
    Xml,
    BufferFull,
    Aborted,
    Internal,
    Unencodable,
}

/// Where an error happened. Fields that do not apply are 0.
///
/// | Field | Meaning |
/// |---|---|
/// | `byte`, `bit` | EXI input: the offset of the event code or value that failed, as a byte offset from the start of the stream and a bit within that byte (0 = most significant). The bit is always 0 in byte-aligned bodies |
/// | `event` | The index of the event being decoded or encoded, counting from 0 at SD |
/// | `line`, `column` | XML text input: 1-based line and column, in characters, of the token that failed |
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Position {
    pub byte: u64,
    pub bit: u8,
    pub event: u64,
    pub line: u32,
    pub column: u32,
}

/// An error: a kind, a position and a fixed description of the specific
/// cause.
///
/// ```
/// use exi::{Error, ErrorKind, Position};
/// let e = Error::new(ErrorKind::Limit, "max_depth")
///     .at(Position { event: 7, ..Position::default() });
/// assert_eq!((e.kind(), e.detail(), e.position().event), (ErrorKind::Limit, "max_depth", 7));
/// assert!(e.is_fatal());
/// assert!(!Error::new(ErrorKind::Unencodable, "undeclared element").is_fatal());
/// assert_eq!(e.to_string(), "Limit: max_depth (event 7)");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    position: Position,
    detail: &'static str,
}

impl Error {
    /// A new error at position zero.
    ///
    /// ```
    /// use exi::{Error, ErrorKind, Position};
    /// let error = Error::new(ErrorKind::Limit, "max_depth");
    /// assert_eq!(error.position(), Position::default());
    /// ```
    pub const fn new(kind: ErrorKind, detail: &'static str) -> Self {
        Error {
            kind,
            position: Position {
                byte: 0,
                bit: 0,
                event: 0,
                line: 0,
                column: 0,
            },
            detail,
        }
    }
    /// The same error at `p`.
    ///
    /// ```
    /// use exi::{Error, ErrorKind, Position};
    /// let error = Error::new(ErrorKind::Value, "invalid character");
    /// let pos = Position { byte: 2, ..Position::default() };
    /// assert_eq!(error.at(pos).position(), pos);
    /// ```
    pub const fn at(self, p: Position) -> Self {
        Error { position: p, ..self }
    }
    ///
    /// ```
    /// use exi::{Error, ErrorKind, Position};
    /// assert_eq!(Error::new(ErrorKind::Limit, "max_depth").kind(), ErrorKind::Limit);
    /// ```
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }
    ///
    /// ```
    /// use exi::{Error, ErrorKind, Position};
    /// let pos = Position { event: 9, ..Position::default() };
    /// assert_eq!(Error::new(ErrorKind::Grammar, "bad code").at(pos).position().event, 9);
    /// ```
    pub const fn position(&self) -> Position {
        self.position
    }
    ///
    /// ```
    /// use exi::{Error, ErrorKind, Position};
    /// assert_eq!(Error::new(ErrorKind::Value, "invalid character").detail(), "invalid character");
    /// ```
    pub const fn detail(&self) -> &'static str {
        self.detail
    }
    /// False only for `ErrorKind::Unencodable` (D23).
    ///
    /// ```
    /// use exi::{Error, ErrorKind, Position};
    /// assert!(!Error::new(ErrorKind::Unencodable, "undeclared attribute").is_fatal());
    /// assert!(Error::new(ErrorKind::Grammar, "bad code").is_fatal());
    /// ```
    pub const fn is_fatal(&self) -> bool {
        !matches!(self.kind, ErrorKind::Unencodable)
    }
}

/// `"{kind:?}: {detail}"`, followed by the first of these that applies:
///
/// 1. `line` is not 0: `" (line L, column C)"`;
/// 2. `byte` or `bit` is not 0: `" (byte B bit b, event E)"`;
/// 3. `event` is not 0: `" (event E)"`;
/// 4. otherwise nothing.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)?;
        let p = &self.position;
        if p.line != 0 {
            write!(f, " (line {}, column {})", p.line, p.column)
        } else if p.byte != 0 || p.bit != 0 {
            write!(f, " (byte {} bit {}, event {})", p.byte, p.bit, p.event)
        } else if p.event != 0 {
            write!(f, " (event {})", p.event)
        } else {
            Ok(())
        }
    }
}

impl core::error::Error for Error {}
