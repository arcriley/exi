// benches/xml.rs
//
// XML text decoding and DOM tree benchmarks vs libxml2
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Baseline CPU speed and heap usage for decoding XML text into a DOM
//! tree (`exi::dom::Document`) compared against system `libxml2`
//! (`xmlReadMemory` + `xmlFreeDoc`).
//!
//!     cargo bench --bench xml --features dom
//!     cargo bench --bench xml --features dom -- --quick
//!     cargo bench --bench xml --features dom -- svg-1m
//!     cargo bench --bench xml --features dom -- path/to/file.xml
//!
//! Speed and heap are measured in two separate passes so allocator
//! accounting never perturbs CPU timing:
//!
//! 1. **CPU speed pass:** Rust's `#[global_allocator]` fast-paths a
//!    relaxed `AtomicBool` (`false`) straight to `System`, and `libxml2`
//!    runs with its stock glibc `malloc`/`free` before `xmlMemSetup` is
//!    installed.
//! 2. **Heap pass:** enables exact requested-byte accounting on Rust's
//!    `GlobalAlloc` for `exi` cases, then resets `libxml2`
//!    (`xmlCleanupParser` + `xmlMemSetup` + `xmlInitParser`) to measure
//!    exact requested bytes (`peak`, live `tree` before free, and
//!    allocation count) for `libxml2` cases.
//!
//! Writes `target/bench/xml.csv` and `target/bench/xml.meta`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::Stats;
use exi::dom::{Document, DocumentBuilder};
use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{Event, EventSink, Status};

// =======================================================================================
// Metered allocator (active only during the heap measurement pass)
// =======================================================================================

static METERING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Default)]
struct Heap {
    live: isize,
    peak: isize,
    tree: Option<isize>,
    allocs: u64,
}

thread_local! {
    static HEAP: Cell<Heap> = const { Cell::new(Heap {
        live: 0,
        peak: 0,
        tree: None,
        allocs: 0,
    }) };
}

struct MeteredSystem;

#[global_allocator]
static ALLOCATOR: MeteredSystem = MeteredSystem;

#[inline]
fn account_alloc(size: usize) {
    let _ = HEAP.try_with(|cell| {
        let mut h = cell.get();
        h.live += size as isize;
        if h.live > h.peak {
            h.peak = h.live;
        }
        h.allocs += 1;
        cell.set(h);
    });
}

#[inline]
fn account_realloc(old: usize, new: usize) {
    let _ = HEAP.try_with(|cell| {
        let mut h = cell.get();
        h.live += new as isize - old as isize;
        if h.live > h.peak {
            h.peak = h.live;
        }
        h.allocs += 1;
        cell.set(h);
    });
}

#[inline]
fn account_dealloc(size: usize) {
    let _ = HEAP.try_with(|cell| {
        let mut h = cell.get();
        h.live -= size as isize;
        cell.set(h);
    });
}

fn snapshot_tree_heap() {
    let _ = HEAP.try_with(|cell| {
        let mut h = cell.get();
        h.tree = Some(h.live);
        cell.set(h);
    });
}

fn measure_heap(f: impl FnOnce()) -> Heap {
    HEAP.with(|c| c.set(Heap::default()));
    METERING.store(true, Ordering::SeqCst);
    f();
    METERING.store(false, Ordering::SeqCst);
    HEAP.with(|c| c.get())
}

// SAFETY: delegates directly to System with identical layouts, so blocks
// allocated with METERING off and freed with METERING on (or vice versa)
// remain layout-compatible.
unsafe impl GlobalAlloc for MeteredSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if METERING.load(Ordering::Relaxed) && !p.is_null() {
            account_alloc(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if METERING.load(Ordering::Relaxed) && !p.is_null() {
            account_alloc(layout.size());
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if METERING.load(Ordering::Relaxed) && !p.is_null() {
            account_realloc(layout.size(), new_size);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if METERING.load(Ordering::Relaxed) {
            account_dealloc(layout.size());
        }
        unsafe { System.dealloc(ptr, layout) };
    }
}

// =======================================================================================
// Optional runtime binding to system libxml2 (no external Rust crates)
// =======================================================================================

#[cfg(unix)]
mod xml2 {
    use super::*;

    const RTLD_NOW: c_int = 2;
    pub const XML_PARSE_NOBLANKS: c_int = 1 << 8;
    pub const XML_PARSE_NONET: c_int = 1 << 11;
    pub const XML_PARSE_NODICT: c_int = 1 << 12;

    type FreeFn = unsafe extern "C" fn(*mut c_void);
    type MallocFn = unsafe extern "C" fn(usize) -> *mut c_void;
    type ReallocFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
    type StrdupFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn malloc(size: usize) -> *mut c_void;
        fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        fn free(ptr: *mut c_void);
        fn strlen(s: *const c_char) -> usize;
    }

    /// 16-byte aligned prefix storing requested size for C allocations
    /// during the libxml2 heap pass.
    #[repr(C, align(16))]
    struct CHeader {
        size: usize,
        _pad: usize,
    }

    unsafe extern "C" fn metered_malloc(size: usize) -> *mut c_void {
        let total = core::mem::size_of::<CHeader>().checked_add(size).unwrap();
        // SAFETY: libc::malloc returns 16-byte aligned memory on 64-bit Unix.
        let raw = unsafe { malloc(total) } as *mut CHeader;
        if raw.is_null() {
            return ptr::null_mut();
        }
        unsafe {
            (*raw).size = size;
            (*raw)._pad = 0;
        }
        if METERING.load(Ordering::Relaxed) {
            account_alloc(size);
        }
        unsafe { raw.add(1) as *mut c_void }
    }

    unsafe extern "C" fn metered_free(ptr: *mut c_void) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: every non-null pointer came from metered_malloc/metered_realloc.
        let raw = unsafe { (ptr as *mut CHeader).sub(1) };
        let size = unsafe { (*raw).size };
        if METERING.load(Ordering::Relaxed) {
            account_dealloc(size);
        }
        unsafe { free(raw as *mut c_void) };
    }

    unsafe extern "C" fn metered_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
        if ptr.is_null() {
            return unsafe { metered_malloc(size) };
        }
        // SAFETY: ptr came from metered_malloc/metered_realloc.
        let old_raw = unsafe { (ptr as *mut CHeader).sub(1) };
        let old_size = unsafe { (*old_raw).size };
        let total = core::mem::size_of::<CHeader>().checked_add(size).unwrap();
        let raw = unsafe { realloc(old_raw as *mut c_void, total) } as *mut CHeader;
        if raw.is_null() {
            return ptr::null_mut();
        }
        unsafe { (*raw).size = size };
        if METERING.load(Ordering::Relaxed) {
            account_realloc(old_size, size);
        }
        unsafe { raw.add(1) as *mut c_void }
    }

    unsafe extern "C" fn metered_strdup(s: *const c_char) -> *mut c_char {
        if s.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: libxml2 passes NUL-terminated C strings to xmlMemStrdup.
        let len = unsafe { strlen(s) } + 1;
        let dst = unsafe { metered_malloc(len) } as *mut c_char;
        if !dst.is_null() {
            unsafe { ptr::copy_nonoverlapping(s, dst, len) };
        }
        dst
    }

    pub struct LibXml2 {
        pub soname: &'static str,
        pub version: String,
        init_parser: unsafe extern "C" fn(),
        cleanup_parser: unsafe extern "C" fn(),
        read_memory: unsafe extern "C" fn(
            *const c_char,
            c_int,
            *const c_char,
            *const c_char,
            c_int,
        ) -> *mut c_void,
        free_doc: unsafe extern "C" fn(*mut c_void),
        get_root: unsafe extern "C" fn(*const c_void) -> *mut c_void,
        mem_setup: unsafe extern "C" fn(FreeFn, MallocFn, ReallocFn, StrdupFn) -> c_int,
    }

    impl LibXml2 {
        pub fn load() -> Option<Self> {
            let candidates: &[(&str, &CStr)] = &[
                ("libxml2.so", c"libxml2.so"),
                ("libxml2.so.16", c"libxml2.so.16"),
                ("libxml2.so.2", c"libxml2.so.2"),
                ("/usr/lib64/libxml2.so", c"/usr/lib64/libxml2.so"),
                ("/usr/lib/libxml2.so", c"/usr/lib/libxml2.so"),
                (
                    "/usr/lib/x86_64-linux-gnu/libxml2.so.2",
                    c"/usr/lib/x86_64-linux-gnu/libxml2.so.2",
                ),
            ];
            let mut handle = ptr::null_mut();
            let mut soname = "";
            for &(name, cname) in candidates {
                // SAFETY: cname is a valid CStr.
                let h = unsafe { dlopen(cname.as_ptr(), RTLD_NOW) };
                if !h.is_null() {
                    handle = h;
                    soname = name;
                    break;
                }
            }
            if handle.is_null() {
                return None;
            }
            unsafe {
                let sym = |n: &CStr| -> Option<*mut c_void> {
                    let p = dlsym(handle, n.as_ptr());
                    (!p.is_null()).then_some(p)
                };
                let init_parser = core::mem::transmute::<*mut c_void, unsafe extern "C" fn()>(
                    sym(c"xmlInitParser")?,
                );
                let cleanup_parser = core::mem::transmute::<*mut c_void, unsafe extern "C" fn()>(
                    sym(c"xmlCleanupParser")?,
                );
                let read_memory = core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(
                        *const c_char,
                        c_int,
                        *const c_char,
                        *const c_char,
                        c_int,
                    ) -> *mut c_void,
                >(sym(c"xmlReadMemory")?);
                let free_doc = core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut c_void),
                >(sym(c"xmlFreeDoc")?);
                let get_root = core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*const c_void) -> *mut c_void,
                >(sym(c"xmlDocGetRootElement")?);
                let mem_setup = core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(FreeFn, MallocFn, ReallocFn, StrdupFn) -> c_int,
                >(sym(c"xmlMemSetup")?);

                let version = match sym(c"xmlParserVersion") {
                    Some(vp) => {
                        let cptr = *(vp as *const *const c_char);
                        if cptr.is_null() {
                            "unknown".into()
                        } else {
                            CStr::from_ptr(cptr).to_string_lossy().into_owned()
                        }
                    }
                    None => "unknown".into(),
                };
                init_parser();
                Some(Self {
                    soname,
                    version,
                    init_parser,
                    cleanup_parser,
                    read_memory,
                    free_doc,
                    get_root,
                    mem_setup,
                })
            }
        }

        #[inline]
        pub fn parse_and_free(&self, xml: &[u8], flags: c_int, snapshot: bool) {
            let len = c_int::try_from(xml.len()).expect("document fits c_int");
            // SAFETY: xml is a valid byte slice of length `len`; doc is freed below.
            unsafe {
                let doc = (self.read_memory)(
                    xml.as_ptr() as *const c_char,
                    len,
                    c"bench.xml".as_ptr(),
                    ptr::null(),
                    flags,
                );
                assert!(!doc.is_null(), "libxml2 xmlReadMemory failed");
                assert!(
                    !(self.get_root)(doc).is_null(),
                    "libxml2 parsed a document without a root element"
                );
                black_box(doc);
                if snapshot {
                    snapshot_tree_heap();
                }
                (self.free_doc)(doc);
            }
        }

        pub fn enable_metered_heap(&self) {
            // SAFETY: no xmlDoc is live; resets parser state before installing
            // custom memory hooks and re-initializing libxml2.
            unsafe {
                (self.cleanup_parser)();
                let rc = (self.mem_setup)(
                    metered_free,
                    metered_malloc,
                    metered_realloc,
                    metered_strdup,
                );
                assert_eq!(rc, 0, "xmlMemSetup failed");
                (self.init_parser)();
            }
        }

        pub fn cleanup(&self) {
            unsafe { (self.cleanup_parser)() };
        }
    }
}

// =======================================================================================
// Deterministic XML test corpus
// =======================================================================================

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

/// Namespaced, attribute-heavy SVG drawing (matches `benches/throughput.rs`).
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

/// Element- and text-heavy orders feed (matches `benches/throughput.rs`).
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

fn build_corpus(files: &[PathBuf], quick: bool) -> Vec<(String, Vec<u8>)> {
    if !files.is_empty() {
        return files
            .iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string());
                let bytes =
                    std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                (name, bytes)
            })
            .collect();
    }
    let mut docs = Vec::new();
    let logo = common::package("fixtures/input/infer/logo.svg");
    if let Ok(bytes) = std::fs::read(&logo) {
        docs.push(("logo-1k".to_string(), bytes));
    }
    docs.push(("orders-64k".to_string(), orders(64 << 10).into_bytes()));
    if !quick {
        docs.push(("svg-1m".to_string(), svg(1 << 20).into_bytes()));
        docs.push(("orders-1m".to_string(), orders(1 << 20).into_bytes()));
    }
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

// =======================================================================================
// Benchmark operations & timing
// =======================================================================================

struct NullSink(u64);

impl EventSink for NullSink {
    fn event(&mut self, e: &Event<'_>) -> exi::Result<()> {
        black_box(e);
        self.0 += 1;
        Ok(())
    }
}

fn exi_xml_events(g: &Grammars<'_>, xml: &[u8], snapshot: bool) {
    let mut r: XmlReader = XmlReader::new(
        XmlConfig::new()
            .report_comments(true)
            .report_pis(true)
            .report_doctype(true),
        g,
    );
    let mut sink = NullSink(0);
    let mut rest = xml;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        if exi::transcode(&mut r, &mut sink).unwrap() == Status::End {
            break;
        }
    }
    black_box(sink.0);
    if snapshot {
        snapshot_tree_heap();
    }
}

fn exi_dom_shared(strings: &GlobalStrings, xml: &[u8], noblanks: bool, snapshot: bool) {
    let builder = DocumentBuilder::new(strings).ignore_blanks(noblanks);
    let doc = Document::parse_xml_with(strings, xml, builder).unwrap();
    assert!(doc.root().is_some());
    black_box(&doc);
    if snapshot {
        snapshot_tree_heap();
    }
}

fn exi_dom_fresh(xml: &[u8], snapshot: bool) {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let doc = Document::parse_xml(&strings, xml).unwrap();
    assert!(doc.root().is_some());
    black_box(&doc);
    if snapshot {
        snapshot_tree_heap();
    }
}

struct TimingConfig {
    warmup: Duration,
    sample_window: Duration,
    samples: usize,
}

fn time_op(cfg: &TimingConfig, mut f: impl FnMut()) -> (Stats, u32) {
    let start = Instant::now();
    let mut warmup_runs = 0u32;
    while start.elapsed() < cfg.warmup || warmup_runs < 2 {
        f();
        warmup_runs += 1;
    }
    let per = start.elapsed() / warmup_runs;
    let runs = ((cfg.sample_window.as_nanos() / per.as_nanos().max(1)) as u32).max(1);
    let stats = Stats::of(
        (0..cfg.samples)
            .map(|_| {
                let t = Instant::now();
                for _ in 0..runs {
                    f();
                }
                t.elapsed() / runs
            })
            .collect(),
    );
    (stats, runs)
}

fn fmt_time(d: Duration) -> String {
    let ns = d.as_nanos() as f64;
    if ns >= 1e9 {
        format!("{:.3} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.3} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} µs", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

fn fmt_bytes(b: isize) -> String {
    let abs = b.unsigned_abs() as f64;
    let sign = if b < 0 { "-" } else { "" };
    if abs >= (1 << 20) as f64 {
        format!("{sign}{:.2} MiB", abs / (1 << 20) as f64)
    } else if abs >= (1 << 10) as f64 {
        format!("{sign}{:.1} KiB", abs / (1 << 10) as f64)
    } else {
        format!("{b} B")
    }
}

#[derive(Clone, Copy)]
enum Kind {
    ExiEvents,
    ExiDomShared { noblanks: bool },
    ExiDomFresh,
    #[cfg(unix)]
    LibXml2 { flags: c_int },
}

struct Row {
    doc_index: usize,
    doc_name: String,
    case_name: &'static str,
    kind: Kind,
    bytes: usize,
    stats: Stats,
    runs: u32,
    heap: Heap,
}

fn main() {
    let mut files = Vec::new();
    let mut words = Vec::new();
    let mut quick = false;
    for arg in std::env::args().skip(1) {
        if arg == "--bench" {
            continue;
        }
        if arg == "--quick" {
            quick = true;
            continue;
        }
        let p = PathBuf::from(&arg);
        if p.is_file() {
            files.push(p);
        } else if !arg.starts_with('-') {
            words.push(arg);
        }
    }

    let tcfg = if quick {
        TimingConfig {
            warmup: Duration::from_millis(40),
            sample_window: Duration::from_millis(30),
            samples: 5,
        }
    } else {
        TimingConfig {
            warmup: Duration::from_millis(100),
            sample_window: Duration::from_millis(70),
            samples: 9,
        }
    };

    #[cfg(unix)]
    let libxml2 = xml2::LibXml2::load();
    #[cfg(unix)]
    match &libxml2 {
        Some(x) => println!(
            "Loaded {} (xmlParserVersion = {})",
            x.soname, x.version
        ),
        None => println!("libxml2 shared library not found: skipping libxml2 rows"),
    }
    #[cfg(not(unix))]
    println!("non-Unix target: skipping libxml2 rows");

    let docs = build_corpus(&files, quick);
    let wanted = |doc: &str, case: &str| -> bool {
        let full = format!("{doc} {case}");
        words.iter().all(|w| full.contains(w.as_str()))
    };

    let mut cases: Vec<(&'static str, Kind)> = vec![
        ("exi xml events (L1)", Kind::ExiEvents),
        ("exi dom shared-pool", Kind::ExiDomShared { noblanks: false }),
        ("exi dom fresh-pool", Kind::ExiDomFresh),
        ("exi dom noblanks", Kind::ExiDomShared { noblanks: true }),
    ];
    #[cfg(unix)]
    if libxml2.is_some() {
        cases.push((
            "libxml2 default (dict)",
            Kind::LibXml2 { flags: xml2::XML_PARSE_NONET },
        ));
        cases.push((
            "libxml2 nodict",
            Kind::LibXml2 {
                flags: xml2::XML_PARSE_NONET | xml2::XML_PARSE_NODICT,
            },
        ));
        cases.push((
            "libxml2 noblanks",
            Kind::LibXml2 {
                flags: xml2::XML_PARSE_NONET | xml2::XML_PARSE_NOBLANKS,
            },
        ));
    }

    // Shared pool and grammar attachment for warm-pool cases.
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let schemaless = GrammarSet::schemaless();
    let g = schemaless.attach(&strings).unwrap();

    // ---------------------------------------------------------------------------
    // Pass 1: CPU speed (METERING = false, libxml2 on untouched system malloc)
    // ---------------------------------------------------------------------------
    let mut rows: Vec<Row> = Vec::new();
    for (di, (doc_name, xml)) in docs.iter().enumerate() {
        for &(case_name, kind) in &cases {
            if !wanted(doc_name, case_name) {
                continue;
            }
            let (stats, runs) = match kind {
                Kind::ExiEvents => time_op(&tcfg, || exi_xml_events(&g, black_box(xml), false)),
                Kind::ExiDomShared { noblanks } => time_op(&tcfg, || {
                    exi_dom_shared(&strings, black_box(xml), noblanks, false)
                }),
                Kind::ExiDomFresh => time_op(&tcfg, || exi_dom_fresh(black_box(xml), false)),
                #[cfg(unix)]
                Kind::LibXml2 { flags } => {
                    let x = libxml2.as_ref().unwrap();
                    time_op(&tcfg, || x.parse_and_free(black_box(xml), flags, false))
                }
            };
            rows.push(Row {
                doc_index: di,
                doc_name: doc_name.clone(),
                case_name,
                kind,
                bytes: xml.len(),
                stats,
                runs,
                heap: Heap::default(),
            });
        }
    }

    // ---------------------------------------------------------------------------
    // Pass 2: Exact requested heap bytes (Rust GlobalAlloc, then libxml2 xmlMemSetup)
    // ---------------------------------------------------------------------------
    for row in &mut rows {
        let xml = &docs[row.doc_index].1;
        match row.kind {
            Kind::ExiEvents => {
                // Warm pool capacity once before measuring steady-state event parse heap.
                exi_xml_events(&g, xml, false);
                row.heap = measure_heap(|| exi_xml_events(&g, xml, true));
            }
            Kind::ExiDomShared { noblanks } => {
                // Warm pool table capacity once so shared-pool measures per-doc tree heap.
                exi_dom_shared(&strings, xml, noblanks, false);
                // Chunks kept from earlier trees would be reused unseen:
                // start from an empty cache so the whole tree is counted.
                exi::dom::release_cache();
                row.heap = measure_heap(|| exi_dom_shared(&strings, xml, noblanks, true));
            }
            Kind::ExiDomFresh => {
                exi::dom::release_cache();
                row.heap = measure_heap(|| exi_dom_fresh(xml, true));
            }
            #[cfg(unix)]
            Kind::LibXml2 { .. } => {}
        }
    }

    #[cfg(unix)]
    if let Some(x) = &libxml2 {
        x.enable_metered_heap();
        for row in &mut rows {
            if let Kind::LibXml2 { flags } = row.kind {
                let xml = &docs[row.doc_index].1;
                row.heap = measure_heap(|| x.parse_and_free(xml, flags, true));
            }
        }
        x.cleanup();
    }

    // ---------------------------------------------------------------------------
    // Report table, CSV, and metadata
    // ---------------------------------------------------------------------------
    let mut csv = String::from(
        "document,case,input_octets,runs_per_sample,samples,median_ns,min_ns,max_ns,mb_s,\
         peak_heap_bytes,tree_heap_bytes,after_free_bytes,allocs\n",
    );
    let mut cur_doc: Option<usize> = None;
    for r in &rows {
        if cur_doc != Some(r.doc_index) {
            cur_doc = Some(r.doc_index);
            println!(
                "\n== {} ({} octets, {}) ==",
                r.doc_name,
                r.bytes,
                fmt_bytes(r.bytes as isize)
            );
            println!(
                "{:24} {:>11} {:>9} {:>7} {:>11} {:>11} {:>7} {:>9}",
                "Case",
                "Median",
                "Speed",
                "Spread",
                "Peak heap",
                "Tree heap",
                "Tree/in",
                "Allocs"
            );
        }
        let mbs = r.bytes as f64 / r.stats.median.as_secs_f64() / 1e6;
        let tree = r.heap.tree.unwrap_or(r.heap.live);
        let ratio = tree as f64 / r.bytes.max(1) as f64;
        println!(
            "{:24} {:>11} {:>6.1} MB/s {:>5.1}% {:>11} {:>11} {:>6.2}x {:>9}",
            r.case_name,
            fmt_time(r.stats.median),
            mbs,
            100.0 * r.stats.spread(),
            fmt_bytes(r.heap.peak),
            fmt_bytes(tree),
            ratio,
            r.heap.allocs,
        );
        let _ = writeln!(
            csv,
            "\"{}\",\"{}\",{},{},{},{},{},{},{mbs:.2},{},{},{},{}",
            r.doc_name,
            r.case_name,
            r.bytes,
            r.runs,
            r.stats.samples,
            r.stats.median.as_nanos(),
            r.stats.min.as_nanos(),
            r.stats.max.as_nanos(),
            r.heap.peak,
            tree,
            r.heap.live,
            r.heap.allocs,
        );
    }

    let csv_path = common::out_dir().join("xml.csv");
    std::fs::write(&csv_path, csv).unwrap();
    println!("\nwrote {}", csv_path.display());
    common::write_meta("xml");
    #[cfg(unix)]
    if let Some(x) = &libxml2 {
        let meta_path = common::out_dir().join("xml.meta");
        let mut meta = std::fs::read_to_string(&meta_path).unwrap_or_default();
        let _ = writeln!(meta, "libxml2_soname: {}", x.soname);
        let _ = writeln!(meta, "libxml2_version: {}", x.version);
        std::fs::write(&meta_path, meta).unwrap();
    }
}
