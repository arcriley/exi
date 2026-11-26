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
//! cargo bench --bench xhtml5 --features xhtml5,html5ever -- [page.html
//! ...]

use std::hint::black_box;
use std::time::{Duration, Instant};

use exi::html::profile;

fn median_us(mut operation: impl FnMut()) -> f64 {
    let mut samples = [0.0; 7];
    for sample in &mut samples {
        let start = Instant::now();
        let mut runs = 0;
        loop {
            operation();
            runs += 1;
            let elapsed = start.elapsed();
            if elapsed >= Duration::from_millis(70) {
                *sample = elapsed.as_secs_f64() * 1e6 / f64::from(runs);
                break;
            }
        }
    }
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    let mut files: Vec<_> =
        std::env::args_os().skip(1).filter(|s| s != "--bench").collect();
    if files.is_empty() {
        files.push("fixtures/input/html/page.html".into());
        files.push("fixtures/input/html/fragmentary.html".into());
    }
    println!(
        "{:>10} {:>10} {:>10}  page",
        "HTML bytes", "encode µs", "decode µs"
    );
    for path in files {
        let html = std::fs::read_to_string(&path).expect("read UTF-8 page");
        let exi = profile::encode(&html, 1).unwrap();
        profile::decode(&exi, 1).unwrap();
        let encode = median_us(|| {
            black_box(profile::encode(black_box(&html), 1).unwrap());
        });
        let decode = median_us(|| {
            black_box(profile::decode(black_box(&exi), 1).unwrap());
        });
        println!(
            "{:>10} {encode:>10.1} {decode:>10.1}  {}",
            html.len(),
            std::path::Path::new(&path).display()
        );
    }
}
