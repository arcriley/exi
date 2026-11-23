//! libexi: W3C EXI 1.0 in Rust.
//!
//! This skeleton holds one component: the string tables. Every function body is `todo!()`,
//! so every example below compiles and then fails. That is the red step of test-driven
//! development: an item is done when its examples pass.
//!
//! - [`strings`]: the global tables, one per kind of string, shared by every session in a
//!   process. A string is stored once and identified by an integer id.
//! - [`tables`]: the string tables of one EXI stream, built on the global tables.
//!
//! [`error`], [`limits`] and [`name`] are stand-ins holding only what the tables need. The
//! rest of each arrives with the reader and writer.
//!
//! The crate is `no_std` and needs a heap (`alloc`). The `std` feature adds nothing to the
//! string tables yet.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]
// Skeleton phase: every body is `todo!()`. Remove these two lines when the bodies are written.
#![allow(unused_variables, dead_code)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod error;
pub mod limits;
pub mod name;
pub mod strings;
pub mod tables;

pub use error::{Error, ErrorKind, Result};
pub use limits::Limits;
pub use name::NameId;
