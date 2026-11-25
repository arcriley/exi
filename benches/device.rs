// benches/device.rs
//
// Fixed-schema sensor decoding and heap benchmarks
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Host baseline for fixed-schema firmware.
//!
//!     cargo bench --bench device --no-default-features --features
//! std,bit-packed
//!
//! Heap figures count live requested Rust allocation bytes above each
//! phase's starting point. They exclude inline buffers, allocator
//! overhead, transient allocator-internal reallocation space and
//! firmware memory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::{Duration, Instant};

use exi::grammar::informed::{IntegerRep, Prod, Representation};
use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{
    BinaryKind, Coding, Event, EventSink, Header, Limits, Options, OwnedName,
    Reader, ReaderConfig, Step, Value, Writer, WriterConfig,
};

const NS: &str = "urn:example:sensor";
fn sensor_grammar() -> GrammarSet {
    let mut b = GrammarSet::builder();
    let int = b.simple_type(None, Representation::Integer(IntegerRep::Signed));
    let binary =
        b.simple_type(None, Representation::Binary(BinaryKind::Base64));
    let int_type = b.simple_content_type(None, int);
    let binary_type = b.simple_content_type(None, binary);
    let temperature =
        b.element(OwnedName::new(NS, "temperature"), int_type, false);
    let samples = b.element(OwnedName::new(NS, "samples"), binary_type, false);
    let reading_type = b.complex_type(
        None,
        vec![
            vec![Prod::Se { element: temperature, next: 1 }],
            vec![Prod::Se { element: samples, next: 2 }],
            vec![Prod::EE],
        ],
    );
    let reading =
        b.element(OwnedName::new(NS, "reading"), reading_type, false);
    b.global(reading);
    b.build().unwrap()
}

#[derive(Clone, Copy, Default)]
struct Heap {
    live: isize,
    peak: isize,
    checkpoint: Option<isize>,
}

thread_local! {
    static HEAP: Cell<Option<Heap>> = const { Cell::new(None) };
}

struct MeteredSystem;

#[global_allocator]
static ALLOCATOR: MeteredSystem = MeteredSystem;

fn account(delta: isize) {
    let _ = HEAP.try_with(|meter| {
        if let Some(mut heap) = meter.get() {
            heap.live += delta;
            heap.peak = heap.peak.max(heap.live);
            meter.set(Some(heap));
        }
    });
}

// SAFETY: pointer operations delegate to System with the original
// layouts.
unsafe impl GlobalAlloc for MeteredSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            account(layout.size() as isize);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            account(layout.size() as isize);
        }
        ptr
    }

    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: Layout,
        size: usize,
    ) -> *mut u8 {
        let ptr = unsafe { System.realloc(ptr, layout, size) };
        if !ptr.is_null() {
            account(size as isize - layout.size() as isize);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) };
    }
}

fn record_live_heap() {
    if let Some(mut heap) = HEAP.get() {
        heap.checkpoint = Some(heap.live);
        HEAP.set(Some(heap));
    }
}

fn run<T>(label: &str, mut work: impl FnMut() -> T) -> T {
    let warmup = Instant::now();
    let mut runs = 0;
    while warmup.elapsed() < Duration::from_millis(50) {
        black_box(work());
        runs += 1;
    }
    let per_run = warmup.elapsed() / runs.max(1);
    let runs = (50_000_000 / per_run.as_nanos().max(1)).max(1) as u32;
    let mut samples = [Duration::ZERO; 5];
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..runs {
            black_box(work());
        }
        *sample = start.elapsed() / runs;
    }
    samples.sort_unstable();

    HEAP.set(Some(Heap::default()));
    let result = work();
    let heap = HEAP.replace(None).unwrap();
    println!(
        "{label:34} {:>10.2} µs {:>8} B {:>8} B",
        samples[2].as_secs_f64() * 1e6,
        heap.peak,
        heap.checkpoint.unwrap_or(heap.live),
    );
    result
}

fn encode(g: &Grammars<'_>, options: &Options, samples: &[u8]) -> Vec<u8> {
    let cfg = WriterConfig::new(options.clone(), g)
        .header(Header::Bare { cookie: false });
    let mut w = Writer::new(Vec::new(), cfg).unwrap();
    w.start_element((NS, "reading")).unwrap();
    w.start_element((NS, "temperature")).unwrap();
    w.event(&Event::Characters(Value::Int(215))).unwrap();
    w.end_element().unwrap();
    w.start_element((NS, "samples")).unwrap();
    w.event(&Event::Characters(Value::Binary(samples, BinaryKind::Base64)))
        .unwrap();
    w.end_element().unwrap();
    w.end_element().unwrap();
    w.end_document().unwrap();
    w.into_sink()
}

fn decode<const N: usize>(
    g: &Grammars<'_>,
    options: &Options,
    mut input: &[u8],
    chunk: usize,
    samples: &[u8],
) {
    let cfg =
        ReaderConfig::new(g).options(options.clone()).limits(Limits::device());
    let mut r = Reader::<N>::new(cfg).unwrap();
    let mut values = 0;
    loop {
        let n = r.feed(&input[..input.len().min(chunk)]);
        input = &input[n..];
        if input.is_empty() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(Event::Characters(value), _) => {
                    match (values, value) {
                        (0, Value::Int(215)) => {}
                        (1, Value::Binary(data, _)) => {
                            assert_eq!(data, samples)
                        }
                        _ => panic!("unexpected sensor value: {value:?}"),
                    }
                    values += 1;
                }
                Step::Event(_, _) => {}
                Step::NeedInput => break,
                Step::End => {
                    assert_eq!(values, 2);
                    assert!(input.is_empty());
                    record_live_heap();
                    return;
                }
            }
        }
        assert!(n > 0 && !input.is_empty(), "reader stopped making progress");
    }
}

fn main() {
    let set = sensor_grammar();
    assert!(set.is_closed());
    let exig = set.to_exig().unwrap();
    println!("Strict bit-packed sensor schema, {} EXIG bytes", exig.len());
    println!(
        "{:34} {:>13} {:>10} {:>10}",
        "Phase", "Median", "Peak heap", "Live at end"
    );
    let limits = Limits::device();
    let set = run("Load .exig", || {
        GrammarSet::from_exig_with(black_box(&exig), limits).unwrap()
    });
    run("Attach grammar to fresh pool", || {
        let global = GlobalStrings::new(GlobalConfig::device()).unwrap();
        let g = set.attach(&global).unwrap();
        black_box(g.initial());
        record_live_heap();
    });
    let global = GlobalStrings::new(GlobalConfig::device()).unwrap();
    let g = set.attach(&global).unwrap();
    let options = Options::builder()
        .coding(Coding::BitPacked)
        .strict(true)
        .value_partition_capacity(Some(0))
        .build()
        .unwrap();
    for size in [64, 2048] {
        let samples: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let stream = encode(&g, &options, &samples);
        println!("\n{size} B binary value, {} B EXI input", stream.len());
        run("4 KiB ring, complete input", || {
            decode::<4096>(
                &g,
                &options,
                black_box(&stream),
                stream.len(),
                &samples,
            )
        });
        run("4 KiB ring, one-byte feeds", || {
            decode::<4096>(&g, &options, black_box(&stream), 1, &samples)
        });
        run("256 B ring, 32-byte feeds", || {
            decode::<256>(&g, &options, black_box(&stream), 32, &samples)
        });
    }
}
