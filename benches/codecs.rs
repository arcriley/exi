// benches/codecs.rs
//
// EXI, XML and HTML compression benchmarks
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Sizes and times of EXI codings, alone and under LZ4 and zstd, against XML and HTML text
//! under the same compressors.
//!
//!     cargo bench --bench codecs --features infer,xhtml5,html5ever
//!     cargo bench --bench codecs --features infer,xhtml5,html5ever -- --quick
//!     cargo bench --bench codecs --features infer,xhtml5,html5ever -- stanzas
//!
//! Writes, under `target/bench/`:
//!
//! | File | Rows |
//! |---|---|
//! | `codecs.csv` | one per corpus and configuration: octets, ratio to the text, encode and decode times |
//! | `stanzas.csv` | one per stanza: its size as text and under every per-stanza configuration |
//! | `pages.csv` | one per HTML page: its size under every document configuration |
//! | `codecs.meta` | the build and machine (`benches/common`) |
//!
//! # Corpora
//!
//! | Corpus | What it is |
//! |---|---|
//! | `chat`, `iot`, `commands` | synthetic XMPP sessions, 1500 stanzas each (`benches/xmpp/gen_sessions.py`); a second session of each kind, with other users and seeds, trains the zstd dictionaries and the inferred schemas |
//! | `pages` | HTML pages in `target/html-corpus` (or `EXI_HTML_CORPUS`), when present |
//! | `log` | a generated data log of 60 000 records |
//!
//! A corpus that is missing is skipped with a line that says how to make it, so check
//! the output before comparing two runs.
//!
//! # What is measured
//!
//! Encoding starts from events and ends with the octets sent; decoding starts from those
//! octets and ends with events. So the text rows include writing or parsing the text, and
//! the EXI rows writing or reading EXI; each includes its compressor. Rows for the
//! original text compress the text as it was given, not the text written, so the writer
//! is timed but its output is thrown away. Times are the median of 3 to 15 runs over the
//! whole corpus (one with `--quick`); `codecs.csv` also has the fastest and slowest.
//!
//! Per-stanza configurations encode every stanza alone: fresh string tables and a fresh
//! compressor for each, so nothing one stanza contains changes the size of another (no
//! CRIME-style oracle between stanzas). A zstd dictionary is fixed, trained on the other
//! session, and sent ahead of time, so it is not counted. Session-wide configurations
//! keep state across stanzas, as XEP-0138 zlib and XEP-0322 `sessionWideBuffers` do; they
//! are shown for comparison.
//!
//! Compressor settings: LZ4 is `lz4_flex` block format with the size in front (4 octets);
//! zstd frames carry the content size but no checksum or dictionary id; DEFLATE is raw.

mod common;

use std::fmt::Write as _;
use std::hint::black_box;
use std::io::Write as _;
use std::time::{Duration, Instant};

use common::Stats;

use exi::grammar::{GrammarSet, Grammars};
use exi::infer::{Fit, SchemaInference};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    Coding, EventSink, Header, Options, OwnedEvent, Reader, ReaderConfig, Sequence, Step, Writer,
    WriterConfig,
};

// ----- outer compressors -----

#[derive(Clone)]
enum Outer {
    None,
    Lz4,
    /// LZ4 with a fixed dictionary: the last octets of the trained zstd dictionary.
    Lz4Dict(std::sync::Arc<Vec<u8>>),
    Zstd(i32),
    /// zstd with a dictionary trained for this representation.
    ZstdDict(i32, std::sync::Arc<Vec<u8>>),
    Deflate(u32),
}

impl Outer {
    fn name(&self) -> String {
        match self {
            Outer::None => String::new(),
            Outer::Lz4 => " + lz4".into(),
            Outer::Lz4Dict(d) => format!(" + lz4 dict {}K", d.len() / 1024),
            Outer::Zstd(l) => format!(" + zstd-{l}"),
            Outer::ZstdDict(l, _) => format!(" + zstd-{l} dict"),
            Outer::Deflate(l) => format!(" + deflate-{l}"),
        }
    }
}

/// A compressor ready for many messages: zstd contexts and dictionaries are prepared once.
struct Packer {
    outer: Outer,
    zc: Option<zstd::bulk::Compressor<'static>>,
    zd: Option<zstd::bulk::Decompressor<'static>>,
}

impl Packer {
    fn new(outer: &Outer) -> Packer {
        let (zc, zd) = match outer {
            Outer::Zstd(l) => (
                Some(zstd::bulk::Compressor::new(*l).unwrap()),
                Some(zstd::bulk::Decompressor::new().unwrap()),
            ),
            Outer::ZstdDict(l, d) => (
                Some(zstd::bulk::Compressor::with_dictionary(*l, d).unwrap()),
                Some(zstd::bulk::Decompressor::with_dictionary(d).unwrap()),
            ),
            _ => (None, None),
        };
        let mut p = Packer {
            outer: outer.clone(),
            zc,
            zd,
        };
        if let Some(c) = p.zc.as_mut() {
            c.include_checksum(false).unwrap();
            c.include_dictid(false).unwrap();
            c.include_contentsize(true).unwrap();
        }
        p
    }
    /// Compresses an encoder's output; with no compressor it is passed on, not copied.
    fn pack(&mut self, data: Vec<u8>) -> Vec<u8> {
        match &self.outer {
            Outer::None => data,
            _ => self.pack_slice(&data),
        }
    }
    /// Compresses borrowed octets: the original text.
    fn pack_slice(&mut self, data: &[u8]) -> Vec<u8> {
        match &self.outer {
            Outer::None => data.to_vec(),
            Outer::Lz4 => lz4_flex::block::compress_prepend_size(data),
            Outer::Lz4Dict(d) => lz4_flex::block::compress_prepend_size_with_dict(data, d),
            Outer::Zstd(_) | Outer::ZstdDict(..) => {
                self.zc.as_mut().unwrap().compress(data).unwrap()
            }
            Outer::Deflate(l) => {
                let mut z =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(*l));
                z.write_all(data).unwrap();
                z.finish().unwrap()
            }
        }
    }
    fn unpack(&mut self, data: &[u8]) -> Vec<u8> {
        match &self.outer {
            Outer::None => data.to_vec(),
            Outer::Lz4 => lz4_flex::block::decompress_size_prepended(data).unwrap(),
            Outer::Lz4Dict(d) => {
                lz4_flex::block::decompress_size_prepended_with_dict(data, d).unwrap()
            }
            Outer::Zstd(_) | Outer::ZstdDict(..) => {
                self.zd.as_mut().unwrap().decompress(data, 1 << 26).unwrap()
            }
            Outer::Deflate(_) => {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(data), &mut out)
                    .unwrap();
                out
            }
        }
    }
}

// ----- representations -----

#[derive(Clone, Copy, PartialEq)]
enum Text {
    Xml,
    Html,
}

#[derive(Clone)]
enum Repr {
    /// XML or HTML text.
    Text(Text),
    /// EXI with these options, header without options (agreed ahead of time).
    Exi {
        label: &'static str,
        opts: Options,
        set: usize,
    },
}

impl Repr {
    fn name(&self) -> String {
        match self {
            Repr::Text(Text::Xml) => "xml".into(),
            Repr::Text(Text::Html) => "html".into(),
            Repr::Exi { label, opts, .. } => format!("exi {label} {}", coding(opts.coding())),
        }
    }
}

fn coding(c: Coding) -> &'static str {
    match c {
        Coding::BitPacked => "bit",
        Coding::ByteAligned => "byte",
        Coding::PreCompression => "pre",
        Coding::Compression => "compression",
    }
}

/// Grammars by index: 0 schema-less, then a corpus's own.
struct Sets<'a, 'g> {
    g: Vec<&'a Grammars<'g>>,
}

fn to_text(t: Text, evs: &[OwnedEvent]) -> Vec<u8> {
    match t {
        Text::Xml => {
            let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
            for e in evs {
                w.event(&e.as_event()).unwrap();
            }
            w.into_sink()
        }
        Text::Html => {
            let mut w = exi::html::HtmlWriter::new(Vec::new());
            for e in evs {
                w.event(&e.as_event()).unwrap();
            }
            w.into_sink()
        }
    }
}

fn from_text(t: Text, g: &Grammars<'_>, text: &[u8]) -> Vec<OwnedEvent> {
    match t {
        Text::Xml => xml_events(g, text),
        Text::Html => {
            let mut v: Vec<OwnedEvent> = Vec::new();
            exi::html::encode_html(
                std::str::from_utf8(text).unwrap(),
                &mut v,
                &exi::html::HtmlOptions::new(),
            )
            .unwrap();
            v
        }
    }
}

fn xml_events(g: &Grammars<'_>, xml: &[u8]) -> Vec<OwnedEvent> {
    let mut r: XmlReader = XmlReader::new(XmlConfig::new(), g);
    let mut out = Vec::new();
    let mut rest = xml;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

fn exi_encode(g: &Grammars<'_>, opts: &Options, evs: &[OwnedEvent]) -> Vec<u8> {
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), g).header(Header::Bare { cookie: false }),
    )
    .unwrap();
    for e in evs {
        w.event(&e.as_event()).unwrap();
    }
    w.into_sink()
}

fn exi_decode(g: &Grammars<'_>, opts: &Options, exi: &[u8]) -> Vec<OwnedEvent> {
    let mut r: Reader<'_> = Reader::new(ReaderConfig::new(g).options(opts.clone())).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        at += r.feed(&exi[at..]);
        if at == exi.len() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

/// One item (a stanza or a page) in a representation, before the outer compressor.
fn encode_item(sets: &Sets<'_, '_>, r: &Repr, evs: &[OwnedEvent]) -> Vec<u8> {
    match r {
        Repr::Text(t) => to_text(*t, evs),
        Repr::Exi { opts, set, .. } => exi_encode(sets.g[*set], opts, evs),
    }
}

fn decode_item(sets: &Sets<'_, '_>, r: &Repr, bytes: &[u8]) -> Vec<OwnedEvent> {
    match r {
        Repr::Text(t) => from_text(*t, sets.g[0], bytes),
        Repr::Exi { opts, set, .. } => exi_decode(sets.g[*set], opts, bytes),
    }
}

// ----- timing -----

struct Clock {
    quick: bool,
}

impl Clock {
    /// Times `f` over several runs (one with --quick).
    fn time(&self, mut f: impl FnMut()) -> Stats {
        let mut v = Vec::new();
        let start = Instant::now();
        loop {
            let t = Instant::now();
            f();
            v.push(t.elapsed());
            if self.quick
                || (v.len() >= 3 && start.elapsed() > Duration::from_millis(600))
                || v.len() >= 15
            {
                break;
            }
        }
        Stats::of(v)
    }
}

// ----- results -----

struct Row {
    corpus: String,
    mode: &'static str,
    config: String,
    items: usize,
    text_bytes: usize,
    bytes: usize,
    enc: Stats,
    dec: Stats,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

// ----- corpora -----

struct Corpus {
    name: String,
    text: Text,
    /// Each item's original text and events.
    items: Vec<(Vec<u8>, Vec<OwnedEvent>)>,
    /// Training items: for dictionaries and schemas.
    train: Vec<(Vec<u8>, Vec<OwnedEvent>)>,
}

/// A session and its training session, or `None` when they have not been generated.
fn load_stanzas(g: &Grammars<'_>, kind: &str) -> Option<Corpus> {
    let read = |which: &str| -> Option<Vec<(Vec<u8>, Vec<OwnedEvent>)>> {
        let path = common::package(&format!("benches/xmpp/{kind}-{which}.xml"));
        let text = std::fs::read_to_string(path).ok()?;
        Some(
            text.lines()
                .filter(|l| !l.is_empty())
                .map(|l| (l.as_bytes().to_vec(), xml_events(g, l.as_bytes())))
                .collect(),
        )
    };
    Some(Corpus {
        name: kind.into(),
        text: Text::Xml,
        items: read("test")?,
        train: read("train")?,
    })
}

fn log_document() -> Vec<u8> {
    let mut s = String::from("<log>");
    for i in 0..60_000u32 {
        let _ = write!(
            s,
            "<e k=\"v{}\" t=\"v{}\" id=\"{i}\" at=\"2026-11-{:02}T{:02}:{:02}:00Z\">value {} text {i}</e>",
            i % 7,
            i % 5,
            1 + i / 2880 % 28,
            i / 60 % 24,
            i % 60,
            i % 1000
        );
    }
    s.push_str("</log>");
    s.into_bytes()
}

fn trained_dict(samples: &[Vec<u8>]) -> std::sync::Arc<Vec<u8>> {
    std::sync::Arc::new(zstd::dict::from_samples(samples, 16 * 1024).expect("dictionary training"))
}

// ----- the measurements -----

/// Per-item configurations: each item alone.
#[allow(clippy::too_many_arguments)]
fn per_item(
    clock: &Clock,
    sets: &Sets<'_, '_>,
    c: &Corpus,
    mode: &'static str,
    r: &Repr,
    outer: &Outer,
    rows: &mut Vec<Row>,
    sizes: &mut Vec<(String, Vec<usize>)>,
) {
    let mut p = Packer::new(outer);
    let wire: Vec<Vec<u8>> = c
        .items
        .iter()
        .map(|(_, evs)| p.pack(encode_item(sets, r, evs)))
        .collect();
    // Check the round trip once: the same elements, attributes and text back. Namespace
    // declarations go when prefixes are not kept, and the XHTML5 profile keeps no comments.
    let content = |v: &[OwnedEvent]| {
        v.iter()
            .filter(|e| {
                !matches!(
                    e,
                    OwnedEvent::Namespace { .. }
                        | OwnedEvent::Comment(_)
                        | OwnedEvent::ProcessingInstruction { .. }
                        | OwnedEvent::Doctype { .. }
                )
            })
            .count()
    };
    for (w, (_, evs)) in wire.iter().zip(&c.items) {
        let back = decode_item(sets, r, &p.unpack(w));
        assert_eq!(content(&back), content(evs), "{} {}", c.name, r.name());
    }
    let enc = clock.time(|| {
        for (_, evs) in &c.items {
            black_box(p.pack(encode_item(sets, r, evs)));
        }
    });
    let dec = clock.time(|| {
        for w in &wire {
            black_box(decode_item(sets, r, &p.unpack(w)).len());
        }
    });
    let config = format!("{}{}", r.name(), outer.name());
    let bytes = wire.iter().map(Vec::len).sum();
    let text_bytes = c.items.iter().map(|(t, _)| t.len()).sum();
    println!(
        "{:9} {:9} {:44} {:>10} {:>7.1}% {:>9.1} {:>9.1}",
        c.name,
        mode,
        config,
        bytes,
        100.0 * bytes as f64 / text_bytes as f64,
        ms(enc.median),
        ms(dec.median)
    );
    sizes.push((config.clone(), wire.iter().map(Vec::len).collect()));
    rows.push(Row {
        corpus: c.name.clone(),
        mode,
        config,
        items: c.items.len(),
        text_bytes,
        bytes,
        enc,
        dec,
    });
}

/// The original text under a compressor: the size the XML or HTML would have on the wire.
///
/// Encoding writes the text from events, as an application would, and then compresses
/// the original text, so the sizes are those of the text as given. The written text is
/// thrown away; the time to write it is kept, so these rows compare with the EXI rows.
fn original_text(
    clock: &Clock,
    sets: &Sets<'_, '_>,
    c: &Corpus,
    mode: &'static str,
    outer: &Outer,
    rows: &mut Vec<Row>,
    sizes: &mut Vec<(String, Vec<usize>)>,
) {
    let mut p = Packer::new(outer);
    let wire: Vec<Vec<u8>> = c.items.iter().map(|(t, _)| p.pack_slice(t)).collect();
    let r = Repr::Text(c.text);
    let enc = clock.time(|| {
        for (t, evs) in &c.items {
            black_box(to_text(c.text, evs).len());
            black_box(p.pack_slice(t));
        }
    });
    let dec = clock.time(|| {
        for w in &wire {
            black_box(decode_item(sets, &r, &p.unpack(w)).len());
        }
    });
    let config = format!("{}{}", r.name(), outer.name());
    let bytes = wire.iter().map(Vec::len).sum();
    let text_bytes = c.items.iter().map(|(t, _)| t.len()).sum();
    println!(
        "{:9} {:9} {:44} {:>10} {:>7.1}% {:>9.1} {:>9.1}",
        c.name,
        mode,
        config,
        bytes,
        100.0 * bytes as f64 / text_bytes as f64,
        ms(enc.median),
        ms(dec.median)
    );
    sizes.push((config.clone(), wire.iter().map(Vec::len).collect()));
    rows.push(Row {
        corpus: c.name.clone(),
        mode,
        config,
        items: c.items.len(),
        text_bytes,
        bytes,
        enc,
        dec,
    });
}

/// Session-wide references: one compression context, or one EXI stream of frames, for the
/// whole session. These leak between stanzas.
fn session_wide(clock: &Clock, sets: &Sets<'_, '_>, c: &Corpus, rows: &mut Vec<Row>) {
    let text_bytes: usize = c.items.iter().map(|(t, _)| t.len()).sum();
    let mut push = |config: String, bytes: usize, enc: Stats, dec: Stats| {
        println!(
            "{:9} {:9} {:44} {:>10} {:>7.1}% {:>9.1} {:>9.1}",
            c.name,
            "session",
            config,
            bytes,
            100.0 * bytes as f64 / text_bytes as f64,
            ms(enc.median),
            ms(dec.median)
        );
        rows.push(Row {
            corpus: c.name.clone(),
            mode: "session",
            config,
            items: c.items.len(),
            text_bytes,
            bytes,
            enc,
            dec,
        });
    };
    // XEP-0138: one zlib stream, flushed after each stanza.
    let zlib = || {
        let mut z = flate2::Compress::new(flate2::Compression::new(6), true);
        let mut out = Vec::with_capacity(text_bytes);
        for (t, _) in &c.items {
            out.reserve(t.len() + 64);
            z.compress_vec(t, &mut out, flate2::FlushCompress::Sync)
                .unwrap();
        }
        out
    };
    let wire = zlib();
    let enc = clock.time(|| {
        black_box(zlib().len());
    });
    let dec = clock.time(|| {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&wire[..]), &mut out).ok();
        black_box(out.len());
    });
    push(
        "xml + zlib-6 session (XEP-0138)".into(),
        wire.len(),
        enc,
        dec,
    );
    // zstd, one stream flushed after each stanza.
    let zs = || {
        let mut e = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
        for (t, _) in &c.items {
            e.write_all(t).unwrap();
            e.flush().unwrap();
        }
        e.finish().unwrap()
    };
    let wire = zs();
    let enc = clock.time(|| {
        black_box(zs().len());
    });
    let dec = clock.time(|| {
        black_box(zstd::stream::decode_all(&wire[..]).unwrap().len());
    });
    push("xml + zstd-3 session".into(), wire.len(), enc, dec);
    // EXI: one stream of frames, string tables and learned grammars kept.
    for (label, set, strict) in [("schema-less", 0usize, false), ("schema", 1, false)] {
        if set >= sets.g.len() {
            continue;
        }
        let g = sets.g[set];
        let opts = Options::builder()
            .coding(Coding::ByteAligned)
            .fragment(true)
            .strict(strict)
            .build()
            .unwrap();
        let frames = || -> Vec<Vec<u8>> {
            let mut w = Writer::new(
                Vec::new(),
                WriterConfig::new(opts.clone(), g)
                    .header(Header::Bare { cookie: false })
                    .sequence(Sequence::Frames),
            )
            .unwrap();
            let mut out = Vec::new();
            for (_, evs) in &c.items {
                for e in evs {
                    if !matches!(e, OwnedEvent::StartDocument | OwnedEvent::EndDocument) {
                        w.event(&e.as_event()).unwrap();
                    }
                }
                out.push(w.end_frame().unwrap().to_vec());
            }
            out
        };
        let wire = frames();
        let enc = clock.time(|| {
            black_box(frames().len());
        });
        let dec = clock.time(|| {
            let mut r: Reader<'_> = Reader::new(
                ReaderConfig::new(g)
                    .options(opts.clone())
                    .sequence(Sequence::Frames),
            )
            .unwrap();
            let mut n = 0;
            for f in &wire {
                let mut at = 0;
                while at < f.len() {
                    at += r.feed(&f[at..]);
                    while let Step::Event(..) = r.next().unwrap() {
                        n += 1;
                    }
                }
                r.end_frame();
                while let Step::Event(..) = r.next().unwrap() {
                    n += 1;
                }
            }
            black_box(n);
        });
        push(
            format!("exi {label} byte, frames session (sessionWideBuffers)"),
            wire.iter().map(Vec::len).sum(),
            enc,
            dec,
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--bench")
        .collect();
    let quick = args.iter().any(|a| a == "--quick");
    let words: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let wanted = |name: &str| words.is_empty() || words.iter().any(|w| name.contains(w.as_str()));
    let clock = Clock { quick };
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let none = GrammarSet::schemaless();
    let g0 = none.attach(&strings).unwrap();
    let mut rows = Vec::new();
    let out = common::out_dir();
    println!(
        "{:9} {:9} {:44} {:>10} {:>8} {:>9} {:>9}",
        "corpus", "mode", "configuration", "octets", "of text", "enc ms", "dec ms"
    );

    // ----- XMPP stanzas -----
    let mut stanza_csv = String::from("corpus,stanza,element,config,octets\n");
    for kind in ["chat", "iot", "commands"] {
        if !wanted("stanzas") && !wanted(kind) {
            continue;
        }
        let Some(c) = load_stanzas(&g0, kind) else {
            println!("{kind:9} skipped: no session; run python3 benches/xmpp/gen_sessions.py");
            continue;
        };
        // A schema learned from the training session.
        let mut learn = SchemaInference::new().fit(Fit::General);
        for (t, _) in &c.train {
            learn.document(t).unwrap();
        }
        let schema = learn.grammars().unwrap();
        let g1 = schema.attach(&strings).unwrap();
        let sets = Sets { g: vec![&g0, &g1] };
        let opts =
            |c: Coding, strict: bool| Options::builder().coding(c).strict(strict).build().unwrap();
        // Strict needs every test stanza to fit the learned schema.
        let strict_ok = c.items.iter().all(|(_, evs)| {
            let mut w = Writer::new(
                Vec::new(),
                WriterConfig::new(opts(Coding::BitPacked, true), sets.g[1]),
            )
            .unwrap();
            evs.iter().all(|e| w.event(&e.as_event()).is_ok())
        });
        let mut sizes: Vec<(String, Vec<usize>)> = Vec::new();
        // Dictionaries are trained on the training session in each representation.
        let dict_for = |r: &Repr| -> std::sync::Arc<Vec<u8>> {
            let samples: Vec<Vec<u8>> = match r {
                Repr::Text(_) => c.train.iter().map(|(t, _)| t.clone()).collect(),
                _ => c
                    .train
                    .iter()
                    .map(|(_, evs)| encode_item(&sets, r, evs))
                    .collect(),
            };
            trained_dict(&samples)
        };
        // LZ4 dictionaries: the tail of the trained dictionary, where its content is.
        let lz4d = |d: &std::sync::Arc<Vec<u8>>, n: usize| {
            Outer::Lz4Dict(std::sync::Arc::new(d[d.len().saturating_sub(n)..].to_vec()))
        };
        let xml = Repr::Text(Text::Xml);
        for o in [
            Outer::None,
            Outer::Lz4,
            lz4d(&dict_for(&xml), 4096),
            lz4d(&dict_for(&xml), 16384),
            Outer::Zstd(3),
            Outer::ZstdDict(3, dict_for(&xml)),
            Outer::ZstdDict(19, dict_for(&xml)),
            Outer::Deflate(6),
        ] {
            original_text(&clock, &sets, &c, "stanza", &o, &mut rows, &mut sizes);
        }
        let mut reprs: Vec<(Repr, Vec<Outer>)> = Vec::new();
        for (label, set, strict) in [
            ("schema-less", 0usize, false),
            ("schema", 1, false),
            ("schema strict", 1, true),
        ] {
            if strict && !strict_ok {
                println!(
                    "{:9} (the learned schema does not fit every test stanza: no strict rows)",
                    c.name
                );
                continue;
            }
            let ex = |cd: Coding| Repr::Exi {
                label,
                opts: opts(cd, strict),
                set,
            };
            let pre = ex(Coding::PreCompression);
            let bit = ex(Coding::BitPacked);
            let byte = ex(Coding::ByteAligned);
            let dpre = dict_for(&pre);
            let dbit = dict_for(&bit);
            let dbyte = dict_for(&byte);
            reprs.push((
                bit.clone(),
                vec![
                    Outer::None,
                    Outer::Lz4,
                    lz4d(&dbit, 4096),
                    lz4d(&dbit, 16384),
                    Outer::ZstdDict(3, dbit),
                ],
            ));
            reprs.push((
                byte.clone(),
                vec![
                    Outer::None,
                    Outer::Lz4,
                    lz4d(&dbyte, 4096),
                    lz4d(&dbyte, 16384),
                    Outer::Zstd(3),
                ],
            ));
            reprs.push((
                pre,
                vec![
                    Outer::None,
                    Outer::Lz4,
                    lz4d(&dpre, 4096),
                    lz4d(&dpre, 16384),
                    Outer::Zstd(3),
                    Outer::ZstdDict(3, dpre.clone()),
                    Outer::ZstdDict(19, dpre),
                ],
            ));
            reprs.push((ex(Coding::Compression), vec![Outer::None]));
        }
        for (r, outers) in &reprs {
            for o in outers {
                per_item(&clock, &sets, &c, "stanza", r, o, &mut rows, &mut sizes);
            }
        }
        session_wide(&clock, &sets, &c, &mut rows);
        let element = |evs: &[OwnedEvent]| {
            evs.iter()
                .find_map(|e| {
                    if let OwnedEvent::StartElement(n) = e {
                        Some(n.as_name().local_name().to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        };
        for (config, v) in &sizes {
            for (i, n) in v.iter().enumerate() {
                let _ = writeln!(
                    stanza_csv,
                    "{},{i},{},\"{config}\",{n}",
                    c.name,
                    element(&c.items[i].1)
                );
            }
        }
        println!();
    }
    std::fs::write(out.join("stanzas.csv"), stanza_csv).unwrap();

    // ----- documents -----
    let mut page_csv = String::from("page,config,octets\n");
    let dir = std::env::var("EXI_HTML_CORPUS").unwrap_or_else(|_| "target/html-corpus".into());
    let mut docs: Vec<Corpus> = Vec::new();
    if wanted("pages") {
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        paths.retain(|p| p.extension().is_some_and(|x| x == "html"));
        paths.sort();
        if paths.is_empty() {
            println!("pages     skipped: no .html files in {dir} (set EXI_HTML_CORPUS)");
        } else {
            let items = paths
                .iter()
                .map(|p| {
                    let t = std::fs::read(p).unwrap();
                    let evs = from_text(Text::Html, &g0, &t);
                    (t, evs)
                })
                .collect();
            docs.push(Corpus {
                name: "pages".into(),
                text: Text::Html,
                items,
                train: Vec::new(),
            });
        }
    }
    if wanted("log") {
        let t = log_document();
        let evs = xml_events(&g0, &t);
        docs.push(Corpus {
            name: "log".into(),
            text: Text::Xml,
            items: vec![(t, evs)],
            train: Vec::new(),
        });
    }
    let x5set = exi::html::profile::grammars(1).unwrap();
    let gx5 = x5set.attach(&strings).unwrap();
    for c in &docs {
        let sets = Sets { g: vec![&g0, &gx5] };
        let mut sizes = Vec::new();
        for o in [
            Outer::None,
            Outer::Lz4,
            Outer::Zstd(3),
            Outer::Zstd(19),
            Outer::Deflate(6),
        ] {
            original_text(&clock, &sets, c, "document", &o, &mut rows, &mut sizes);
        }
        let mut reprs: Vec<(Repr, Vec<Outer>)> = Vec::new();
        // Comments, PIs and the DOCTYPE are kept, as in the text.
        let keep = exi::Preserve::COMMENTS | exi::Preserve::PIS | exi::Preserve::DTD;
        let ex = |cd: Coding| Repr::Exi {
            label: "schema-less",
            opts: Options::builder()
                .coding(cd)
                .preserve(keep)
                .build()
                .unwrap(),
            set: 0,
        };
        reprs.push((
            ex(Coding::BitPacked),
            vec![Outer::None, Outer::Lz4, Outer::Zstd(3)],
        ));
        reprs.push((
            ex(Coding::ByteAligned),
            vec![Outer::None, Outer::Lz4, Outer::Zstd(3)],
        ));
        reprs.push((
            ex(Coding::PreCompression),
            vec![Outer::None, Outer::Lz4, Outer::Zstd(3), Outer::Zstd(19)],
        ));
        reprs.push((ex(Coding::Compression), vec![Outer::None]));
        if c.text == Text::Html {
            let x5 = |cd: Coding| {
                let o = exi::html::profile::options(1)
                    .unwrap()
                    .to_builder()
                    .coding(cd)
                    .build()
                    .unwrap();
                Repr::Exi {
                    label: "xhtml5",
                    opts: o,
                    set: 1,
                }
            };
            reprs.push((x5(Coding::BitPacked), vec![Outer::None, Outer::Lz4]));
            reprs.push((
                x5(Coding::PreCompression),
                vec![Outer::None, Outer::Lz4, Outer::Zstd(3), Outer::Zstd(19)],
            ));
            reprs.push((x5(Coding::Compression), vec![Outer::None]));
        }
        for (r, outers) in &reprs {
            for o in outers {
                per_item(&clock, &sets, c, "document", r, o, &mut rows, &mut sizes);
            }
        }
        if c.name == "pages" {
            for (config, v) in &sizes {
                for (i, n) in v.iter().enumerate() {
                    let _ = writeln!(page_csv, "{i},\"{config}\",{n}");
                }
            }
        }
        println!();
    }
    if docs.iter().any(|d| d.name == "pages") {
        std::fs::write(out.join("pages.csv"), page_csv).unwrap();
    }

    // New columns go at the end, so scripts that read the first ones keep working.
    let mut csv = String::from(
        "corpus,mode,config,items,text_octets,octets,ratio,encode_ms,decode_ms,encode_mb_s,decode_mb_s,encode_runs,decode_runs,encode_min_ms,encode_max_ms,decode_min_ms,decode_max_ms\n",
    );
    for r in &rows {
        let mb = r.text_bytes as f64 / 1e6;
        let _ = writeln!(
            csv,
            "{},{},\"{}\",{},{},{},{:.4},{:.3},{:.3},{:.1},{:.1},{},{},{:.3},{:.3},{:.3},{:.3}",
            r.corpus,
            r.mode,
            r.config,
            r.items,
            r.text_bytes,
            r.bytes,
            r.bytes as f64 / r.text_bytes as f64,
            ms(r.enc.median),
            ms(r.dec.median),
            mb / r.enc.median.as_secs_f64(),
            mb / r.dec.median.as_secs_f64(),
            r.enc.samples,
            r.dec.samples,
            ms(r.enc.min),
            ms(r.enc.max),
            ms(r.dec.min),
            ms(r.dec.max)
        );
    }
    std::fs::write(out.join("codecs.csv"), csv).unwrap();
    println!("wrote {}: codecs.csv, stanzas.csv, pages.csv", out.display());
    common::write_meta("codecs");
}
