//! Errors. A stand-in for the core `error.rs`: the full `ErrorKind` list, and an `Error`
//! without a stream position, which the reader and writer add later.
//!
//! The string tables return three kinds:
//!
//! | Kind | Meaning |
//! |---|---|
//! | [`ErrorKind::Limit`] | A configured limit would be exceeded. Nothing was changed |
//! | [`ErrorKind::Arg`] | An argument is out of range or inconsistent, such as a URI number the session does not have |
//! | [`ErrorKind::State`] | The call is not allowed in the object's current state |

use core::fmt;

/// `Result` with [`Error`] as the default error type.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// The broad category of an error. Maps one-to-one onto the C status codes.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Arg, State, NoMem, Io, Header, Grammar, Value, Truncated, Limit, Schema,
    Unsupported, Xml, BufferFull, Aborted, Internal, Unencodable,
}

/// An error: a kind and a fixed description of the specific cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    detail: &'static str,
}

impl Error {
    /// A new error. Stand-in: implemented, not part of the red set.
    pub const fn new(kind: ErrorKind, detail: &'static str) -> Self {
        Error { kind, detail }
    }
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }
    pub const fn detail(&self) -> &'static str {
        self.detail
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl core::error::Error for Error {}
