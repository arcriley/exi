// src/html/profile.rs
//
// Shared XHTML5 coding schema (feature `xhtml5`)
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! A released version is a frozen coding schema, `urn:libexi:xhtml5:N`
//! (Appendix D). Its grammars ship with the library as an `.exig`, so
//! encoding and decoding pages needs no XSD compiler, and the grammar
//! digest is pinned in tests: a decoder that holds other grammars under
//! the same id must refuse them.
//!
//! | Version | Schema id | Grammars | Status |
//! |---|---|---|---|
//! | 1 | `urn:libexi:xhtml5:1` | `schemas/xhtml5-1.xsd`, `schemas/xhtml5-1.exig` | draft: may change until released |
//!
//! Each version fixes its options: byte-aligned, not strict (a page
//! with anything the schema does not declare still encodes),
//! `valuePartitionCapacity` 8192 (a bound on a decoder's value table),
//! nothing preserved. No `valueMaxLength`: with one of 128 (the
//! first draft's), long values that repeat (license texts, SVG path
//! data) are written again each time, and pages grew by up to 7 times.
//! Over HTTP the media type carries the version (`application/exi;
//! schema="urn:libexi:xhtml5:1"`), so a page's header is the single
//! octet `0x80`.
//!
//! ```
//! # #[cfg(feature = "html5ever")] {
//! use exi::html::profile;
//! let page = "<!doctype html><title>Hi</title><p class=intro>Hello <a href=/x>there</a>";
//! let exi = profile::encode(page, 1)?;
//! assert_eq!(exi[0], 0x80);                       // the bare header
//! assert_eq!(profile::decode(&exi, 1)?,
//!     r#"<!DOCTYPE html><html><head><title>Hi</title></head><body><p class="intro">Hello <a href="/x">there</a></p></body></html>"#);
//! assert_eq!(profile::media_type(1).as_deref(), Some(r#"application/exi; schema="urn:libexi:xhtml5:1""#));
//! # }
//! # Ok::<(), exi::Error>(())
//! ```

use std::string::String;
use std::sync::OnceLock;
#[cfg(feature = "html5ever")]
use std::vec::Vec;

use crate::grammar::GrammarSet;
use crate::{Coding, Error, ErrorKind, Options, Result, SchemaId};

/// The newest version.
pub const LATEST: u32 = 1;

/// Version 1's schema id.
pub const SCHEMA_V1: &str = "urn:libexi:xhtml5:1";

/// The SHA-256 of version 1's `.exig`: the grammars every
/// implementation must hold under [`SCHEMA_V1`].
pub const DIGEST_V1: &str =
    "sha256:f19fdbf13de843e287fc30dc57ca8b1066f5260b179374452fa005f535c50b28";

/// Version 1's coding schema, as XSD.
pub const XSD_V1: &str = include_str!("../../schemas/xhtml5-1.xsd");

/// Version 1's grammars, as `.exig`.
pub const EXIG_V1: &[u8] = include_bytes!("../../schemas/xhtml5-1.exig");

/// The schema id of a version.
pub fn schema_id(version: u32) -> Option<&'static str> {
    match version {
        1 => Some(SCHEMA_V1),
        _ => None,
    }
}

/// The version a schema id names.
pub fn version_of(schema_id: &str) -> Option<u32> {
    match schema_id {
        SCHEMA_V1 => Some(1),
        _ => None,
    }
}

/// The media type of pages in a version.
pub fn media_type(version: u32) -> Option<String> {
    schema_id(version)
        .map(|id| std::format!("application/exi; schema=\"{id}\""))
}

/// A version's grammars, loaded once from the `.exig` built in.
pub fn grammars(version: u32) -> Option<&'static GrammarSet> {
    static V1: OnceLock<GrammarSet> = OnceLock::new();
    match version {
        1 => Some(V1.get_or_init(|| {
            GrammarSet::from_exig(EXIG_V1)
                .expect("the built-in XHTML5 v1 grammars load")
        })),
        _ => None,
    }
}

/// Grammars for a schema id from a stream's header, if it names a
/// profile version.
pub fn resolve(schema_id: &str) -> Option<&'static GrammarSet> {
    grammars(version_of(schema_id)?)
}

/// A version's options. The schema id is in them, for headers that
/// carry options.
pub fn options(version: u32) -> Result<Options> {
    let id = schema_id(version).ok_or_else(|| {
        Error::new(ErrorKind::Arg, "no such XHTML5 profile version")
    })?;
    Options::builder()
        .coding(Coding::ByteAligned)
        .strict(false)
        .value_partition_capacity(Some(8192))
        .schema_id(SchemaId::Id(id.into()))
        .build()
}

/// Encodes an HTML page with a profile version, with the bare header
/// `0x80`.
#[cfg(feature = "html5ever")]
pub fn encode(html: &str, version: u32) -> Result<Vec<u8>> {
    encode_with(html, version, crate::Header::Bare { cookie: false })
}

/// As [`encode`], with another header (`Header::WithOptions` for a page
/// kept outside HTTP: the header then names the version).
#[cfg(feature = "html5ever")]
pub fn encode_with(
    html: &str,
    version: u32,
    header: crate::Header,
) -> Result<Vec<u8>> {
    use crate::strings::{GlobalConfig, GlobalStrings};
    use crate::{Writer, WriterConfig};
    let set = grammars(version).ok_or_else(|| {
        Error::new(ErrorKind::Arg, "no such XHTML5 profile version")
    })?;
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(options(version)?, &g).header(header),
    )?;
    super::encode_html(html, &mut w, &super::HtmlOptions::new())?;
    Ok(w.into_sink())
}

/// Decodes a page of a profile version to HTML text. A header with
/// options must name the same version.
pub fn decode(exi: &[u8], version: u32) -> Result<String> {
    use crate::ReaderConfig;
    use crate::strings::{GlobalConfig, GlobalStrings};
    let set = grammars(version).ok_or_else(|| {
        Error::new(ErrorKind::Arg, "no such XHTML5 profile version")
    })?;
    if let Some(h) = crate::header::parse_header(exi, &crate::Limits::new())? {
        if let Some(o) = h.options {
            if !matches!(o.schema_id(), SchemaId::Id(id) if version_of(id) == Some(version))
            {
                return Err(Error::new(
                    ErrorKind::Schema,
                    "the stream names another schema than this profile version",
                ));
            }
        }
    }
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    super::writer::decode_with(
        exi,
        ReaderConfig::new(&g).options(options(version)?),
    )
}

/// What a page needs that the version's schema does not declare: the
/// deviations its encoding counts (Appendix D.7). Merge them over many
/// pages to propose the next version.
///
/// ```
/// # #[cfg(feature = "html5ever")] {
/// use exi::html::profile;
/// use exi::DeviationKind;
/// let d = profile::report("<p data-x=1>a<my-widget>b</my-widget>", 1)?;
/// let seen: Vec<_> = d.iter().map(|(k, _, n, c)| (k, n.to_string(), c)).collect();
/// assert!(seen.contains(&(DeviationKind::Element, "{http://www.w3.org/1999/xhtml}my-widget".into(), 1)));
/// assert!(seen.contains(&(DeviationKind::Attribute, "{}data-x".into(), 1)));
/// # }
/// # Ok::<(), exi::Error>(())
/// ```
#[cfg(feature = "html5ever")]
pub fn report(html: &str, version: u32) -> Result<crate::Deviations> {
    use crate::strings::{GlobalConfig, GlobalStrings};
    use crate::{Writer, WriterConfig};
    let set = grammars(version).ok_or_else(|| {
        Error::new(ErrorKind::Arg, "no such XHTML5 profile version")
    })?;
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let g = set.attach(&strings)?;
    let cfg = WriterConfig::new(options(version)?, &g)
        .header(crate::Header::Bare { cookie: false })
        .count_deviations(true);
    let mut w = Writer::new(Vec::new(), cfg)?;
    super::encode_html(html, &mut w, &super::HtmlOptions::new())?;
    Ok(w.deviations().cloned().unwrap_or_default())
}
