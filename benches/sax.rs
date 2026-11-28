// benches/sax.rs
//
// Layer 1 XML events vs expat and libxml2 SAX2
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//! Event-parsing speed of libexi's Layer 1 `XmlReader` against system
//! `expat` (namespace-aware) and `libxml2` (SAX2), all feeding handlers that
//! only count what they see. No tree is built.
//!
//!     cargo bench --bench sax --features std,xml
//!     cargo bench --bench sax --features std,xml -- --quick
//!     cargo bench --bench sax --features std,xml -- path/to/file.xml
//!
//! The C libraries are loaded at run time with `dlopen`; a missing one is skipped.
//! Each parser counts elements, attributes (not `xmlns`) and text octets. The
//! counts must agree across parsers or the bench stops: they show that every
//! parser did the same work.
//!
//! Writes `target/bench/sax.csv` and `target/bench/sax.meta`.

mod common;

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::PathBuf;
use std::ptr;
use std::time::{Duration, Instant};

use common::Stats;
use common::corpus::{orders, svg};
use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{Event, EventSink, Status, Value};

/// What a parser saw. Equal across parsers for the same document.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Counts {
    elements: u64,
    attributes: u64,
    text: u64,
}

// ---------------------------------------------------------------------------
// libexi Layer 1
// ---------------------------------------------------------------------------

impl EventSink for Counts {
    fn event(&mut self, e: &Event<'_>) -> exi::Result<()> {
        match e {
            Event::StartElement(n) => {
                black_box(n);
                self.elements += 1;
            }
            Event::Attribute(n, _) => {
                black_box(n);
                self.attributes += 1;
            }
            Event::Characters(v) => {
                if let Value::Str(s) = v {
                    self.text += s.len() as u64;
                } else {
                    black_box(v);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// dlopen helper
// ---------------------------------------------------------------------------

#[cfg_attr(target_os = "linux", link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

fn open_lib(names: &[&'static CStr]) -> Option<*mut c_void> {
    for n in names {
        // SAFETY: valid C string.
        let h = unsafe { dlopen(n.as_ptr(), 2) };
        if !h.is_null() {
            return Some(h);
        }
    }
    None
}

unsafe fn sym<T>(h: *mut c_void, name: &CStr) -> Option<T> {
    let p = unsafe { dlsym(h, name.as_ptr()) };
    // SAFETY: the caller names T as the symbol's real function type.
    (!p.is_null()).then(|| unsafe { core::mem::transmute_copy::<*mut c_void, T>(&p) })
}

// ---------------------------------------------------------------------------
// expat
// ---------------------------------------------------------------------------

type ElemStart = unsafe extern "C" fn(*mut c_void, *const c_char, *mut *const c_char);
type ElemEnd = unsafe extern "C" fn(*mut c_void, *const c_char);
type CharData = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);

struct Expat {
    version: String,
    create_ns: unsafe extern "C" fn(*const c_char, c_char) -> *mut c_void,
    set_user: unsafe extern "C" fn(*mut c_void, *mut c_void),
    set_element: unsafe extern "C" fn(*mut c_void, ElemStart, ElemEnd),
    set_chars: unsafe extern "C" fn(*mut c_void, CharData),
    parse: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_int) -> c_int,
    free: unsafe extern "C" fn(*mut c_void),
}

unsafe extern "C" fn x_start(u: *mut c_void, name: *const c_char, atts: *mut *const c_char) {
    let c = unsafe { &mut *(u as *mut Counts) };
    black_box(name);
    c.elements += 1;
    let mut a = atts;
    unsafe {
        while !(*a).is_null() {
            black_box(*a);
            black_box(*a.add(1));
            c.attributes += 1;
            a = a.add(2);
        }
    }
}
unsafe extern "C" fn x_end(_: *mut c_void, name: *const c_char) {
    black_box(name);
}
unsafe extern "C" fn x_chars(u: *mut c_void, s: *const c_char, len: c_int) {
    black_box(s);
    unsafe { (*(u as *mut Counts)).text += len as u64 };
}

impl Expat {
    fn load() -> Option<Self> {
        let h = open_lib(&[c"libexpat.so.1", c"libexpat.so", c"/usr/lib64/libexpat.so.1"])?;
        unsafe {
            let ver: unsafe extern "C" fn() -> *const c_char = sym(h, c"XML_ExpatVersion")?;
            Some(Self {
                version: CStr::from_ptr(ver()).to_string_lossy().into_owned(),
                create_ns: sym(h, c"XML_ParserCreateNS")?,
                set_user: sym(h, c"XML_SetUserData")?,
                set_element: sym(h, c"XML_SetElementHandler")?,
                set_chars: sym(h, c"XML_SetCharacterDataHandler")?,
                parse: sym(h, c"XML_Parse")?,
                free: sym(h, c"XML_ParserFree")?,
            })
        }
    }

    fn run(&self, xml: &[u8]) -> Counts {
        let mut c = Counts::default();
        unsafe {
            let p = (self.create_ns)(ptr::null(), b' ' as c_char);
            assert!(!p.is_null());
            (self.set_user)(p, &mut c as *mut Counts as *mut c_void);
            (self.set_element)(p, x_start, x_end);
            (self.set_chars)(p, x_chars);
            let len = c_int::try_from(xml.len()).expect("document fits c_int");
            let ok = (self.parse)(p, xml.as_ptr() as *const c_char, len, 1);
            assert_eq!(ok, 1, "expat rejected the document");
            (self.free)(p);
        }
        c
    }
}

// ---------------------------------------------------------------------------
// libxml2 SAX2
// ---------------------------------------------------------------------------

/// `xmlSAXHandler` (parser.h, libxml2 2.9 to 2.15). Slots we do not set stay `*mut c_void`.
#[repr(C)]
struct Sax {
    before_start_element: [*mut c_void; 14], // internalSubset .. endDocument
    start_element: *mut c_void,
    end_element: *mut c_void,
    reference: *mut c_void,
    characters: *mut c_void,
    ignorable: *mut c_void,
    pi: *mut c_void,
    comment: *mut c_void,
    warning: *mut c_void,
    error: *mut c_void,
    fatal: *mut c_void,
    get_param_entity: *mut c_void,
    cdata: *mut c_void,
    external_subset: *mut c_void,
    initialized: u32,
    private: *mut c_void,
    start_element_ns: *mut c_void,
    end_element_ns: *mut c_void,
    serror: *mut c_void,
}

struct LibXml2 {
    version: String,
    sax_version: unsafe extern "C" fn(*mut Sax, c_int) -> c_int,
    parse: unsafe extern "C" fn(*mut Sax, *mut c_void, *const c_char, c_int) -> c_int,
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn l_start(
    u: *mut c_void,
    local: *const u8,
    prefix: *const u8,
    uri: *const u8,
    nb_ns: c_int,
    _ns: *mut *const u8,
    nb_attr: c_int,
    _defaulted: c_int,
    attrs: *mut *const u8,
) {
    black_box((local, prefix, uri, nb_ns));
    let c = unsafe { &mut *(u as *mut Counts) };
    c.elements += 1;
    c.attributes += nb_attr as u64;
    // Each attribute is 5 pointers: localname, prefix, URI, value, value end.
    let mut a = attrs;
    for _ in 0..nb_attr {
        unsafe {
            black_box((*a, *a.add(1), *a.add(2), *a.add(3)));
            a = a.add(5);
        }
    }
}
unsafe extern "C" fn l_end(_: *mut c_void, local: *const u8, _: *const u8, _: *const u8) {
    black_box(local);
}
unsafe extern "C" fn l_chars(u: *mut c_void, s: *const u8, len: c_int) {
    black_box(s);
    unsafe { (*(u as *mut Counts)).text += len as u64 };
}

impl LibXml2 {
    fn load() -> Option<Self> {
        let h = open_lib(&[c"libxml2.so.16", c"libxml2.so.2", c"libxml2.so"])?;
        unsafe {
            let vp = dlsym(h, c"xmlParserVersion".as_ptr()) as *const *const c_char;
            let version = if vp.is_null() || (*vp).is_null() {
                "unknown".into()
            } else {
                CStr::from_ptr(*vp).to_string_lossy().into_owned()
            };
            let init: unsafe extern "C" fn() = sym(h, c"xmlInitParser")?;
            init();
            Some(Self {
                version,
                sax_version: sym(h, c"xmlSAXVersion")?,
                parse: sym(h, c"xmlSAXUserParseMemory")?,
            })
        }
    }

    fn run(&self, xml: &[u8]) -> Counts {
        let mut c = Counts::default();
        unsafe {
            let mut s: Sax = core::mem::zeroed();
            assert_eq!((self.sax_version)(&mut s, 2), 0);
            s.start_element_ns = l_start as *mut c_void;
            s.end_element_ns = l_end as *mut c_void;
            s.characters = l_chars as *mut c_void;
            s.ignorable = l_chars as *mut c_void;
            s.cdata = l_chars as *mut c_void;
            // The stock SAX2 handlers build a tree (and need a document); this bench builds none.
            s.before_start_element = [ptr::null_mut(); 14];
            s.reference = ptr::null_mut();
            s.comment = ptr::null_mut();
            s.pi = ptr::null_mut();
            let len = c_int::try_from(xml.len()).expect("document fits c_int");
            let rc = (self.parse)(
                &mut s,
                &mut c as *mut Counts as *mut c_void,
                xml.as_ptr() as *const c_char,
                len,
            );
            assert_eq!(rc, 0, "libxml2 rejected the document");
        }
        c
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn exi_run(g: &exi::grammar::Grammars<'_>, xml: &[u8]) -> Counts {
    let mut r: XmlReader = XmlReader::new(
        XmlConfig::new().report_comments(false).report_pis(false),
        g,
    );
    let mut c = Counts::default();
    let mut rest = xml;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        if exi::transcode(&mut r, &mut c).unwrap() == Status::End {
            return c;
        }
    }
}

struct Timing {
    warmup: Duration,
    window: Duration,
    samples: usize,
}

fn time_op(t: &Timing, mut f: impl FnMut()) -> (Stats, u32) {
    let start = Instant::now();
    let mut n = 0u32;
    while start.elapsed() < t.warmup || n < 2 {
        f();
        n += 1;
    }
    let per = start.elapsed() / n;
    let runs = ((t.window.as_nanos() / per.as_nanos().max(1)) as u32).max(1);
    let stats = Stats::of(
        (0..t.samples)
            .map(|_| {
                let s = Instant::now();
                for _ in 0..runs {
                    f();
                }
                s.elapsed() / runs
            })
            .collect(),
    );
    (stats, runs)
}

fn corpus(files: &[PathBuf], quick: bool) -> Vec<(String, Vec<u8>)> {
    if !files.is_empty() {
        return files
            .iter()
            .map(|p| {
                let n = p.file_name().unwrap().to_string_lossy().into_owned();
                (n, std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
            })
            .collect();
    }
    let mut d = Vec::new();
    if let Ok(b) = std::fs::read(common::package("fixtures/input/infer/logo.svg")) {
        d.push(("logo-1k".to_string(), b));
    }
    d.push(("orders-64k".into(), orders(64 << 10).into_bytes()));
    d.push(("svg-64k".into(), svg(64 << 10).into_bytes()));
    if !quick {
        d.push(("orders-1m".into(), orders(1 << 20).into_bytes()));
        d.push(("svg-1m".into(), svg(1 << 20).into_bytes()));
        d.push(("orders-16m".into(), orders(16 << 20).into_bytes()));
    }
    d
}

fn main() {
    let mut files = Vec::new();
    let mut quick = false;
    for a in std::env::args().skip(1) {
        if a == "--quick" {
            quick = true;
        } else if !a.starts_with('-') && PathBuf::from(&a).is_file() {
            files.push(PathBuf::from(a));
        }
    }
    let t = if quick {
        Timing { warmup: Duration::from_millis(40), window: Duration::from_millis(30), samples: 5 }
    } else {
        Timing { warmup: Duration::from_millis(150), window: Duration::from_millis(100), samples: 11 }
    };

    let expat = Expat::load();
    let xml2 = LibXml2::load();
    match &expat {
        Some(e) => println!("Loaded {}", e.version),
        None => println!("expat not found: skipping"),
    }
    match &xml2 {
        Some(x) => println!("Loaded libxml2 {}", x.version),
        None => println!("libxml2 not found: skipping"),
    }

    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let schemaless = GrammarSet::schemaless();
    let g = schemaless.attach(&strings).unwrap();

    let mut csv = String::from(
        "document,parser,input_octets,elements,attributes,runs_per_sample,samples,\
         median_ns,min_ns,max_ns,mb_s\n",
    );
    println!("{:12} {:10} {:>11} {:>10} {:>7}", "Document", "Parser", "Median", "Speed", "Spread");
    for (name, xml) in &corpus(&files, quick) {
        type Run<'a> = Box<dyn Fn() -> Counts + 'a>;
        let mut parsers: Vec<(&str, Run)> = vec![("libexi", Box::new(|| exi_run(&g, xml)))];
        if let Some(e) = &expat {
            parsers.push(("expat", Box::new(move || e.run(xml))));
        }
        if let Some(x) = &xml2 {
            parsers.push(("libxml2", Box::new(move || x.run(xml))));
        }
        let want = parsers[0].1();
        for (pname, run) in &parsers {
            let got = run();
            assert_eq!(got, want, "{name}: {pname} counted differently from libexi");
            let (st, runs) = time_op(&t, || {
                black_box(run());
            });
            let mbs = xml.len() as f64 / st.median.as_secs_f64() / 1e6;
            println!(
                "{name:12} {pname:10} {:>8.3} ms {:>6.0} MB/s {:>6.1}%",
                st.median.as_secs_f64() * 1e3,
                mbs,
                100.0 * st.spread()
            );
            let _ = writeln!(
                csv,
                "{name},{pname},{},{},{},{runs},{},{},{},{},{mbs:.2}",
                xml.len(),
                want.elements,
                want.attributes,
                st.samples,
                st.median.as_nanos(),
                st.min.as_nanos(),
                st.max.as_nanos()
            );
        }
    }
    let path = common::out_dir().join("sax.csv");
    std::fs::write(&path, csv).unwrap();
    println!("\nwrote {}", path.display());
    common::write_meta("sax");
    let meta = common::out_dir().join("sax.meta");
    let mut m = std::fs::read_to_string(&meta).unwrap_or_default();
    if let Some(e) = &expat {
        let _ = writeln!(m, "expat_version: {}", e.version);
    }
    if let Some(x) = &xml2 {
        let _ = writeln!(m, "libxml2_version: {}", x.version);
    }
    std::fs::write(&meta, m).unwrap();
}
