// benches/exi.rs
//
// EXI throughput benchmarks
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! `cargo bench --features infer`: time the crate's main paths over two generated
//! documents (an SVG drawing and a data feed, about 1 MB each) and any `.xml` or `.svg`
//! files in `benches/data`.
//!
//!     cargo bench --features infer                     # every case
//!     cargo bench --features infer -- encode svg       # cases whose name has every word
//!     cargo bench --features infer -- --corpus DIR     # write the generated documents
//!     cargo bench --features infer,dom                 # and the document tree
//!
//! Each case runs for about a second after a warm-up. A sample is the mean time of the
//! runs in about 100 ms; the figures are the median of ten samples, input megabytes per
//! second at the median, and the spread (slowest over fastest sample). A spread above a
//! few percent means the machine was busy or changed speed. `target/bench/exi.csv` holds
//! every case and `target/bench/exi.meta` the build and machine. `benches/compare.sh`
//! times the `exi` tool against other XML and EXI tools with hyperfine.
//!
//! Schema-less and strict encoding and decoding run in every coding built, except
//! pre-compression: it is compression without DEFLATE, and `codecs` measures it. The
//! other cases use byte alignment. A `reset` case reuses one writer or reader, as a server
//! does for each stream; the others make a new one for each run.
//!
//! Every run starts a stream with its own string tables, which release their strings
//! when the stream ends. So the global string pool should hold as many strings after a
//! case as before it. The harness checks this and reports a case that leaves strings
//! behind, because a growing pool would change the times of the cases after it.
//!
//! This harness uses only the standard library, so `cargo bench` needs no downloads.

mod common;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

use common::Stats;
use exi::grammar::{GrammarSet, Grammars};
use exi::infer::{Fit, SchemaInference};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    Coding, Event, EventSink, Options, OwnedEvent, Reader, ReaderConfig, Status, Step, Writer,
    WriterConfig,
};

// ----- the harness -----

struct Bench<'s> {
    words: Vec<String>,
    strings: &'s GlobalStrings,
    /// `target/bench/exi.csv`: one line for each case run.
    csv: RefCell<String>,
}

/// Live strings in all four tables of the global pool.
fn live(strings: &GlobalStrings) -> u64 {
    u64::from(strings.uris().stats().strings)
        + u64::from(strings.prefixes().stats().strings)
        + u64::from(strings.locals().stats().strings)
        + u64::from(strings.values().stats().strings)
}

impl Bench<'_> {
    fn wanted(&self, name: &str) -> bool {
        self.words.iter().all(|w| name.contains(w.as_str()))
    }

    /// Times `f`, which handles `bytes` octets of input per run.
    fn run(&self, name: &str, bytes: usize, mut f: impl FnMut()) {
        if !self.wanted(name) {
            return;
        }
        let pool = live(self.strings);
        // Warm up, and find how many runs make a sample of about 100 ms.
        let start = Instant::now();
        let mut runs = 0u32;
        while start.elapsed() < Duration::from_millis(200) || runs < 2 {
            f();
            runs += 1;
        }
        let per = start.elapsed() / runs;
        let n = ((Duration::from_millis(100).as_nanos() / per.as_nanos().max(1)) as u32).max(1);
        let s = Stats::of(
            (0..10)
                .map(|_| {
                    let t = Instant::now();
                    for _ in 0..n {
                        f();
                    }
                    t.elapsed() / n
                })
                .collect(),
        );
        let mbs = bytes as f64 / s.median.as_secs_f64() / 1e6;
        let grew = live(self.strings).saturating_sub(pool);
        let note = if grew > 0 {
            format!("  (the string pool grew by {grew})")
        } else {
            String::new()
        };
        println!(
            "{name:48} {:>12} {mbs:>9.1} MB/s  spread {:>4.1}%{note}",
            fmt(s.median),
            100.0 * s.spread(),
        );
        let _ = writeln!(
            self.csv.borrow_mut(),
            "\"{name}\",{bytes},{n},{},{},{},{},{mbs:.2},{grew}",
            s.samples,
            s.median.as_nanos(),
            s.min.as_nanos(),
            s.max.as_nanos()
        );
    }
}

fn fmt(d: Duration) -> String {
    let ns = d.as_nanos() as f64;
    if ns >= 1e9 {
        format!("{:.3} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.3} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.3} µs", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

/// Counts events, so nothing is optimized away.
struct Null(u64);

impl EventSink for Null {
    fn event(&mut self, e: &Event<'_>) -> exi::Result<()> {
        black_box(e);
        self.0 += 1;
        Ok(())
    }
}

// ----- the paths timed -----

fn opts(coding: Coding, strict: bool) -> Options {
    Options::builder()
        .coding(coding)
        .strict(strict)
        .build()
        .unwrap()
}

/// The codings built, by name. Pre-compression is left out: see the module docs.
fn codings() -> Vec<(&'static str, Coding)> {
    let mut v = vec![("byte", Coding::ByteAligned)];
    if cfg!(feature = "bit-packed") {
        v.push(("bit", Coding::BitPacked));
    }
    if cfg!(feature = "compression") {
        v.push(("compression", Coding::Compression));
    }
    v
}

fn xml_reader<'g>(g: &'g Grammars<'g>) -> XmlReader<'g> {
    XmlReader::new(XmlConfig::new().report_comments(false).report_pis(false), g)
}

/// XML text into `sink`.
fn parse(g: &Grammars<'_>, xml: &[u8], sink: &mut impl EventSink) {
    let mut x = xml_reader(g);
    let mut rest = xml;
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        if exi::transcode(&mut x, sink).unwrap() == Status::End {
            return;
        }
    }
}

fn encode(g: &Grammars<'_>, xml: &[u8], o: &Options) -> Vec<u8> {
    let mut w = Writer::new(
        Vec::with_capacity(xml.len() / 4),
        WriterConfig::new(o.clone(), g),
    )
    .unwrap();
    parse(g, xml, &mut w);
    w.into_sink()
}

fn decode(g: &Grammars<'_>, stream: &[u8], sink: &mut impl EventSink) {
    let mut r: Reader = Reader::new(ReaderConfig::new(g)).unwrap();
    decode_with(&mut r, stream, sink);
}

/// Decodes `stream` with a reader that is new or reset.
fn decode_with(r: &mut Reader<'_>, stream: &[u8], sink: &mut impl EventSink) {
    let mut rest = stream;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        if exi::transcode(r, sink).unwrap() == Status::End {
            return;
        }
    }
}

fn events(g: &Grammars<'_>, xml: &[u8]) -> Vec<OwnedEvent> {
    let mut x = xml_reader(g);
    let mut out = Vec::new();
    let mut rest = xml;
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        loop {
            match x.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::End => return out,
                Step::NeedInput => break,
            }
        }
    }
}

fn learn(xml: &[u8]) -> SchemaInference {
    let mut l = SchemaInference::new().fit(Fit::Exact);
    l.document(xml).unwrap();
    l
}

fn case(b: &Bench<'_>, name: &str, xml: &[u8]) {
    let strings = b.strings;
    let none = GrammarSet::schemaless();
    let g = none.attach(strings).unwrap();
    let evs = events(&g, xml);
    let n = xml.len();
    let byte = opts(Coding::ByteAligned, false);

    b.run(&format!("xml parse {name}"), n, || {
        let mut s = Null(0);
        parse(&g, xml, &mut s);
        black_box(s.0);
    });
    b.run(&format!("xml write {name}"), n, || {
        let mut w = XmlWriter::new(Vec::with_capacity(n), XmlWriterConfig::new());
        for e in &evs {
            w.event(&e.as_event()).unwrap();
        }
        black_box(w.into_sink());
    });

    let mut sizes = format!("xml {n}");
    for (cname, coding) in codings() {
        let o = opts(coding, false);
        let stream = encode(&g, xml, &o);
        let _ = write!(sizes, ", {cname} {}", stream.len());
        b.run(&format!("encode {name} schema-less {cname}"), n, || {
            black_box(encode(&g, xml, &o));
        });
        b.run(&format!("decode {name} schema-less {cname}"), n, || {
            let mut s = Null(0);
            decode(&g, &stream, &mut s);
            black_box(s.0);
        });
    }

    // From events, without the XML parser: a new writer, then one writer reset each run.
    let stream = encode(&g, xml, &byte);
    b.run(&format!("encode-events {name} schema-less byte"), n, || {
        let mut w = Writer::new(
            Vec::with_capacity(n / 4),
            WriterConfig::new(byte.clone(), &g),
        )
        .unwrap();
        for e in &evs {
            w.event(&e.as_event()).unwrap();
        }
        black_box(w.into_sink());
    });
    let mut w = Writer::new(
        Vec::with_capacity(n / 4),
        WriterConfig::new(byte.clone(), &g),
    )
    .unwrap();
    b.run(&format!("encode-events {name} schema-less byte reset"), n, || {
        for e in &evs {
            w.event(&e.as_event()).unwrap();
        }
        black_box(w.reset(Vec::with_capacity(n / 4)));
    });
    drop(w);
    // Reset after each stream, so the pool check does not count the last stream's strings.
    let mut r: Reader = Reader::new(ReaderConfig::new(&g)).unwrap();
    b.run(&format!("decode {name} schema-less byte reset"), n, || {
        let mut s = Null(0);
        decode_with(&mut r, &stream, &mut s);
        r.reset();
        black_box(s.0);
    });
    drop(r);
    b.run(&format!("decode-to-xml {name} schema-less byte"), n, || {
        let mut w = XmlWriter::new(Vec::with_capacity(n), XmlWriterConfig::new());
        decode(&g, &stream, &mut w);
        black_box(w.into_sink());
    });
    #[cfg(feature = "dom")]
    dom(b, name, xml, &g, &stream);

    // A schema learned from the document itself: strict streams.
    b.run(&format!("infer {name}"), n, || {
        black_box(learn(xml).schemas());
    });
    let l = learn(xml);
    let docs = l.schemas();
    b.run(
        &format!("compile {name} inferred xsd"),
        docs.iter().map(|d| d.text.len()).sum(),
        || {
            black_box(l.grammars().unwrap());
        },
    );
    let set = l.grammars().unwrap();
    let exig = set.to_exig().unwrap();
    b.run(&format!("exig-load {name} inferred"), exig.len(), || {
        black_box(GrammarSet::from_exig(&exig).unwrap());
    });
    let gs = set.attach(strings).unwrap();
    for (cname, coding) in codings() {
        let o = opts(coding, true);
        let strict = encode(&gs, xml, &o);
        let _ = write!(sizes, ", strict {cname} {}", strict.len());
        b.run(&format!("encode {name} strict {cname}"), n, || {
            black_box(encode(&gs, xml, &o));
        });
        b.run(&format!("decode {name} strict {cname}"), n, || {
            let mut s = Null(0);
            decode(&gs, &strict, &mut s);
            black_box(s.0);
        });
    }
    if b.wanted("size") || b.words.is_empty() {
        println!("{:48} {sizes}, exig {}", format!("size {name}"), exig.len());
    }
}

/// Layer 2: the tree from XML text and from EXI, and back.
#[cfg(feature = "dom")]
fn dom(b: &Bench<'_>, name: &str, xml: &[u8], g: &Grammars<'_>, stream: &[u8]) {
    use exi::dom::{Document, DocumentBuilder};
    let strings = b.strings;
    let n = xml.len();
    b.run(&format!("dom parse-xml {name}"), n, || {
        black_box(Document::parse_xml(strings, xml).unwrap());
    });
    let doc = Document::parse_xml(strings, xml).unwrap();
    b.run(&format!("dom to-xml {name}"), n, || {
        black_box(doc.to_xml().unwrap());
    });
    b.run(&format!("dom read {name} schema-less byte"), n, || {
        let mut t = DocumentBuilder::new(strings);
        decode(g, stream, &mut t);
        black_box(t.finish().unwrap());
    });
    b.run(&format!("dom write {name} schema-less byte"), n, || {
        let mut w = Writer::new(
            Vec::with_capacity(n / 4),
            WriterConfig::new(opts(Coding::ByteAligned, false), g),
        )
        .unwrap();
        doc.write(&mut w).unwrap();
        black_box(w.into_sink());
    });
}

/// The hand-written schemas in `schemas/`: compiling, loading and attaching.
fn schemas(b: &Bench<'_>) {
    let xsd = std::fs::read(common::package("schemas/xhtml5-1.xsd")).unwrap();
    b.run("compile xhtml5-1.xsd", xsd.len(), || {
        black_box(GrammarSet::from_xsd(&xsd).unwrap());
    });
    let exig = std::fs::read(common::package("schemas/xhtml5-1.exig")).unwrap();
    b.run("exig-load xhtml5-1.exig", exig.len(), || {
        black_box(GrammarSet::from_exig(&exig).unwrap());
    });
    let set = GrammarSet::from_exig(&exig).unwrap();
    b.run("attach xhtml5-1.exig", exig.len(), || {
        black_box(set.attach(b.strings).unwrap());
    });
}

// ----- the generated documents -----

/// A small deterministic generator (an LCG), so every run times the same documents.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// An Inkscape-like drawing: groups of paths, circles, text and gradient references.
fn svg(target: usize) -> String {
    let mut r = Lcg(1);
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"1024\" height=\"768\" viewBox=\"0 0 1024 768\" version=\"1.1\">\n  <defs>\n",
    );
    let colors = [
        "#ff6a00", "#ee0979", "#7a00ff", "#00c3ff", "#222222", "#ffffff", "#3c9d4e",
    ];
    for i in 0..8 {
        s += &format!(
            "    <linearGradient id=\"g{i}\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\">\n      <stop offset=\"0\" stop-color=\"{}\"/>\n      <stop offset=\"1\" stop-color=\"{}\"/>\n    </linearGradient>\n",
            colors[i % 7],
            colors[(i + 3) % 7]
        );
    }
    s += "  </defs>\n";
    let mut layer = 0;
    while s.len() < target {
        s += &format!(
            "  <g id=\"layer{layer}\" transform=\"translate({},{})\" fill=\"url(#g{})\" stroke=\"{}\" stroke-width=\"{}\">\n",
            r.next(900),
            r.next(700),
            r.next(8),
            colors[r.next(7) as usize],
            1 + r.next(4)
        );
        for _ in 0..20 {
            match r.next(4) {
                0 | 1 => {
                    let mut d = format!("M{} {}", r.next(1000), r.next(800));
                    for _ in 0..(3 + r.next(6)) {
                        d += &format!(
                            " C {}.{} {}.{}, {} {}, {} {}",
                            r.next(1000),
                            r.next(10),
                            r.next(800),
                            r.next(10),
                            r.next(1000),
                            r.next(800),
                            r.next(1000),
                            r.next(800)
                        );
                    }
                    s += &format!("    <path d=\"{d} Z\" opacity=\"0.{}\"/>\n", 1 + r.next(9));
                }
                2 => {
                    s += &format!(
                        "    <circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>\n",
                        r.next(1000),
                        r.next(800),
                        1 + r.next(80),
                        colors[r.next(7) as usize]
                    )
                }
                _ => {
                    s += &format!(
                        "    <use xlink:href=\"#layer{}\" transform=\"scale(0.{})\"/>\n",
                        r.next(layer + 1),
                        1 + r.next(9)
                    )
                }
            }
        }
        s += &format!(
            "    <text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"{}\">Layer {layer}</text>\n  </g>\n",
            r.next(1000),
            r.next(800),
            8 + r.next(30)
        );
        layer += 1;
    }
    s + "</svg>\n"
}

/// A data feed: orders with typed fields, optional elements and repeated lines.
fn orders(target: usize) -> String {
    let mut r = Lcg(2);
    let names = [
        "Ada", "Grace", "Linus", "Barbara", "Ken", "Margaret", "Dennis", "Frances",
    ];
    let status = ["open", "paid", "shipped", "returned"];
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<orders>\n");
    let mut id = 100000;
    while s.len() < target {
        s += &format!(
            "  <order id=\"{id}\" paid=\"{}\">\n    <customer>{}</customer>\n    <date>2026-{:02}-{:02}</date>\n",
            r.next(2) == 0,
            names[r.next(8) as usize],
            1 + r.next(12),
            1 + r.next(28)
        );
        for _ in 0..(1 + r.next(5)) {
            s += &format!(
                "    <line sku=\"{}-{}\" qty=\"{}\" price=\"{}.{:02}\"/>\n",
                (b'A' + r.next(26) as u8) as char,
                r.next(100),
                1 + r.next(20),
                r.next(500),
                r.next(100)
            );
        }
        if r.next(4) == 0 {
            s += "    <note>leave at the door</note>\n";
        }
        s += &format!(
            "    <status>{}</status>\n  </order>\n",
            status[r.next(4) as usize]
        );
        id += 1;
    }
    s + "</orders>\n"
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut docs = vec![
        ("svg".to_string(), svg(1 << 20).into_bytes()),
        ("orders".to_string(), orders(1 << 20).into_bytes()),
    ];
    if let Ok(dir) = std::fs::read_dir(common::package("benches/data")) {
        let mut extra: Vec<_> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "xml" || x == "svg"))
            .collect();
        extra.sort();
        for p in extra {
            docs.push((
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            ));
        }
    }
    docs
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--corpus") {
        let dir = Path::new(args.get(i + 1).expect("--corpus DIR"));
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("generated.svg"), svg(1 << 20)).unwrap();
        std::fs::write(dir.join("orders.xml"), orders(1 << 20)).unwrap();
        return;
    }
    // cargo passes --bench; words are the filter.
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let b = Bench {
        words: args.into_iter().filter(|a| !a.starts_with('-')).collect(),
        strings: &strings,
        csv: RefCell::new(String::from(
            "case,input_octets,runs_per_sample,samples,median_ns,min_ns,max_ns,mb_s,pool_growth\n",
        )),
    };
    for (name, xml) in corpus() {
        case(&b, &name, &xml);
    }
    schemas(&b);
    let path = common::out_dir().join("exi.csv");
    std::fs::write(&path, b.csv.into_inner()).unwrap();
    println!("wrote {}", path.display());
    common::write_meta("exi");
}
