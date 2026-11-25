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
//!
//! Each case runs for about a second after a warm-up. The figure is the median of ten
//! samples: time per run, and input megabytes per second. `benches/compare.sh` times the
//! `exi` tool against other XML and EXI tools with hyperfine.
//!
//! This harness uses only the standard library, so `cargo bench` needs no downloads.

use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

use exi::grammar::{GrammarSet, Grammars};
use exi::infer::{Fit, SchemaInference};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    Coding, Event, EventSink, Options, OwnedEvent, Reader, ReaderConfig, Status, Step, Writer,
    WriterConfig,
};

// ----- the harness -----

struct Bench {
    words: Vec<String>,
}

impl Bench {
    fn wanted(&self, name: &str) -> bool {
        self.words.iter().all(|w| name.contains(w.as_str()))
    }

    /// Times `f`, which handles `bytes` octets of input per run.
    fn run(&self, name: &str, bytes: usize, mut f: impl FnMut()) {
        if !self.wanted(name) {
            return;
        }
        // Warm up, and find how many runs make a sample of about 100 ms.
        let start = Instant::now();
        let mut runs = 0u32;
        while start.elapsed() < Duration::from_millis(200) || runs < 2 {
            f();
            runs += 1;
        }
        let per = start.elapsed() / runs;
        let n = ((Duration::from_millis(100).as_nanos() / per.as_nanos().max(1)) as u32).max(1);
        let mut samples: Vec<Duration> = (0..10)
            .map(|_| {
                let t = Instant::now();
                for _ in 0..n {
                    f();
                }
                t.elapsed() / n
            })
            .collect();
        samples.sort();
        let median = samples[5];
        let mbs = bytes as f64 / median.as_secs_f64() / 1e6;
        println!("{name:44} {:>12} {:>9.1} MB/s", fmt(median), mbs);
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

fn opts(strict: bool) -> Options {
    Options::builder()
        .coding(Coding::ByteAligned)
        .strict(strict)
        .build()
        .unwrap()
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

fn encode(g: &Grammars<'_>, xml: &[u8], strict: bool) -> Vec<u8> {
    let mut w = Writer::new(
        Vec::with_capacity(xml.len() / 4),
        WriterConfig::new(opts(strict), g),
    )
    .unwrap();
    parse(g, xml, &mut w);
    w.into_sink()
}

fn decode(g: &Grammars<'_>, stream: &[u8], sink: &mut impl EventSink) {
    let mut r: Reader = Reader::new(ReaderConfig::new(g)).unwrap();
    let mut rest = stream;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        if exi::transcode(&mut r, sink).unwrap() == Status::End {
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

fn case(b: &Bench, strings: &GlobalStrings, name: &str, xml: &[u8]) {
    let none = GrammarSet::schemaless();
    let g = none.attach(strings).unwrap();
    let evs = events(&g, xml);
    let n = xml.len();

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

    let stream = encode(&g, xml, false);
    b.run(&format!("encode {name} schema-less"), n, || {
        black_box(encode(&g, xml, false));
    });
    b.run(&format!("encode-events {name} schema-less"), n, || {
        let mut w = Writer::new(
            Vec::with_capacity(n / 4),
            WriterConfig::new(opts(false), &g),
        )
        .unwrap();
        for e in &evs {
            w.event(&e.as_event()).unwrap();
        }
        black_box(w.into_sink());
    });
    b.run(&format!("decode {name} schema-less"), n, || {
        let mut s = Null(0);
        decode(&g, &stream, &mut s);
        black_box(s.0);
    });
    b.run(&format!("decode-to-xml {name} schema-less"), n, || {
        let mut w = XmlWriter::new(Vec::with_capacity(n), XmlWriterConfig::new());
        decode(&g, &stream, &mut w);
        black_box(w.into_sink());
    });

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
    let strict = encode(&gs, xml, true);
    b.run(&format!("encode {name} strict"), n, || {
        black_box(encode(&gs, xml, true));
    });
    b.run(&format!("decode {name} strict"), n, || {
        let mut s = Null(0);
        decode(&gs, &strict, &mut s);
        black_box(s.0);
    });
    if b.wanted("size") || b.words.is_empty() {
        println!(
            "{:44} xml {n}, exi {}, strict exi {}, exig {}",
            format!("size {name}"),
            stream.len(),
            strict.len(),
            exig.len()
        );
    }
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
    if let Ok(dir) = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/data")) {
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
    let b = Bench {
        words: args.into_iter().filter(|a| !a.starts_with('-')).collect(),
    };
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    for (name, xml) in corpus() {
        case(&b, &strings, &name, &xml);
    }
}
