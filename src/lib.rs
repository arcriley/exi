// src/lib.rs
//
// Rust library
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The doc comments are the specification the bodies are written from;
//! each item's examples are its unit tests.
//!
//! # Scope of this build
//!
//! | Included | Later |
//! |---|---|
//! | Header and options documents | |
//! | Bit-packed (`bit-packed`) and byte-aligned bodies | |
//! | Pre-compression and DEFLATE compression (`compression`, through flate2) | |
//! | Schema-less streams, built-in grammars that learn | |
//! | Schema-informed streams, with or without `strict` | |
//! | Self-contained elements, fragments, frames | EXI Profile, datatype maps |
//! | Canonical EXI (`canonical`, [`WriterConfig::canonical`]) | |
//! | `.exig` grammar files (feature `exig`) | |
//! | The XSD compiler (`xsd`) | |
//! | Schemas learned from example documents (`infer`) | |
//! | Layer 2: the document tree, after libxml2's (`dom`) | |
//! | HTML pages in and out (`html`, `html5ever`), the XHTML5 profile (`xhtml5`) | |
//! | String tables shared across sessions | |
//! | XML 1.0 text reader and writer (`xml`) | |
//! | The C ABI (`capi`), with the DOM in C (`include/exi_dom.h`, `include/exi_libxml_compat.h`) | |
//! | The `exi` command-line tool | |
//!
//! # The `exi` tool
//!
//! `src/bin/exi.rs` wraps the crate: `exi encode`, `decode`, `info`,
//! `compile`, `infer`, `exig`, `digest`, `xhtml`, `xhtml5` and
//! `features`. It is built with the crate's features, and a command
//! that needs a feature the build lacks says which one. `exi help`
//! lists the commands.
//!
//! ```text
//! cargo build --release --features infer,xhtml5,html5ever
//! exi encode logo.svg | zstd -19 > logo.svg.exi.zst
//! exi encode --html --xhtml5 page.html -o page.xhtml.exi
//! exi decode --html page.xhtml.exi
//! ```
//!
//! # Benchmarks
//!
//! `cargo bench --features infer` times the main paths;
//! `benches/compare.sh` compares sizes and times with other XML and EXI
//! tools, and HTML pages (`.html`) with their XHTML5 EXI.
//!
//! # Modules
//!
//! | Module | Holds |
//! |---|---|
//! | [`error`], [`limits`] | errors and limits for hostile input |
//! | [`name`], [`value`], [`event`] | the data model |
//! | [`stream`] | event sources, sinks, push handlers, `transcode` |
//! | [`options`], [`header`] | options and the stream header |
//! | [`coding`] | sinks, bit and byte coders, datatype and name coding |
//! | [`strings`] | shared string storage |
//! | [`grammar`] | built-in and schema-informed grammars, `.exig` files |
//! | [`reader`], [`writer`] | EXI in and out |
//! | [`xml`] | XML text in and out (feature `xml`) |
//! | `xsd` | XSD documents to grammars (feature `xsd`) |
//! | `infer` | schemas learned from example documents (feature `infer`) |
//! | `dom` | layer 2: the document tree, after libxml2's (feature `dom`) |
//! | `html` | HTML pages as EXI events and back (features `html`, `html5ever`, `xhtml5`) |
//! | `capi` | the C ABI (feature `capi`) |
//!
//! # A round trip
//!
//! ```
//! # #[cfg(feature = "xml")] {
//! use exi::{transcode, Coding, Header, Options, Reader, ReaderConfig, Writer, WriterConfig};
//! use exi::grammar::GrammarSet;
//! use exi::strings::{GlobalConfig, GlobalStrings};
//! use exi::xml::{XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
//!
//! let strings = GlobalStrings::new(GlobalConfig::new())?;
//! let set = GrammarSet::schemaless();
//! let grammars = set.attach(&strings)?;
//! let xml = r#"<p xmlns="http://www.w3.org/1999/xhtml">a <b>b</b></p>"#;
//!
//! let mut text: XmlReader = XmlReader::new(XmlConfig::new(), &grammars);
//! text.feed(xml.as_bytes());
//! text.end_input();
//! let opts = Options::builder().coding(Coding::ByteAligned).build()?;
//! let mut exi = Writer::new(Vec::new(), WriterConfig::new(opts, &grammars))?;
//! transcode(&mut text, &mut exi)?;
//! let bytes = exi.into_sink();
//!
//! let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
//! r.feed(&bytes);
//! r.end_input();
//! let mut out = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
//! transcode(&mut r, &mut out)?;
//! assert_eq!(String::from_utf8(out.into_sink()).unwrap(), xml);   // whitespace kept
//! # }
//! # Ok::<(), exi::Error>(())
//! ```

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub(crate) mod canonical;
#[cfg(feature = "capi")]
pub mod capi;
pub mod coding;
#[cfg(feature = "dom")]
pub mod dom;
mod engine;
pub mod error;
pub mod event;
pub mod grammar;
pub mod header;
#[cfg(feature = "html")]
pub mod html;
#[cfg(feature = "infer")]
pub mod infer;
pub(crate) mod input;
mod lexical;
pub mod limits;
pub mod name;
pub mod options;
pub mod reader;
pub mod stream;
pub mod strings;
pub(crate) mod tables;
pub mod value;
pub mod writer;
#[cfg(feature = "xml")]
pub mod xml;
#[cfg(feature = "xsd")]
pub mod xsd;

pub use error::{Error, ErrorKind, Position, Result};
pub use event::{
    Doctype, Event, EventKind, EventMeta, NsDecl, OwnedEvent,
};
pub use header::Header;
pub use limits::Limits;
pub use name::{AsName, GlobalName, Name, NameId, OwnedName};
pub use options::{Coding, Options, OptionsBuilder, Preserve, SchemaId};
pub use reader::{Reader, ReaderConfig, Sequence};
pub use stream::{
    EventSink, EventSource, Status, Step,
    transcode,
};
pub use value::{
    BinaryKind, DateTime, DateTimeKind, Decimal, Float, IntoValue, ListRef,
    OwnedValue, Value, ValueFlags,
};
pub use writer::{DeviationKind, Deviations, Writer, WriterConfig};
