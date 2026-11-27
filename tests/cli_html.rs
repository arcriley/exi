// tests/cli_html.rs
//
// HTML command-line tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The `exi` tool's HTML commands, run as a user would.
#![cfg(all(feature = "xhtml5", feature = "html5ever"))]

use std::io::Write;
use std::process::{Command, Output, Stdio};

const EXI: &str = env!("CARGO_BIN_EXE_exi");
const PAGE: &str = "fixtures/input/html/page.html";

fn run(args: &[&str], stdin: &[u8]) -> Output {
    let mut c = Command::new(EXI)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A command that fails may exit before reading its input.
    let _ = c.stdin.take().unwrap().write_all(stdin);
    c.wait_with_output().unwrap()
}

fn ok(args: &[&str], stdin: &[u8]) -> Vec<u8> {
    let o = run(args, stdin);
    assert!(
        o.status.success(),
        "exi {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    o.stdout
}

fn err(args: &[&str], stdin: &[u8]) -> String {
    let o = run(args, stdin);
    assert!(!o.status.success(), "exi {args:?} succeeded");
    String::from_utf8(o.stderr).unwrap()
}

/// Encodes and decodes `html` with `flags`; then does it again with the result, which
/// must not change (the library's tests compare the first pass with html5ever).
fn round_trip(html: &[u8], flags: &[&str]) -> String {
    let pass = |h: &[u8]| {
        ok(
            &[&["decode", "--html"], flags].concat(),
            &ok(&[&["encode", "--html"], flags].concat(), h),
        )
    };
    let once = pass(html);
    assert_eq!(
        String::from_utf8(pass(&once)).unwrap(),
        String::from_utf8(once.clone()).unwrap(),
        "{flags:?}"
    );
    String::from_utf8(once).unwrap()
}

#[test]
fn html_round_trips_both_ways() {
    let page = std::fs::read(PAGE).unwrap();
    let plain = round_trip(&page, &["-p", "comments,dtd"]);
    assert!(
        plain.starts_with("<!DOCTYPE html><html lang=\"en\">"),
        "{plain}"
    );
    assert!(
        plain.contains("<!-- a comment - - with dashes -->"),
        "{plain}"
    );
    assert!(
        plain.contains(r#"<link rel="stylesheet" href="site.css">"#),
        "{plain}"
    );
    assert!(
        plain.contains(r#"<template id="row"><tr><td>cell</td></tr></template>"#),
        "{plain}"
    );
    // XHTML5 keeps no comments and sorts attributes by name.
    let x5 = round_trip(&page, &["--xhtml5"]);
    assert!(!x5.contains("<!--"), "{x5}");
    assert!(
        x5.contains(r#"<link href="site.css" rel="stylesheet">"#),
        "{x5}"
    );
    // The header names the profile, so decoding needs only --html.
    let exi = ok(&["encode", "--html", "--xhtml5"], &page);
    assert_eq!(
        String::from_utf8(ok(&["decode", "--html"], &exi)).unwrap(),
        x5
    );
}

#[test]
fn xhtml5_streams_name_the_profile() {
    let page = std::fs::read(PAGE).unwrap();
    let exi = ok(&["encode", "--html", "--xhtml5"], &page);
    let info = String::from_utf8(ok(&["info"], &exi)).unwrap();
    assert!(info.contains("urn:libexi:xhtml5:1"), "{info}");
    // Decoded as XML, it is XHTML.
    let xml = String::from_utf8(ok(&["decode"], &exi)).unwrap();
    assert!(
        xml.contains(r#"<html xmlns="http://www.w3.org/1999/xhtml""#),
        "{xml}"
    );
    // A bare header needs --xhtml5 on both sides.
    let bare = ok(&["encode", "--html", "--xhtml5", "--bare"], &page);
    assert!(bare.len() < exi.len());
    assert_eq!(
        ok(&["decode", "--html", "--xhtml5"], &bare),
        ok(&["decode", "--html"], &exi)
    );
    assert!(!err(&["decode", "--html"], &bare).is_empty());
}

#[test]
fn xhtml_writes_the_parsed_page() {
    let page = std::fs::read(PAGE).unwrap();
    let x = String::from_utf8(ok(&["xhtml"], &page)).unwrap();
    assert!(
        x.starts_with(r#"<html xmlns="http://www.w3.org/1999/xhtml""#),
        "{x}"
    );
    // It is XML: the tool reads it back.
    ok(&["encode"], x.as_bytes());
}

#[test]
fn xhtml5_report_and_suggest() {
    let dir = std::env::temp_dir().join(format!("exi-cli-html-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("odd.html");
    std::fs::write(
        &p,
        r#"<!DOCTYPE html><p data-foo="1" aria-label="x">a<blink>b</blink><ol start="3"><li>c</ol>"#,
    )
    .unwrap();
    let p = p.to_str().unwrap();
    let report = String::from_utf8(ok(&["xhtml5", "report", p], b"")).unwrap();
    assert!(
        report.contains("{http://www.w3.org/1999/xhtml}blink"),
        "{report}"
    );
    assert!(report.contains("{}start"), "{report}");
    let suggest = String::from_utf8(ok(&["xhtml5", "suggest", p], b"")).unwrap();
    assert!(suggest.contains(r#"<xs:element name="blink""#), "{suggest}");
    assert!(
        suggest.contains(r#"<xs:attribute name="start""#),
        "{suggest}"
    );
    for name in ["data-foo", "aria-label"] {
        assert!(report.contains(&format!("{{}}{name}")), "{report}");
        assert!(!suggest.contains(name), "{suggest}");
    }
    // A page on standard input, the report to a file.
    let page = std::fs::read(p).unwrap();
    let out = dir.join("report.txt");
    let out = out.to_str().unwrap();
    assert!(ok(&["xhtml5", "report", "-", "-o", out], &page).is_empty());
    assert_eq!(std::fs::read_to_string(out).unwrap(), report);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn usage_errors() {
    let page = std::fs::read(PAGE).unwrap();
    assert!(err(&["encode", "--html", "--xhtml5", "-p", "comments"], &page).contains("--xhtml5"));
    assert!(
        err(
            &["encode", "--html", "--xhtml5", "-s", "schemas/xhtml5-1.exig"],
            &page
        )
        .contains("leave out -s")
    );
    assert!(err(&["xhtml5", "report"], b"").contains("page"));
    assert!(err(&["xhtml5", "frob", PAGE], b"").contains("report or suggest"));
}

#[test]
fn alternate_profile_codings_with_and_without_header_options() {
    use exi::{Coding, Limits, SchemaId};
    let html = b"<p>hello &amp; goodbye";
    let want = b"<!DOCTYPE html><html><head></head><body><p>hello &amp; goodbye</p></body></html>";
    for (flag, coding) in [
        #[cfg(feature = "bit-packed")]
        ("bit", Coding::BitPacked),
        #[cfg(feature = "compression")]
        ("pre", Coding::PreCompression),
        #[cfg(feature = "compression")]
        ("compression", Coding::Compression),
        ("byte", Coding::ByteAligned),
    ] {
        let opts = ["encode", "--html", "--xhtml5", "-c", flag];
        let stream = ok(&opts, html);
        let header = exi::header::parse_header(&stream, &Limits::new())
            .unwrap()
            .unwrap();
        let options = header.options.unwrap();
        assert_eq!(options.coding(), coding);
        assert_eq!(
            options.schema_id(),
            &SchemaId::Id(exi::html::profile::SCHEMA_V1.into())
        );
        assert_eq!(ok(&["decode", "--html"], &stream), want);
        let bare = ok(&[&opts[..], &["--bare"]].concat(), html);
        assert!(
            exi::header::parse_header(&bare, &Limits::new())
                .unwrap()
                .unwrap()
                .options
                .is_none()
        );
        assert_eq!(
            ok(&["decode", "--html", "--xhtml5", "-c", flag], &bare),
            want
        );
    }
}

#[test]
fn html_decode_requires_the_named_schema() {
    let stream = ok(
        &["encode", "--html", "-s", "schemas/xhtml5-1.exig", "--non-strict"],
        b"<p>hello",
    );
    let error = err(&["decode", "--html"], &stream);
    assert!(error.contains("sha256:") && error.contains("-s"), "{error}");
}
