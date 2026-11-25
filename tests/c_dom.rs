// tests/c_dom.rs
//
// C document tree compatibility tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The C DOM from C: `tests/c/dom_test.c` is a libxml2 program. It is built against
//! libexi through `exi_libxml_compat.h`, and, where libxml2's shared library is
//! installed, against libxml2 too; both must print the same, except lines marked `!`
//! where libexi differs on purpose. Under valgrind when it is installed.
//!
//! Needs a C compiler (`cc`); skipped without one. Builds `libexi.so` with
//! `cargo rustc --crate-type cdylib` into `target/c-api`.
#![cfg(feature = "capi")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Builds libexi.so once, returning its directory.
fn libexi() -> PathBuf {
    let target = root().join("target/c-api");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .current_dir(root())
        .args([
            "rustc",
            "--quiet",
            "--lib",
            "--crate-type",
            "cdylib",
            "--features",
            "capi",
            "--target-dir",
        ])
        .arg(&target)
        .status()
        .expect("cargo runs");
    assert!(st.success(), "building libexi.so");
    target.join("debug")
}

fn cc(out: &Path, args: &[&str]) {
    let o = Command::new("cc")
        .args(["-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(out)
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "cc {args:?}:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// The libexi.so the programs load: the one just built. `cargo test` puts its own
/// target directory on `LD_LIBRARY_PATH`, which outranks the rpath, and an older
/// libexi.so there would be loaded instead.
fn program(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.env("LD_LIBRARY_PATH", root().join("target/c-api/debug"));
    c
}

fn run(exe: &Path) -> String {
    let o = program(exe).output().unwrap();
    assert!(
        o.status.success(),
        "{} failed:\n{}{}",
        exe.display(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

fn libxml2() -> Option<PathBuf> {
    [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/lib/x86_64-linux-gnu",
        "/usr/local/lib",
    ]
    .iter()
    .map(|d| Path::new(d).join("libxml2.so.2"))
    .find(|p| p.exists())
}

#[test]
fn c_programs() {
    if !have("cc") {
        eprintln!("no C compiler: skipped");
        return;
    }
    let lib = libexi();
    let lib_s = lib.to_str().unwrap();
    let inc = root().join("include");
    let inc_s = format!("-I{}", inc.display());
    let rpath = format!("-Wl,-rpath,{lib_s}");
    let tmp = std::env::temp_dir().join(format!("exi-c-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let c = root().join("tests/c");

    let layer1 = tmp.join("layer1");
    cc(
        &layer1,
        &[
            &inc_s,
            c.join("layer1.c").to_str().unwrap(),
            &format!("-L{lib_s}"),
            "-lexi",
            &rpath,
        ],
    );
    run(&layer1);

    // The layouts: C and Rust agree.
    let layout = tmp.join("layout");
    cc(&layout, &[&inc_s, c.join("layout.c").to_str().unwrap()]);
    let printed = run(&layout);
    {
        use core::mem::{offset_of, size_of};
        use exi::dom::raw::*;
        let rust = [
            ("exiNode".to_string(), size_of::<RawNode>()),
            ("exiAttr".into(), size_of::<RawAttr>()),
            ("exiNs".into(), size_of::<RawNs>()),
            ("exiDtd".into(), size_of::<RawDtd>()),
            ("exiDoc".into(), size_of::<RawDoc>()),
            ("exiNode.content".into(), offset_of!(RawNode, content)),
            ("exiNode.nsDef".into(), offset_of!(RawNode, nsDef)),
            ("exiNode.line".into(), offset_of!(RawNode, line)),
            ("exiNode.uriId".into(), offset_of!(RawNode, uriId)),
            ("exiNode.localId".into(), offset_of!(RawNode, localId)),
            ("exiAttr.ns".into(), offset_of!(RawAttr, ns)),
            ("exiAttr.atype".into(), offset_of!(RawAttr, atype)),
            ("exiAttr.uriId".into(), offset_of!(RawAttr, uriId)),
            ("exiNs.href".into(), offset_of!(RawNs, href)),
            ("exiNs.context".into(), offset_of!(RawNs, context)),
            ("exiNs.uriId".into(), offset_of!(RawNs, uriId)),
            ("exiDtd.ExternalID".into(), offset_of!(RawDtd, ExternalID)),
            ("exiDtd.pentities".into(), offset_of!(RawDtd, pentities)),
            ("exiDoc.intSubset".into(), offset_of!(RawDoc, intSubset)),
            ("exiDoc.URL".into(), offset_of!(RawDoc, URL)),
            ("exiDoc.properties".into(), offset_of!(RawDoc, properties)),
        ];
        let c_side: Vec<(String, usize)> = printed
            .lines()
            .map(|l| l.split_once(' ').unwrap())
            .map(|(k, v)| (k.to_string(), v.parse().unwrap()))
            .collect();
        assert_eq!(c_side, rust);
    }

    // The libxml2 program against libexi.
    let src = c.join("dom_test.c");
    let exe = tmp.join("dom_exi");
    cc(
        &exe,
        &[
            &inc_s,
            src.to_str().unwrap(),
            &format!("-L{lib_s}"),
            "-lexi",
            &rpath,
        ],
    );
    let ours = run(&exe);
    assert!(ours.ends_with("done\n"), "{ours}");
    assert!(!ours.contains("BAD-"), "{ours}");
    assert!(ours.contains(r#"!built: <root xmlns:x="urn:x"><x:item id="1" x:k="v">text<x:inner/></x:item>tail+more<!-- note --><?pi data?><title xmlns="http://www.w3.org/2000/svg" xml:lang="en">New title (2)</title></root>"#), "{ours}");

    // Against libxml2: the same output.
    if let Some(xml2) = libxml2() {
        let exe2 = tmp.join("dom_xml2");
        cc(
            &exe2,
            &[
                "-DUSE_LIBXML2",
                src.to_str().unwrap(),
                xml2.to_str().unwrap(),
            ],
        );
        let theirs = run(&exe2);
        let keep = |s: &str| {
            s.lines()
                .filter(|l| !l.starts_with('!'))
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(keep(&ours), keep(&theirs), "libexi and libxml2 differ");
    } else {
        eprintln!("libxml2.so.2 not found: compared with nothing");
    }

    // No memory errors.
    if have("valgrind") {
        let o = program(Path::new("valgrind"))
            .args([
                "-q",
                "--error-exitcode=9",
                "--leak-check=full",
                "--errors-for-leak-kinds=definite",
            ])
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "valgrind:\n{}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    std::fs::remove_dir_all(&tmp).ok();
}

/// Every C export of the DOM is declared in `exi_dom.h`, and mapped in the compat header
/// unless it is libexi's own.
#[test]
fn headers_declare_every_export() {
    let src = std::fs::read_to_string(root().join("src/capi/dom.rs")).unwrap();
    let h = std::fs::read_to_string(root().join("include/exi_dom.h")).unwrap();
    let compat = std::fs::read_to_string(root().join("include/exi_libxml_compat.h")).unwrap();
    let own = [
        "exiNewDocWithStrings",
        "exiNodeGetValue",
        "exiNodeSetValue",
        "exiNameLookup",
        "exiDocReadFrom",
        "exiDocWriteTo",
        "exiNodeWriteTo",
        "exiNodeDumpMemory",
    ];
    let mut n = 0;
    for line in src.lines() {
        if let Some(rest) = line
            .strip_prefix("pub unsafe extern \"C\" fn ")
            .or_else(|| line.strip_prefix("pub extern \"C\" fn "))
        {
            let name = &rest[..rest.find('(').unwrap()];
            assert!(
                h.contains(&format!(" {name}(")) || h.contains(&format!("*{name}(")),
                "{name} is not in exi_dom.h"
            );
            if !own.contains(&name) {
                assert!(
                    compat.contains(&format!("#define xml{} {name}", &name[3..])),
                    "{name} is not in exi_libxml_compat.h"
                );
            }
            n += 1;
        }
    }
    assert!(n > 70, "{n} exports");
}

/// The layer 1 bridges, through the C ABI: a document from a reader of XML text, written
/// to an EXI writer, read back from an EXI reader; and a typed value's view.
#[test]
fn layer1_bridges() {
    use exi::capi::dom::*;
    use exi::capi::*;
    // SAFETY: C ABI calls with valid arguments, as their documentation asks.
    unsafe {
        let strings = exi_strings_new(core::ptr::null());
        // XML text in.
        let mut rc = core::mem::zeroed::<exi_reader_config>();
        rc.size = core::mem::size_of::<exi_reader_config>() as u32;
        rc.format = EXI_FORMAT_XML;
        rc.strings = strings;
        let r = exi_reader_new(&rc);
        let xml = br#"<msg xmlns="jabber:client" to="romeo@example.net"><body>Hi</body></msg>"#;
        assert_eq!(
            exi_reader_feed(
                r,
                xml.as_ptr().cast(),
                xml.len(),
                EXI_FEED_FINAL,
                core::ptr::null_mut()
            ),
            EXI_OK
        );
        let doc = exiDocReadFrom(r, strings);
        assert!(!doc.is_null());
        exi_reader_free(r);
        let root = exiDocGetRootElement(doc);
        // Name ids match the reader's tables: compare as integers.
        let (mut u, mut l) = (0u32, 0u32);
        assert_eq!(
            exiNameLookup(
                doc,
                c"jabber:client".as_ptr().cast(),
                c"msg".as_ptr().cast(),
                &mut u,
                &mut l
            ),
            1
        );
        assert_eq!(((*root).uriId, (*root).localId), (u, l));

        // EXI out.
        let mut o = core::mem::zeroed::<exi_options>();
        exi_options_init(&mut o);
        o.coding = EXI_CODING_BYTE_ALIGNED;
        let mut wc = core::mem::zeroed::<exi_writer_config>();
        wc.size = core::mem::size_of::<exi_writer_config>() as u32;
        wc.options = &o;
        wc.strings = strings;
        let w = exi_writer_new(&wc);
        assert_eq!(exiDocWriteTo(doc, w), EXI_OK);
        let (mut p, mut n) = (core::ptr::null(), 0);
        assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
        let exi = core::slice::from_raw_parts(p, n).to_vec();
        exi_writer_free(w);
        assert!(exi.len() < xml.len());

        // EXI in: the same document.
        rc.format = EXI_FORMAT_EXI;
        let r = exi_reader_new(&rc);
        assert_eq!(
            exi_reader_feed(
                r,
                exi.as_ptr().cast(),
                exi.len(),
                EXI_FEED_FINAL,
                core::ptr::null_mut()
            ),
            EXI_OK
        );
        let back = exiDocReadFrom(r, strings);
        exi_reader_free(r);
        let (mut a, mut an, mut b, mut bn) = (core::ptr::null_mut(), 0, core::ptr::null_mut(), 0);
        exiDocDumpMemory(doc, &mut a, &mut an);
        exiDocDumpMemory(back, &mut b, &mut bn);
        assert_eq!(
            core::slice::from_raw_parts(a, an as usize),
            core::slice::from_raw_parts(b, bn as usize)
        );
        exiFree(a.cast());
        exiFree(b.cast());

        // One element as a stanza, to XML text.
        let mut wc2 = wc;
        wc2.format = EXI_FORMAT_XML;
        let w = exi_writer_new(&wc2);
        assert_eq!(exiNodeWriteTo(exiFirstElementChild(root), w), EXI_OK);
        assert_eq!(exi_writer_flush(w, core::ptr::null_mut()), EXI_OK);
        let (mut p, mut n) = (core::ptr::null(), 0);
        assert_eq!(exi_writer_take(w, &mut p, &mut n), EXI_OK);
        assert_eq!(
            core::slice::from_raw_parts(p, n),
            br#"<body xmlns="jabber:client">Hi</body>"#
        );
        exi_writer_free(w);

        // A typed value: content is NULL, the value is typed, text on request.
        let t = exiNewDocText(doc, c"x".as_ptr().cast());
        let mut v = core::mem::zeroed::<exi_value>();
        v.r#type = EXI_V_INT;
        v.u.integer = 42;
        assert_eq!(exiNodeSetValue(t, &v), EXI_OK);
        assert!((*t).content.is_null());
        let mut out = core::mem::zeroed::<exi_value>();
        assert_eq!(exiNodeGetValue(t, &mut out), EXI_OK);
        assert_eq!((out.r#type, out.u.integer), (EXI_V_INT, 42));
        let s = exiNodeGetContent(t);
        assert_eq!(std::ffi::CStr::from_ptr(s.cast()).to_str().unwrap(), "42");
        exiFree(s.cast());

        exiFreeDoc(back);
        exiFreeDoc(doc);
        exi_strings_free(strings);
    }
}
