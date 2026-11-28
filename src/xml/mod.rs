// src/xml/mod.rs
//
// XML 1.0 event reader and writer
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! libexi has its own XML parser so that every XML attack class is
//! handled in one place, under the same [`Limits`](crate::Limits) as EXI input, and so that XML text to EXI is
//! one loop: `transcode(&mut xml_reader, &mut exi_writer)`.
//!
//! The reader shares the engine's parts: the input ring, the global
//! string tables and session tables (names get `NameId`s and
//! `GlobalName`s), `Event`, `Position` and `Limits`. Its contract for
//! partial input is the EXI reader's: complete tokens are returned at
//! once, a partial token waits, and `NeedInput` comes in between.

pub(crate) mod chars;
pub(crate) mod scan;
pub mod reader;
pub mod writer;

pub use reader::{EntityPolicy, XmlConfig, XmlReader};
pub use writer::{PrefixPolicy, XmlWriter, XmlWriterConfig};
