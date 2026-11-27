// benches/xhtml5.rs
//
// End-to-end XHTML5 profile benchmark
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Times `profile::encode` and `profile::decode` on HTML pages, next to
//! schema-less EXI of the same pages, and gives the size of each.
//!
//!     cargo bench --bench xhtml5 --features xhtml5,html5ever --
//! [page.html ...]
//!
//! Without pages it uses the two in `fixtures/input/html`. Both forms
//! use the profile's options: byte-aligned, a bare header, and no
//! comments. Each call makes its own string pool and attaches its
//! grammars, as `profile::encode` does, so the times compare whole
//! calls. A figure is the median of 7 samples of about 70 ms each;
//! `target/bench/xhtml5.csv` also has the fastest and slowest, and
//! `target/bench/xhtml5.meta` the build and machine.

mod common;

use std::fmt::Write as _;
use std::hint::black_box;
use std::time::{Duration, Instant};

use common::Stats;
use exi::grammar::GrammarSet;
use exi::html::{HtmlOptions, HtmlWriter, encode_html, profile};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{
    Header, Options, Reader, ReaderConfig, SchemaId, Status, Writer,
    WriterConfig,
};

/// Microseconds per call: 7 samples, each the mean over about 70 ms.
fn time_us(mut operation: impl FnMut()) -> Stats {
    let samples = (0..7)
        .map(|_| {
            let start = Instant::now();
            let mut runs = 0;
            loop {
                operation();
                runs += 1;
                let elapsed = start.elapsed();
                if elapsed >= Duration::from_millis(70) {
                    break elapsed / runs;
                }
            }
        })
        .collect();
    Stats::of(samples)
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

/// The profile's options without its schema.
fn schemaless_options() -> Options {
    profile::options(1)
        .unwrap()
        .to_builder()
        .schema_id(SchemaId::Absent)
        .build()
        .unwrap()
}

/// Schema-less EXI of a page.
fn schemaless_encode(html: &str) -> Vec<u8> {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(schemaless_options(), &g)
            .header(Header::Bare { cookie: false }),
    )
    .unwrap();
    encode_html(html, &mut w, &HtmlOptions::new().keep_comments(false))
        .unwrap();
    w.into_sink()
}

fn schemaless_decode(exi: &[u8]) -> Vec<u8> {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let mut r: Reader =
        Reader::new(ReaderConfig::new(&g).options(schemaless_options()))
            .unwrap();
    let mut out = HtmlWriter::new(Vec::new());
    let mut rest = exi;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        if exi::transcode(&mut r, &mut out).unwrap() == Status::End {
            return out.into_sink();
        }
    }
}

fn main() {
    let mut files: Vec<_> = std::env::args_os()
        .skip(1)
        .filter(|s| s != "--bench")
        .map(std::path::PathBuf::from)
        .collect();
    if files.is_empty() {
        files.push(common::package("fixtures/input/html/page.html"));
        files.push(common::package("fixtures/input/html/fragmentary.html"));
    }
    println!(
        "{:>10} {:>10} {:>10}   {:>9} {:>9} {:>9} {:>9}  page",
        "HTML",
        "xhtml5",
        "schemaless",
        "x5 enc µs",
        "x5 dec µs",
        "sl enc µs",
        "sl dec µs"
    );
    let mut csv = String::from(
        "page,html_octets,xhtml5_octets,schemaless_octets,\
         xhtml5_encode_us,xhtml5_decode_us,schemaless_encode_us,schemaless_decode_us,\
         xhtml5_encode_min_us,xhtml5_encode_max_us,xhtml5_decode_min_us,xhtml5_decode_max_us,\
         schemaless_encode_min_us,schemaless_encode_max_us,schemaless_decode_min_us,schemaless_decode_max_us\n",
    );
    for path in files {
        let html = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let x5 = profile::encode(&html, 1).unwrap();
        profile::decode(&x5, 1).unwrap();
        let sl = schemaless_encode(&html);
        schemaless_decode(&sl);
        let x5e = time_us(|| {
            black_box(profile::encode(black_box(&html), 1).unwrap());
        });
        let x5d = time_us(|| {
            black_box(profile::decode(black_box(&x5), 1).unwrap());
        });
        let sle = time_us(|| {
            black_box(schemaless_encode(black_box(&html)));
        });
        let sld = time_us(|| {
            black_box(schemaless_decode(black_box(&sl)));
        });
        println!(
            "{:>10} {:>10} {:>10}   {:>9.1} {:>9.1} {:>9.1} {:>9.1}  {}",
            html.len(),
            x5.len(),
            sl.len(),
            us(x5e.median),
            us(x5d.median),
            us(sle.median),
            us(sld.median),
            path.display()
        );
        let _ = write!(
            csv,
            "\"{}\",{},{},{}",
            path.display(),
            html.len(),
            x5.len(),
            sl.len()
        );
        for s in [x5e, x5d, sle, sld] {
            let _ = write!(csv, ",{:.2}", us(s.median));
        }
        for s in [x5e, x5d, sle, sld] {
            let _ = write!(csv, ",{:.2},{:.2}", us(s.min), us(s.max));
        }
        csv.push('\n');
    }
    let out = common::out_dir().join("xhtml5.csv");
    std::fs::write(&out, csv).unwrap();
    println!("wrote {}", out.display());
    common::write_meta("xhtml5");
}
