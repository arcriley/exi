// tests/dom_parse.rs
//
// The direct XML parser against the streaming one
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! `Document::parse_xml` reads most documents with a direct parser and
//! leaves the rest to `XmlReader`. These tests hold the direct parser to
//! that: for every input it either declines, or builds exactly the tree
//! `XmlReader` and `DocumentBuilder` build, down to the C structs; and it
//! never accepts what `XmlReader` rejects.
#![cfg(all(feature = "dom", feature = "xml"))]

use std::fmt::Write as _;

use exi::dom::raw::*;
use exi::dom::{Document, DocumentBuilder};
use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{Step, Result};

/// The tree `XmlReader` and `DocumentBuilder` make, as `parse_xml` did
/// before the direct parser.
fn streamed<'g>(s: &'g GlobalStrings, xml: &[u8], blanks: bool) -> Result<Document<'g>> {
    let set = GrammarSet::schemaless();
    let g = set.attach(s)?;
    let mut b = DocumentBuilder::new(s).ignore_blanks(blanks);
    let mut x: XmlReader = XmlReader::new(
        XmlConfig::new().report_comments(true).report_pis(true).report_doctype(true),
        &g,
    );
    let mut rest = xml;
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        loop {
            match x.next()? {
                Step::Event(e, meta) => b.event_at(&e, Some(&meta))?,
                Step::NeedInput => break,
                Step::End => return b.finish(),
            }
        }
    }
}

/// SAFETY for the helpers below: every pointer is a live node, string
/// or declaration of a document borrowed for the call.
unsafe fn cstr<'a>(p: *const u8) -> &'a str {
    if p.is_null() {
        return "<null>";
    }
    unsafe { std::ffi::CStr::from_ptr(p.cast()).to_str().unwrap() }
}

/// Which declaration `ns` is, seen from `el`: the ancestor that declares
/// it (0 for `el`) and its place in that list; `oldNs` for the
/// document's `xml` declaration.
unsafe fn which_ns(doc: &Document<'_>, el: *const RawNode, ns: *const RawNs) -> String {
    if ns.is_null() {
        return "none".into();
    }
    unsafe {
        if std::ptr::eq(ns, (*doc.as_raw()).oldNs) {
            return "oldNs".into();
        }
        let mut e = el;
        let mut up = 0;
        while !e.is_null() && (*e).r#type == EXI_ELEMENT_NODE {
            let mut d = (*e).nsDef as *const RawNs;
            let mut k = 0;
            while !d.is_null() {
                if d == ns {
                    return format!("up{up}[{k}]");
                }
                d = (*d).next;
                k += 1;
            }
            e = (*e).parent;
            up += 1;
        }
        "out of scope".into()
    }
}

/// Every field of the tree that the C API or the Rust API shows.
fn dump(doc: &Document<'_>) -> String {
    let mut out = String::new();
    unsafe {
        let d = doc.as_raw();
        let _ = writeln!(out, "doc oldNs={}", if (*d).oldNs.is_null() { "-" } else { cstr((*(*d).oldNs).href) });
        let mut n = (*d).children as *const RawNode;
        while !n.is_null() {
            node(doc, n, 0, &mut out);
            n = (*n).next;
        }
    }
    out
}

unsafe fn node(doc: &Document<'_>, n: *const RawNode, depth: usize, out: &mut String) {
    unsafe {
        let pad = "  ".repeat(depth);
        let id = doc.node_id(n.cast()).expect("every node has an id");
        let t = (*n).r#type;
        let _ = write!(out, "{pad}t{t} name={:?} line={}", cstr((*n).name), (*n).line);
        if t == EXI_ELEMENT_NODE {
            let nm = doc.name(id).unwrap();
            let _ = write!(
                out,
                " {{{}}}{} prefix={:?} ids={:?} ns={}",
                nm.uri(),
                nm.local_name(),
                nm.prefix(),
                doc.global_name(id).map(|g| (g.uri.index(), g.local.index())),
                which_ns(doc, n, (*n).ns)
            );
            assert_eq!(cstr((*n).name), nm.local_name());
            let mut d = (*n).nsDef as *const RawNs;
            while !d.is_null() {
                let _ = write!(out, " xmlns[{:?}={:?} id={}]", cstr((*d).prefix), cstr((*d).href), (*d).uriId);
                d = (*d).next;
            }
            let _ = writeln!(out);
            let mut a = (*n).properties as *const RawAttr;
            while !a.is_null() {
                let aid = doc.node_id(a.cast()).unwrap();
                let nm = doc.name(aid).unwrap();
                let _ = write!(
                    out,
                    "{pad}  @{{{}}}{} prefix={:?} ids={:?} ns={} parent_ok={}",
                    nm.uri(),
                    nm.local_name(),
                    nm.prefix(),
                    doc.global_name(aid).map(|g| (g.uri.index(), g.local.index())),
                    which_ns(doc, n, (*a).ns),
                    std::ptr::eq((*a).parent, n)
                );
                let mut c = (*a).children as *const RawNode;
                while !c.is_null() {
                    let _ = write!(out, " [t{} {:?}]", (*c).r#type, cstr((*c).content));
                    assert!(doc.node_id(c.cast()).is_some());
                    c = (*c).next;
                }
                let _ = writeln!(out);
                a = (*a).next;
            }
            let mut c = (*n).children as *const RawNode;
            let mut prev: *const RawNode = std::ptr::null();
            while !c.is_null() {
                assert_eq!((*c).parent as *const RawNode, n);
                assert_eq!((*c).prev as *const RawNode, prev);
                node(doc, c, depth + 1, out);
                prev = c;
                c = (*c).next;
            }
            assert_eq!((*n).last as *const RawNode, prev);
        } else {
            let _ = writeln!(out, " content={:?} value={:?}", cstr((*n).content), doc.content(id));
        }
    }
}

/// Live strings in the pool, to see that dropping a tree releases what it
/// held.
fn live(s: &GlobalStrings) -> (u32, u32, u32) {
    (s.uris().stats().strings, s.locals().stats().strings, s.prefixes().stats().strings)
}

/// The direct parser either declines `xml` or matches the streaming
/// parser exactly. Returns whether it took the document.
fn check(xml: &[u8], blanks: bool) -> bool {
    let s = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let before = live(&s);
    let slow = streamed(&s, xml, blanks);
    let fast = Document::parse_xml_direct(&s, xml, blanks);
    let took = fast.is_some();
    match (&slow, &fast) {
        (Ok(a), Some(b)) => assert_eq!(dump(b), dump(a), "input: {:?}", String::from_utf8_lossy(xml)),
        (Err(e), Some(_)) => panic!("direct parser took what XmlReader refuses ({e}): {:?}", String::from_utf8_lossy(xml)),
        _ => {}
    }
    drop(slow);
    drop(fast);
    assert_eq!(live(&s), before, "strings left behind: {:?}", String::from_utf8_lossy(xml));
    took
}

/// Documents the direct parser must take (and match).
const TAKEN: &[&str] = &[
    "<a/>",
    "<a></a>",
    "<a>text</a>",
    "<?xml version=\"1.0\"?><a/>",
    "<?xml version='1.0' encoding='utf-8' standalone='yes'?>\n<a/>\n",
    "\u{feff}<a/>",
    "<!-- before --><?pi data?><a/><!-- after --><?end?>",
    "<a b=\"1\" c='2'>x<b/>y<c d=\"&amp;&lt;&gt;&quot;&apos;\"/>z</a>",
    "<a>&amp;&lt;&#65;&#x42;&#x1F600;<![CDATA[<raw> & ]]>tail</a>",
    "<a>one<!--c-->two<?p x?>three</a>",
    "<a>\n  <b>\n    <c/>\n  </b>\n</a>",
    "<a\n  b='1'\n  c='2'\n>\n<d\n/></a>",
    "<a b='\ttab\nnl'/>",
    "<a b='x&#10;y&#9;z'/>",
    "<a>x\r\ny\rz</a>",
    "<a\r\nb='1'\r\n/>",
    "<a>caf\u{e9} \u{65e5}\u{672c} \u{1F600}</a>",
    "<a xmlns='urn:a'><b/><c xmlns=''><d/></c></a>",
    "<p:a xmlns:p='urn:1'><p:b xmlns:p='urn:2' p:x='1'/><p:c/></p:a>",
    "<a xmlns='urn:d' xmlns:x='urn:x' x:k='v' k='w'><x:b><c/></x:b></a>",
    "<a xmlns:x='urn:x'><b x:k='1' xmlns:y='urn:y' y:k='2'/></a>",
    "<a xml:lang='en' xml:space='preserve'><b xml:lang='de'/></a>",
    "<a xmlns='urn:a' xmlns:a='urn:a'><a:b/><b/></a>",
    "<a>]</a>",
    "<a>]]</a>",
    "<a>a]b]]c</a>",
    "<a>></a>",
    "<a b='>'/>",
    "<a>   </a>",
    "<a> <b/> </a>",
    "<a></a >",
    "<a b = '1' />",
    "<_a-b.c d.e='1'/>",
    "<a:b xmlns:a='u'/>",
];

/// Documents the direct parser must leave to `XmlReader`.
const LEFT: &[&str] = &[
    "<!DOCTYPE a><a/>",
    "<!DOCTYPE a [<!ENTITY e 'x'>]><a>&e;</a>",
    "<a>&nbsp;</a>",
    "<?xml version=\"1.1\"?><a/>",
    "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>",
    "<\u{e9}/>",
    "<a \u{e9}='1'/>",
    "<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>",
    "<xml:a/>",
    // Errors.
    "",
    "<a>",
    "<a></b>",
    "<a/><b/>",
    "text<a/>",
    "<a/>text",
    "<a>]]></a>",
    "<a b='1' b='2'/>",
    "<a x:b='1' y:b='2' xmlns:x='u' xmlns:y='u'/>",
    "<p:a/>",
    "<a xmlns:p=''/>",
    "<a b='<'/>",
    "<a b=1/>",
    "<a b='1'c='2'/>",
    "<a/ >",
    "<a><!-- -- --></a>",
    "<a><!-- x ---></a>",
    "<a><?xml x?></a>",
    "<a>&#0;</a>",
    "<a>&#xD800;</a>",
    "<a>&#xFFFE;</a>",
    "<a>&#;</a>",
    "<a>&amp</a>",
    "<a>\u{1}</a>",
    "<a>\u{fffe}</a>",
    "<a b='\u{1}'/>",
    "<1a/>",
    "<a::b/>",
    "<a:/>",
    "<:a/>",
    "<a></a",
    "<a><![CDATA[x</a>",
    "<![CDATA[x]]><a/>",
    "<a b='1'",
    "<?xml version='1.0'?>",
];

#[test]
fn takes_and_matches() {
    for xml in TAKEN {
        for blanks in [false, true] {
            assert!(check(xml.as_bytes(), blanks), "should be taken: {xml:?}");
        }
    }
}

#[test]
fn leaves_the_rest_to_xmlreader() {
    for xml in LEFT {
        assert!(!check(xml.as_bytes(), false), "should be left to XmlReader: {xml:?}");
    }
}

#[test]
fn errors_read_the_same() {
    // parse_xml gives XmlReader's error for anything the direct parser
    // declines.
    let s = GlobalStrings::new(GlobalConfig::new()).unwrap();
    for xml in LEFT {
        let a = Document::parse_xml(&s, xml.as_bytes()).err().map(|e| e.to_string());
        let b = streamed(&s, xml.as_bytes(), false).err().map(|e| e.to_string());
        assert_eq!(a, b, "{xml:?}");
    }
}

#[test]
fn fixtures_match() {
    let mut taken = 0;
    let mut n = 0;
    for dir in ["fixtures/input", "fixtures/input/infer", "fixtures/input/html"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries {
            let p = e.unwrap().path();
            if !p.extension().is_some_and(|x| x == "xml" || x == "svg" || x == "xsd") {
                continue;
            }
            let xml = std::fs::read(&p).unwrap();
            for blanks in [false, true] {
                taken += check(&xml, blanks) as usize;
                n += 1;
            }
        }
    }
    assert!(n > 40, "{n} documents");
    // Nearly every fixture is plain enough for the direct parser.
    assert!(taken * 10 >= n * 8, "{taken} of {n} taken");
}

/// A small deterministic generator for the mutations below.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

/// Valid documents with bytes changed, cut or repeated: whatever the
/// direct parser takes must be what `XmlReader` takes.
#[test]
fn mutations_never_diverge() {
    let seeds: Vec<Vec<u8>> = TAKEN
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .chain(std::fs::read("fixtures/input/complex.xml").ok())
        .chain(std::fs::read("exi-logo.svg").ok())
        .collect();
    let alphabet = b"<>/=\"'&;:!?-[]# \t\nabxmlns#x0123CDATA\x01\xc3\xa9\xef\xbf\xbe";
    let mut r = Lcg(7);
    let mut taken = 0;
    for round in 0..6000 {
        let mut d = seeds[round % seeds.len()].clone();
        for _ in 0..1 + r.next(3) {
            if d.is_empty() {
                break;
            }
            let at = r.next(d.len());
            match r.next(4) {
                0 => d[at] = alphabet[r.next(alphabet.len())],
                1 => {
                    d.remove(at);
                }
                2 => d.insert(at, alphabet[r.next(alphabet.len())]),
                _ => {
                    let len = 1 + r.next(8.min(d.len() - at));
                    let piece = d[at..at + len].to_vec();
                    d.splice(at..at, piece);
                }
            }
        }
        taken += check(&d, round % 2 == 1) as usize;
    }
    assert!(taken > 500, "only {taken} mutants taken");
}
