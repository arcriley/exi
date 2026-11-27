// tests/cli.rs
//
// EXI command-line tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The `exi` command-line tool, run as a user would.
#![cfg(all(feature = "infer", feature = "std", feature = "exig"))]

use std::io::Write;
use std::process::{Command, Output, Stdio};

const EXI: &str = env!("CARGO_BIN_EXE_exi");
const SVG: &str = "fixtures/input/infer/logo.svg";

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

#[test]
fn schemaless_round_trip_through_pipes() {
    let svg = std::fs::read(SVG).unwrap();
    let exi = ok(&["encode", "-p", "prefixes,comments"], &svg);
    assert!(exi.len() < svg.len());
    let back = ok(&["decode", "-p", "prefixes,comments"], &exi);
    // The same document: encoding the decoded text gives the same stream.
    assert_eq!(ok(&["encode", "-p", "prefixes,comments"], &back), exi);
}

#[test]
fn options_take_name_equals_value() {
    let svg = std::fs::read(SVG).unwrap();
    let spaced = ok(&["encode", "--coding", "bit", "-p", "comments"], &svg);
    assert_eq!(ok(&["encode", "--coding=bit", "--preserve=comments"], &svg), spaced);
}

/// decode says which flags a header with options makes pointless.
#[test]
fn decode_names_the_flags_the_header_overrides() {
    let svg = std::fs::read(SVG).unwrap();
    let exi = ok(&["encode"], &svg);
    let o = run(&["decode", "-c", "bit", "--block", "10"], &exi);
    assert!(o.status.success());
    let warning = String::from_utf8(o.stderr).unwrap();
    assert!(warning.contains("ignoring --coding, --block"), "{warning}");
    assert!(run(&["decode"], &exi).stderr.is_empty());
}

#[test]
fn bare_headers_need_the_same_options() {
    let svg = std::fs::read(SVG).unwrap();
    let exi = ok(&["encode", "--bare"], &svg);
    assert_eq!(exi[0], 0x80);
    let back = ok(&["decode", "--bare"], &exi);
    assert_eq!(ok(&["encode", "--bare"], &back), exi);
}

#[test]
fn inferred_schema_names_itself_in_the_stream() {
    let dir = std::env::temp_dir().join(format!("exi-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exig = dir.join("logo.exig");
    let exig = exig.to_str().unwrap();
    ok(&["infer", "--exact", SVG, "-o", exig], b"");
    let strict = ok(&["encode", "-s", exig, SVG], b"");
    let digest = String::from_utf8(ok(&["digest", exig], b"")).unwrap();
    let info = String::from_utf8(ok(&["info"], &strict)).unwrap();
    assert!(
        info.contains(&format!("schema: {}", digest.trim())),
        "{info}"
    );
    assert!(err(&["decode"], &strict).contains("needs the schema `sha256:"));
    let back = ok(&["decode", "-s", exig], &strict);
    assert_eq!(ok(&["encode", "-s", exig], &back), strict);
    // A different schema is refused by name.
    let xsd = dir.join("xsd");
    ok(&["infer", SVG, "-d", xsd.to_str().unwrap()], b"");
    let main = xsd.join("schema.xsd");
    let xlink = xsd.join("schema-xlink.xsd");
    assert!(
        err(
            &[
                "decode",
                "-s",
                main.to_str().unwrap(),
                "-s",
                xlink.to_str().unwrap()
            ],
            &strict
        )
        .contains("but the schema given is")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn usage_errors() {
    assert!(err(&["frobnicate"], b"").contains("unknown command"));
    assert!(err(&["encode", "--nope"], b"").contains("unknown option"));
    assert!(err(&["encode", "-p", "colour"], b"").contains("unknown --preserve item"));
    assert!(err(&["decode"], b"<a/>").contains("not an EXI stream"));
    assert!(err(&[], b"").starts_with("exi: no command given\n\nexi: "));
    assert!(err(&["encode", "--bare=yes"], b"").contains("--bare takes no value"));
    assert!(err(&["encode", "--coding="], b"").contains("unknown --coding ``"));
    assert!(
        err(&["infer", "fixtures/input/infer/feed.xml"], b"").contains("give a directory with -d")
    );
}

#[test]
fn every_coding_through_pipes() {
    let svg = std::fs::read(SVG).unwrap();
    let byte = ok(
        &["decode", "-p", "prefixes,comments"],
        &ok(&["encode", "-p", "prefixes,comments"], &svg),
    );
    let mut sizes = Vec::new();
    for c in ["bit", "pre", "compression"] {
        let exi = ok(
            &[
                "encode",
                "-c",
                c,
                "--block",
                "20",
                "-p",
                "prefixes,comments",
            ],
            &svg,
        );
        // The header names the coding, so decode needs no flag.
        assert_eq!(
            ok(&["decode", "-p", "prefixes,comments"], &exi),
            byte,
            "{c}"
        );
        sizes.push(exi.len());
        // A bare header needs the same flags on both sides.
        let bare = ok(&["encode", "-c", c, "--block", "20", "--bare"], &svg);
        assert_eq!(
            ok(&["decode", "-c", c, "--block", "20", "--bare"], &bare),
            ok(&["decode"], &ok(&["encode"], &svg)),
            "{c} bare"
        );
    }
    // Bit-packed is smaller than byte-aligned; compression smaller still.
    let plain = ok(&["encode", "-p", "prefixes,comments"], &svg).len();
    assert!(sizes[0] < plain && sizes[2] < sizes[0], "{plain} {sizes:?}");
    let stored = ok(&["encode", "-c", "compression", "--level", "0"], &svg);
    let best = ok(&["encode", "-c", "compression", "--level", "9"], &svg);
    assert!(best.len() < stored.len());
    assert!(err(&["encode", "-c", "zip"], &svg).contains("unknown --coding"));
    assert!(err(&["encode", "--block", "x"], &svg).contains("--block"));
}

#[test]
fn canonical_through_pipes() {
    let a = br#"<r b="2" a="1"><p>one</p>
  <q/></r>"#;
    let b = br#"<r a="1" b="2"><p>one</p><q/></r>"#;
    let ca = ok(&["encode", "--canonical", "-c", "bit"], a);
    assert_eq!(ca, ok(&["encode", "--canonical", "-c", "bit"], b));
    assert_ne!(
        ok(&["encode", "-c", "bit"], a),
        ok(&["encode", "-c", "bit"], b)
    );
    assert!(err(&["encode", "--canonical", "--cookie"], b).contains("cookie"));
    assert!(err(&["encode", "--canonical", "-c", "compression"], b).contains("pre-compression"));
}
