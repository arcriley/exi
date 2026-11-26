// tests/w3c.rs
//
// W3C EXI test suite interoperability tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The interoperability cases of the W3C EXI test suite
//! (github.com/w3c/exi-testsuite), against the encodings EXIficient
//! made of them in 2013. `fixtures/fetch-w3c.sh` puts
//! both in `fixtures/w3c`; without them the test is skipped.
//!
//! Each encoding names its options in its file name (the suite's
//! `TestCaseParameters.getFrameworkTestParams`):
//! `<input>.exi%<preserve>%<flags>%<valuePartitionCapacity>%
//! <valueMaxLength>%<blockSize>%<profile>%<schema>`. For each one the
//! test
//!
//! 1. decodes EXIficient's encoding,
//! 2. encodes the input with the same options and decodes that,
//! 3. expects the same events from both, and
//! 4. counts the encodings that are the same octets (compressed bodies
//!    compared inflated).
//!
//! Cases the crate does not support (EXI profile parameters, datatype
//! representation maps) are counted as skipped. Every other case must
//! pass, except those listed in `tests/w3c-known.tsv` with the reason.
#![cfg(feature = "xml")]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{
    Coding, Event, EventSink, Header, Options, OwnedEvent, Preserve, Reader,
    ReaderConfig, Result, Step, Writer, WriterConfig,
};

const ROOT: &str = "fixtures/w3c";
const ENCODINGS: &str =
    "fixtures/w3c/encodings/EXIficient/Exificient_2013-12-05_11_00_25";

/// One EXIficient encoding and the options its name gives.
struct Case {
    /// The suite's test case name.
    test: String,
    /// The encoding's path under [`ENCODINGS`].
    encoded: String,
    input: PathBuf,
    preserve: String,
    flags: String,
    vpc: Option<u32>,
    vml: Option<u32>,
    block: Option<u32>,
    profile: String,
    schema: Option<String>,
    dtr: Vec<String>,
}

impl Case {
    fn has(&self, f: char) -> bool {
        self.flags.contains(f)
    }

    fn options(&self) -> Options {
        let coding = if self.has('c') {
            Coding::Compression
        } else if self.has('p') {
            Coding::PreCompression
        } else if self.has('a') {
            Coding::ByteAligned
        } else {
            Coding::BitPacked
        };
        let mut p = Preserve::empty();
        for (c, flag) in [
            ('c', Preserve::COMMENTS),
            ('d', Preserve::DTD),
            ('l', Preserve::LEXICAL_VALUES),
            ('p', Preserve::PIS),
            ('x', Preserve::PREFIXES),
        ] {
            if self.preserve.contains(c) {
                p = p | flag;
            }
        }
        let mut b = Options::builder()
            .coding(coding)
            .strict(self.has('v'))
            .fragment(self.has('f'))
            .self_contained(self.has('r'))
            .preserve(p)
            .value_max_length(self.vml)
            .value_partition_capacity(self.vpc);
        if let Some(n) = self.block {
            b = b.block_size(n);
        }
        b.build().unwrap()
    }

    fn header(&self) -> Header {
        let cookie = self.has('m');
        if self.has('o') || self.has('s') {
            Header::WithOptions { cookie }
        } else {
            Header::Bare { cookie }
        }
    }
}

fn number(s: &str) -> Option<u32> {
    (!s.is_empty()).then(|| s.parse().unwrap())
}

/// The input an encoding was made from: the same name with `.xml`, or
/// the file itself (fragments and schemas used as documents).
fn input_of(base: &str) -> PathBuf {
    let xml = Path::new(ROOT).join(format!("{base}.xml"));
    if xml.exists() { xml } else { Path::new(ROOT).join(base) }
}

fn cases() -> Vec<Case> {
    let manifest =
        std::fs::read_to_string(Path::new(ENCODINGS).join("MANIFEST"))
            .unwrap();
    manifest
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (encoded, test) = l.split_once('\t').unwrap();
            let f: Vec<&str> = encoded.split('%').collect();
            let base = f[0].strip_suffix(".exi").unwrap();
            Case {
                test: test.trim().into(),
                encoded: encoded.into(),
                input: input_of(base),
                preserve: f[1].into(),
                flags: f[2].into(),
                vpc: number(f[3]),
                vml: number(f[4]),
                block: number(f[5]),
                profile: f[6].into(),
                schema: (!f[7].is_empty()).then(|| f[7].replace('@', "/")),
                dtr: f[8..].iter().map(|s| s.to_string()).collect(),
            }
        })
        .collect()
}

/// `{uri}local` names that are self-contained, by test case, from the
/// suite's configuration.
fn self_contained_names() -> HashMap<String, Vec<(String, String)>> {
    let mut out = HashMap::new();
    let mut stack = vec![Path::new(ROOT).join("config/testCases-interop")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for case in text.split("<testCase name=\"").skip(1) {
                let name = &case[..case.find('"').unwrap()];
                let body =
                    &case[..case.find("</testCase>").unwrap_or(case.len())];
                let key = "selfContainedQNames\" value=\"";
                if let Some(i) = body.find(key) {
                    let v = &body[i + key.len()..];
                    let v = &v[..v.find('"').unwrap()];
                    let names = v
                        .split_whitespace()
                        .map(|q| {
                            let (u, l) = q
                                .trim_start_matches('{')
                                .split_once('}')
                                .unwrap();
                            (u.to_string(), l.to_string())
                        })
                        .collect();
                    out.insert(name.to_string(), names);
                }
            }
        }
    }
    out
}

/// Compiles `main` (a path under [`ROOT`]) with the documents it
/// includes and imports by `schemaLocation`, as a validating parser
/// would load them.
#[cfg(feature = "xsd")]
fn compile(main: &str) -> Result<GrammarSet> {
    let mut docs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut todo = vec![main.to_string()];
    while let Some(loc) = todo.pop() {
        if docs.iter().any(|d| d.0 == loc) {
            continue;
        }
        let Ok(bytes) = std::fs::read(Path::new(ROOT).join(&loc)) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        for part in text.split("schemaLocation=").skip(1) {
            let q = part.as_bytes()[0] as char;
            let Some(rel) = part[1..].split(q).next() else { continue };
            if rel.contains("://") {
                continue;
            }
            let dir = Path::new(&loc).parent().unwrap_or_else(|| Path::new(""));
            let joined = dir.join(rel).to_str().unwrap().replace('\\', "/");
            let mut p = Vec::new();
            for c in joined.split('/') {
                match c {
                    ".." => {
                        p.pop();
                    }
                    "." | "" => {}
                    c => p.push(c),
                }
            }
            todo.push(p.join("/"));
        }
        docs.push((loc, bytes));
    }
    let mut c = exi::xsd::XsdCompiler::new();
    for (name, bytes) in &docs {
        c = c.document(name, bytes);
    }
    c.compile(main)
}

/// Passes events on: drops the wrapper element of a fragment, and marks
/// the self-contained elements.
struct Shape<'s, S: EventSink> {
    inner: &'s mut S,
    fragment: bool,
    sc: &'s [(String, String)],
    depth: u32,
}

impl<S: EventSink> EventSink for Shape<'_, S> {
    fn event(&mut self, ev: &Event<'_>) -> Result<()> {
        match ev {
            Event::StartElement(_) => self.depth += 1,
            Event::EndElement(_) => self.depth -= 1,
            _ => {}
        }
        let wrapper = self.fragment
            && match ev {
                Event::StartElement(_)
                | Event::Namespace(_)
                | Event::Attribute(..)
                | Event::Characters(_) => self.depth == 1,
                Event::EndElement(_) => self.depth == 0,
                _ => false,
            };
        if wrapper {
            return Ok(());
        }
        self.inner.event(ev)?;
        if let Event::StartElement(n) = ev {
            if self.sc.iter().any(|(u, l)| n.is(u, l)) {
                self.inner.event(&Event::SelfContained)?;
            }
        }
        Ok(())
    }
}

/// The input as the encoder takes it: a fragment wrapped in one
/// element, as the suite's `FragmentsInputStream` does.
fn input_text(c: &Case) -> Vec<u8> {
    let text = std::fs::read(&c.input).unwrap();
    if c.has('f') {
        let mut t = b"<root>".to_vec();
        // The XML declaration, if any, cannot follow the wrapper's
        // start tag.
        let body = match text.windows(2).position(|w| w == b"?>") {
            Some(i) if text.starts_with(b"<?xml") => &text[i + 2..],
            _ => &text[..],
        };
        t.extend_from_slice(body);
        t.extend_from_slice(b"</root>");
        t
    } else {
        text
    }
}

fn encode(
    g: &Grammars<'_>,
    c: &Case,
    sc: &[(String, String)],
) -> Result<Vec<u8>> {
    let opts = c.options();
    let xml = input_text(c);
    let cfg = XmlConfig::new()
        .report_comments(opts.preserve().contains(Preserve::COMMENTS))
        .report_pis(opts.preserve().contains(Preserve::PIS))
        .report_doctype(opts.preserve().contains(Preserve::DTD));
    let mut x: XmlReader = XmlReader::new(cfg, g);
    let wcfg = WriterConfig::new(opts.clone(), g)
        .header(c.header())
        .utc_time(c.has('u'));
    let mut w = Writer::new(Vec::new(), wcfg)?;
    let mut shape =
        Shape { inner: &mut w, fragment: c.has('f'), sc, depth: 0 };
    let mut rest = &xml[..];
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        if exi::transcode(&mut x, &mut shape)? == exi::Status::End {
            break;
        }
    }
    Ok(w.into_sink())
}

fn decode(g: &Grammars<'_>, c: &Case, exi: &[u8]) -> Result<Vec<OwnedEvent>> {
    let mut cfg = ReaderConfig::new(g);
    if matches!(c.header(), Header::Bare { .. }) {
        cfg = cfg.options(c.options());
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

/// The events as the suite compares them:
///
/// - its 2013 framework dropped text that is only whitespace;
/// - an empty value (string, binary or list) is no value: EXIficient
///   writes an empty CH where this crate writes none;
/// - the suite's inputs have changed line ends since 2013 (EXIficient
///   saw `\n\n` where the files now give `\n`), so runs of newlines in
///   text count as one;
/// - a float is its value (EXIficient keeps `4.000` as 4000E-3);
/// - text that a dropped comment or PI split is one text (EXIficient's
///   SAX parser joined it);
/// - a DOCTYPE is its name and ids: EXIficient left the internal subset
///   out.
fn normalize(evs: Vec<OwnedEvent>) -> Vec<OwnedEvent> {
    // Join adjacent string text first.
    let mut joined: Vec<OwnedEvent> = Vec::with_capacity(evs.len());
    for e in evs {
        if let (
            Some(OwnedEvent::Characters(exi::OwnedValue::Str(a))),
            OwnedEvent::Characters(exi::OwnedValue::Str(b)),
        ) = (joined.last_mut(), &e)
        {
            let mut s = a.to_string();
            s.push_str(b);
            *a = s.into();
            continue;
        }
        joined.push(e);
    }
    let evs = joined;
    use exi::OwnedValue;
    let float = |v: OwnedValue| match v {
        OwnedValue::Float(mut f) if f.mantissa != 0 => {
            while f.mantissa % 10 == 0 {
                f.mantissa /= 10;
                f.exponent += 1;
            }
            OwnedValue::Float(f)
        }
        v => v,
    };
    let newlines = |v: OwnedValue| match v {
        OwnedValue::Str(s) if s.contains("\n\n") => {
            let mut t = String::with_capacity(s.len());
            for c in s.chars() {
                if !(c == '\n' && t.ends_with('\n')) {
                    t.push(c);
                }
            }
            OwnedValue::Str(t.into())
        }
        v => v,
    };
    evs.into_iter()
        .filter(|e| match e {
            OwnedEvent::Characters(OwnedValue::Str(s)) => {
                !s.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
            }
            OwnedEvent::Characters(OwnedValue::Binary(b, _)) => !b.is_empty(),
            OwnedEvent::Characters(OwnedValue::List(l)) => !l.is_empty(),
            _ => true,
        })
        .map(|e| match e {
            OwnedEvent::Characters(v) => {
                OwnedEvent::Characters(newlines(float(v)))
            }
            OwnedEvent::Attribute(n, v) => OwnedEvent::Attribute(n, float(v)),
            OwnedEvent::Doctype { name, public_id, system_id, .. } => {
                OwnedEvent::Doctype {
                    name,
                    public_id,
                    system_id,
                    text: Default::default(),
                }
            }
            e => e,
        })
        .collect()
}

/// The raw DEFLATE streams of a compressed body, each inflated.
fn inflated(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let mut z = flate2::Decompress::new(false);
        let mut v = Vec::with_capacity(1 << 16);
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

/// Whether two encodings are the same: the same octets, or for
/// compression the same header and the same inflated streams.
fn same_octets(c: &Case, ours: &[u8], theirs: &[u8]) -> bool {
    if ours == theirs {
        return true;
    }
    if !c.has('c') {
        return false;
    }
    let limits = exi::Limits::new();
    let hlen = |b: &[u8]| {
        exi::header::parse_header(b, &limits)
            .ok()
            .flatten()
            .map(|h| (h.body_offset_bits / 8) as usize)
    };
    match (hlen(ours), hlen(theirs)) {
        (Some(a), Some(b)) if ours[..a] == theirs[..b] => {
            let (x, y) = (inflated(&ours[a..]), inflated(&theirs[b..]));
            x.is_some() && x == y
        }
        _ => false,
    }
}

/// `tests/w3c-known.tsv`: encodings expected to fail, with the reason.
fn known() -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string("tests/w3c-known.tsv") else {
        return BTreeMap::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut f = l.splitn(3, '\t');
            let k = f.next().unwrap();
            let why = f.nth(1).unwrap_or("");
            (k.to_string(), why.to_string())
        })
        .collect()
}

#[test]
fn w3c_interop() {
    if !Path::new(ENCODINGS).join("MANIFEST").exists() {
        assert!(
            std::env::var_os("EXI_REQUIRE_W3C").is_none(),
            "no {ENCODINGS}: run fixtures/fetch-w3c.sh"
        );
        eprintln!("no {ENCODINGS}: run fixtures/fetch-w3c.sh; skipped");
        return;
    }
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let schemaless = GrammarSet::schemaless();
    let sc = self_contained_names();
    #[cfg_attr(not(feature = "xsd"), allow(unused_mut, unused_variables))]
    let mut sets: HashMap<
        String,
        std::result::Result<GrammarSet, String>,
    > = HashMap::new();
    let known = known();

    let mut skipped: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut passed, mut identical, mut expected_failures) = (0, 0, 0);
    let mut unexpected = Vec::new();
    let all = cases();
    for c in &all {
        let skip = if !c.profile.is_empty() {
            Some("EXI profile parameters")
        } else if !c.dtr.is_empty() {
            Some("datatype representation map")
        } else if cfg!(not(feature = "xsd"))
            && (c.schema.is_some() || c.has('i'))
        {
            Some("schema cases need the feature xsd")
        } else if cfg!(not(feature = "compression"))
            && (c.has('c') || c.has('p'))
        {
            Some("compression needs the feature compression")
        } else {
            None
        };
        if let Some(why) = skip {
            *skipped.entry(why).or_default() += 1;
            continue;
        }
        let set: std::result::Result<&GrammarSet, String> = match &c.schema {
            // Schema-informed without a schema: XML Schema's built-in types only.
            #[cfg(feature = "xsd")]
            None if c.has('i') => sets
                .entry(String::new())
                .or_insert_with(|| {
                    GrammarSet::from_xsd(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"/>"#)
                        .map_err(|e| e.to_string())
                })
                .as_ref()
                .map_err(|e| format!("schema: {e}")),
            None => Ok(&schemaless),
            #[cfg(feature = "xsd")]
            Some(s) => sets
                .entry(s.clone())
                .or_insert_with(|| compile(s).map_err(|e| e.to_string()))
                .as_ref()
                .map_err(|e| format!("schema: {e}")),
            #[cfg(not(feature = "xsd"))]
            Some(_) => unreachable!(),
        };
        let outcome = (|| -> std::result::Result<bool, String> {
            let set = set?;
            let g =
                set.attach(&strings).map_err(|e| format!("attach: {e}"))?;
            let theirs =
                std::fs::read(Path::new(ENCODINGS).join(&c.encoded)).unwrap();
            let names = sc.get(&c.test).map(Vec::as_slice).unwrap_or(&[]);
            let their_events = normalize(
                decode(&g, c, &theirs)
                    .map_err(|e| format!("decode EXIficient: {e}"))?,
            );
            let ours =
                encode(&g, c, names).map_err(|e| format!("encode: {e}"))?;
            let our_events = normalize(
                decode(&g, c, &ours)
                    .map_err(|e| format!("decode ours: {e}"))?,
            );
            if our_events != their_events {
                let i = our_events
                    .iter()
                    .zip(&their_events)
                    .position(|(a, b)| a != b)
                    .unwrap_or_else(|| our_events.len().min(their_events.len()));
                return Err(format!(
                    "events differ at {i}: ours {:?}, EXIficient {:?}",
                    our_events.get(i),
                    their_events.get(i)
                ));
            }
            Ok(same_octets(c, &ours, &theirs))
        })();
        match outcome {
            Ok(same) => {
                passed += 1;
                identical += same as usize;
                if known.contains_key(&c.encoded) {
                    unexpected.push(format!(
                        "{} passes: remove it from tests/w3c-known.tsv",
                        c.encoded
                    ));
                }
            }
            Err(why) => {
                let kind = why
                    .split(" at ")
                    .next()
                    .unwrap()
                    .split(':')
                    .next()
                    .unwrap()
                    .to_string();
                failed
                    .entry(kind)
                    .or_default()
                    .push(format!("{} ({}): {why}", c.encoded, c.test));
                if known.contains_key(&c.encoded) {
                    expected_failures += 1;
                } else {
                    unexpected
                        .push(format!("{} ({}): {why}", c.encoded, c.test));
                }
            }
        }
    }
    eprintln!(
        "W3C interop: {} encodings, {passed} pass ({identical} the same octets as EXIficient), {} fail ({expected_failures} known), {} skipped",
        all.len(),
        failed.values().map(Vec::len).sum::<usize>(),
        skipped.values().sum::<usize>()
    );
    for (why, n) in &skipped {
        eprintln!("  skipped {n}: {why}");
    }
    for (kind, list) in &failed {
        eprintln!("  {kind}: {}", list.len());
        for l in list.iter().take(5) {
            eprintln!("    {l}");
        }
    }
    assert!(
        unexpected.is_empty(),
        "{} unexpected:\n{}",
        unexpected.len(),
        unexpected.join("\n")
    );
}
