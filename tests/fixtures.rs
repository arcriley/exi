// tests/fixtures.rs
//
// EXIficient interoperability tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Compares this crate with EXIficient: the fixtures in `fixtures/exificient` were written by
//! EXIficient 1.0.7 from `fixtures/input` (see `fixtures/MakeFixtures.java`).
//!
//! For each fixture, the test encodes the same input and expects the same octets, and
//! decodes EXIficient's octets and expects the input's events (schema-informed fixtures:
//! writes the decoded events again and expects EXIficient's octets). Compressed bodies are
//! compared by the octets their DEFLATE streams inflate to, since DEFLATE output depends
//! on the compressor.
#![cfg(feature = "xml")]

use std::path::Path;

use exi::grammar::{GrammarSet, Grammars};
use exi::header::{parse_header, write_header};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{
    Coding, Event, EventSink, Header, Limits, Options, OwnedEvent, Preserve, Reader, ReaderConfig,
    Result, Step, Writer, WriterConfig,
};

struct Fixture {
    name: String,
    opts: Vec<String>,
    input: String,
    schema: Option<String>,
}

fn manifest() -> Vec<Fixture> {
    let text = std::fs::read_to_string("fixtures/manifest.tsv").unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Fixture {
                name: f[0].into(),
                opts: f[1].split(',').map(String::from).collect(),
                input: f[2].into(),
                schema: f.get(3).filter(|s| **s != "-").map(|s| s.to_string()),
            }
        })
        .collect()
}

fn options(f: &Fixture) -> Options {
    let has = |o: &str| f.opts.iter().any(|x| x == o);
    let num = |k: &str| {
        f.opts.iter().find_map(|x| {
            x.strip_prefix(k)
                .and_then(|v| v.strip_prefix('='))
                .map(|v| v.parse::<u32>().unwrap())
        })
    };
    let coding = if has("byte") {
        Coding::ByteAligned
    } else if has("pre") {
        Coding::PreCompression
    } else if has("compression") {
        Coding::Compression
    } else {
        Coding::BitPacked
    };
    let mut p = Preserve::empty();
    for (o, flag) in [
        ("prefixes", Preserve::PREFIXES),
        ("comments", Preserve::COMMENTS),
        ("pis", Preserve::PIS),
        ("dtd", Preserve::DTD),
        ("lexical", Preserve::LEXICAL_VALUES),
    ] {
        if has(o) {
            p |= flag;
        }
    }
    let mut b = Options::builder()
        .coding(coding)
        .strict(has("strict"))
        .fragment(has("fragment"))
        .self_contained(has("sc"))
        .preserve(p);
    if let Some(n) = num("block") {
        b = b.block_size(n);
    }
    b = b
        .value_max_length(num("vml"))
        .value_partition_capacity(num("vpc"));
    b.build().unwrap()
}

fn header(f: &Fixture) -> Header {
    let cookie = f.opts.iter().any(|o| o == "cookie");
    // Canonical EXI sends the options document unless omitOptionsDocument is set, which
    // EXIficient does not offer.
    let canonical = f.opts.iter().any(|o| o == "canonical");
    if canonical || f.opts.iter().any(|o| o == "options") {
        Header::WithOptions { cookie }
    } else {
        Header::Bare { cookie }
    }
}

/// Drops the outermost element: the wrapper of a fragment's top-level elements.
struct DropRoot<'s, S: EventSink> {
    inner: &'s mut S,
    depth: u32,
}

impl<S: EventSink> EventSink for DropRoot<'_, S> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        match ev {
            Event::StartElement(_) => {
                self.depth += 1;
                if self.depth == 1 {
                    return Ok(());
                }
            }
            Event::EndElement(_) => {
                self.depth -= 1;
                if self.depth == 0 {
                    return Ok(());
                }
            }
            // The wrapper's own whitespace and namespace declarations.
            Event::Namespace(_) | Event::Attribute(..) | Event::Characters(_)
                if self.depth == 1 =>
            {
                return Ok(());
            }
            _ => {}
        }
        self.inner.event(ev)
    }
}

fn xml_events(g: &Grammars<'_>, xml: &[u8], fragment: bool, opts: &Options) -> Vec<OwnedEvent> {
    let cfg = XmlConfig::new()
        .report_comments(opts.preserve().contains(Preserve::COMMENTS))
        .report_pis(opts.preserve().contains(Preserve::PIS));
    let mut r: XmlReader = XmlReader::new(cfg, g);
    let mut rest = xml;
    let mut out = Vec::new();
    let mut depth = 0;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(e, _) => {
                    let skip = fragment
                        && match e {
                            Event::StartElement(_) => {
                                depth += 1;
                                depth == 1
                            }
                            Event::EndElement(_) => {
                                depth -= 1;
                                depth == 0
                            }
                            Event::Namespace(_) | Event::Attribute(..) | Event::Characters(_) => {
                                depth == 1
                            }
                            _ => false,
                        };
                    // Namespace declarations travel only with prefixes preserved.
                    let ns = matches!(e, Event::Namespace(_))
                        && !opts.preserve().contains(Preserve::PREFIXES);
                    if !skip && !ns {
                        out.push(e.to_owned());
                    }
                }
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

fn encode(g: &Grammars<'_>, f: &Fixture, xml: &[u8]) -> Result<Vec<u8>> {
    let opts = options(f);
    let mut x: XmlReader = XmlReader::new(XmlConfig::new(), g);
    let mut w = Writer::new(Vec::new(), writer_config(f, &opts, g))?;
    fn pump<S: EventSink>(x: &mut XmlReader<'_>, sink: &mut S, xml: &[u8]) -> Result<()> {
        let mut rest = xml;
        loop {
            rest = &rest[x.feed(rest)..];
            if rest.is_empty() {
                x.end_input();
            }
            if exi::transcode(x, sink)? == exi::Status::End {
                return Ok(());
            }
        }
    }
    if opts.fragment() {
        pump(
            &mut x,
            &mut DropRoot {
                inner: &mut w,
                depth: 0,
            },
            xml,
        )?;
    } else {
        pump(&mut x, &mut w, xml)?;
    }
    Ok(w.into_sink())
}

fn decode(g: &Grammars<'_>, f: &Fixture, exi: &[u8]) -> Result<Vec<OwnedEvent>> {
    let mut cfg = ReaderConfig::new(g);
    if !f.opts.iter().any(|o| o == "options") {
        cfg = cfg.options(options(f));
    }
    let mut r: Reader = Reader::new(cfg)?;
    let mut rest = exi;
    let mut out = Vec::new();
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next()? {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return Ok(out),
            }
        }
    }
}

/// What a stream can carry of the input's events: prefixes only when preserved, and an
/// `xsi:type` value as a name (EXI codes it as a QName, so its text prefix is gone).
fn normalize(evs: Vec<OwnedEvent>, prefixes: bool) -> Vec<OwnedEvent> {
    use exi::{OwnedName, OwnedValue};
    let name = |n: OwnedName| -> OwnedName {
        let o = OwnedName::new(n.as_name().uri(), n.as_name().local_name());
        match n.as_name().prefix() {
            Some(p) if prefixes && !p.is_empty() => o.with_prefix(p),
            _ => o,
        }
    };
    let value = |n: &OwnedName, v: OwnedValue| -> OwnedValue {
        if !n
            .as_name()
            .is("http://www.w3.org/2001/XMLSchema-instance", "type")
        {
            return v;
        }
        match v {
            OwnedValue::QName(q) => OwnedValue::Str(q.as_name().local_name().into()),
            OwnedValue::Str(s) => OwnedValue::Str(s.rsplit(':').next().unwrap().into()),
            v => v,
        }
    };
    evs.into_iter()
        .map(|e| match e {
            OwnedEvent::StartElement(n) => OwnedEvent::StartElement(name(n)),
            OwnedEvent::EndElement(n) => OwnedEvent::EndElement(name(n)),
            OwnedEvent::Attribute(n, v) => {
                let v = value(&n, v);
                OwnedEvent::Attribute(name(n), v)
            }
            e => e,
        })
        .collect()
}

/// Compiles `fixtures/input/<main>` with every schema document there available to it.
#[cfg(feature = "xsd")]
/// Compiles `main` (relative to `fixtures/input`) with every `.xsd` beside it.
fn compile_schema(dir: &Path, main: &str) -> Result<GrammarSet> {
    let main_path = Path::new(main);
    let sub = main_path.parent().unwrap_or_else(|| Path::new(""));
    let main = main_path.file_name().unwrap().to_str().unwrap();
    let mut docs = Vec::new();
    for e in std::fs::read_dir(dir.join("input").join(sub)).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "xsd") {
            docs.push((
                p.file_name().unwrap().to_string_lossy().to_string(),
                std::fs::read(&p).unwrap(),
            ));
        }
    }
    let mut c = exi::xsd::XsdCompiler::new();
    for (name, bytes) in &docs {
        c = c.document(name, bytes);
    }
    c.compile(main)
}

/// The raw DEFLATE streams that follow one another in `body`, each inflated; `None` when
/// the body is not a sequence of whole streams.
fn inflate_streams(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let mut z = flate2::Decompress::new(false);
        let mut v = Vec::with_capacity(1 << 20);
        loop {
            let before = z.total_in();
            let st = z
                .decompress_vec(
                    &body[at + before as usize..],
                    &mut v,
                    flate2::FlushDecompress::Finish,
                )
                .ok()?;
            if st == flate2::Status::StreamEnd {
                break;
            }
            if z.total_in() == before {
                if v.len() == v.capacity() {
                    v.reserve(v.capacity());
                    continue;
                }
                return None;
            }
        }
        at += z.total_in() as usize;
        out.push(v);
    }
    Some(out)
}

/// A body for a failure message: hex, or for compression each stream inflated, in hex.
fn shown(b: &[u8], hlen: usize, opts: &Options) -> String {
    if opts.coding() != Coding::Compression {
        return hex(b);
    }
    match inflate_streams(&b[hlen.min(b.len())..]) {
        Some(streams) => streams
            .iter()
            .map(|s| format!("[{}]", hex(s)))
            .collect::<Vec<_>>()
            .join(" "),
        None => format!("(not DEFLATE streams) {}", hex(b)),
    }
}

/// The writer's configuration: the header, and Canonical EXI with `canonical` and `utc`.
fn writer_config<'g>(f: &Fixture, opts: &Options, g: &'g Grammars<'g>) -> WriterConfig<'g> {
    let has = |o: &str| f.opts.iter().any(|x| x == o);
    WriterConfig::new(opts.clone(), g)
        .header(header(f))
        .canonical(has("canonical"))
        .utc_time(has("utc"))
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn exificient_fixtures() {
    check(false);
}

#[cfg(feature = "xsd")]
#[test]
fn exificient_schema_fixtures() {
    check(true);
}

/// Every fixture without a schema, or (`schemas`) every one with a schema, compiled by
/// [`GrammarSet::from_xsd`].
fn check(schemas: bool) {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let schemaless = GrammarSet::schemaless();
    let dir = Path::new("fixtures");
    let mut failures = Vec::new();
    let mut checked = 0;
    for f in manifest() {
        if f.schema.is_some() != schemas || f.opts.iter().any(|o| o == "grammars" || o == "decode")
        {
            continue;
        }
        #[cfg(feature = "xsd")]
        let compiled = match &f.schema {
            Some(x) => match compile_schema(dir, x) {
                Ok(set) => Some(set),
                Err(e) => {
                    failures.push(format!("{}: schema error {e}", f.name));
                    continue;
                }
            },
            None => None,
        };
        #[cfg(not(feature = "xsd"))]
        let compiled: Option<GrammarSet> = None;
        let set = compiled.as_ref().unwrap_or(&schemaless);
        let g = set.attach(&strings).unwrap();
        let theirs = std::fs::read(dir.join("exificient").join(format!("{}.exi", f.name))).unwrap();
        let opts = options(&f);
        // Header and options document.
        let mut ours = Vec::new();
        write_header(&mut ours, header(&f), &opts).unwrap();
        let parsed = parse_header(&theirs, &Limits::new()).unwrap().unwrap();
        if let Some(o) = &parsed.options {
            // blockSize is sent only with compression, where it has an effect.
            let compressing = matches!(opts.coding(), Coding::Compression | Coding::PreCompression);
            let opts = if compressing {
                opts.clone()
            } else {
                opts.to_builder().block_size(1_000_000).build().unwrap()
            };
            if *o != opts {
                failures.push(format!("{}: options read {o:?}, expected {opts:?}", f.name));
            }
        }
        let hlen = (parsed.body_offset_bits / 8) as usize;
        if opts.coding() != Coding::BitPacked && theirs[..hlen] != ours[..] {
            failures.push(format!(
                "{}: header ours {} theirs {}",
                f.name,
                hex(&ours),
                hex(&theirs[..hlen])
            ));
        }
        // A coding this build lacks: the header was checked; the body cannot be.
        let built = match opts.coding() {
            Coding::ByteAligned => true,
            Coding::BitPacked => cfg!(feature = "bit-packed"),
            Coding::PreCompression | Coding::Compression => cfg!(feature = "compression"),
        };
        if !built {
            checked += 1;
            continue;
        }
        // Compressed bodies are compared by what their DEFLATE streams hold: the octets of
        // a DEFLATE stream depend on the compressor.
        let same = |ours: &[u8]| -> bool {
            if opts.coding() != Coding::Compression {
                return ours == &theirs[..];
            }
            ours.len() >= hlen
                && ours[..hlen] == theirs[..hlen]
                && inflate_streams(&ours[hlen..]) == inflate_streams(&theirs[hlen..])
        };
        // Body.
        let xml = std::fs::read(dir.join("input").join(&f.input)).unwrap();
        match encode(&g, &f, &xml) {
            Ok(ours) if same(&ours) => {}
            Ok(ours) => failures.push(format!(
                "{}: encode\n  ours   {}\n  theirs {}",
                f.name,
                shown(&ours, hlen, &opts),
                shown(&theirs, hlen, &opts)
            )),
            Err(e) => failures.push(format!("{}: encode error {e}", f.name)),
        }
        if schemas || f.opts.iter().any(|o| o == "canonical") {
            // Typed values come back typed and attributes sorted, and Canonical EXI changes
            // text, so compare by writing the decoded events again: the octets must be
            // EXIficient's.
            let again = decode(&g, &f, &theirs).and_then(|evs| {
                let mut w = Writer::new(Vec::new(), writer_config(&f, &opts, &g))?;
                for e in &evs {
                    w.event(&e.as_event())?;
                }
                Ok(w.into_sink())
            });
            match again {
                Ok(b) if same(&b) => {}
                Ok(b) => failures.push(format!(
                    "{}: decode and write again\n  ours   {}\n  theirs {}",
                    f.name,
                    shown(&b, hlen, &opts),
                    shown(&theirs, hlen, &opts)
                )),
                Err(e) => failures.push(format!("{}: decode error {e}", f.name)),
            }
            checked += 1;
            continue;
        }
        let prefixes = opts.preserve().contains(Preserve::PREFIXES);
        let expected = normalize(xml_events(&g, &xml, opts.fragment(), &opts), prefixes);
        match decode(&g, &f, &theirs).map(|e| normalize(e, prefixes)) {
            Ok(evs) if evs == expected => {}
            Ok(evs) => failures.push(format!(
                "{}: decode\n  ours     {evs:?}\n  expected {expected:?}",
                f.name
            )),
            Err(e) => failures.push(format!("{}: decode error {e}", f.name)),
        }
        checked += 1;
    }
    eprintln!("{checked} fixtures checked");
    assert!(
        failures.is_empty(),
        "{} of {checked} fixtures differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// `.exig` files are portable: EXIficient, given the EXIG schema and the XML form of an
/// EXIG document, writes the same octets this crate's `to_exig` writes.
#[cfg(all(feature = "xsd", feature = "exig"))]
#[test]
fn exig_matches_exificient() {
    let dir = Path::new("fixtures");
    for (schema, fixture) in [
        ("complex.xsd", "complex-exig-encode"),
        ("exig-1.xsd", "exig-exig-encode"),
    ] {
        let set = compile_schema(dir, schema).unwrap();
        let theirs = std::fs::read(dir.join("exificient").join(format!("{fixture}.exi"))).unwrap();
        assert_eq!(hex(&set.to_exig().unwrap()), hex(&theirs), "{schema}");
        // And their bytes load to the same grammars.
        let loaded = GrammarSet::from_exig(&theirs).unwrap();
        assert_eq!(loaded.to_exig().unwrap(), theirs);
    }
}
