// tests/c_api.rs
//
// C ABI header tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! `include/exi.h` is generated from `src/capi/mod.rs` by cbindgen (`cbindgen.toml`) and
//! committed. These tests check it declares every export, matches what cbindgen
//! generates now, and compiles as strict C and C++ with `exi_dom.h` in either order.
//!
//! The cbindgen and compiler checks are skipped when the tool is not installed.
//! Regenerate with `cbindgen --config cbindgen.toml --crate exi -o include/exi.h`.
#![cfg(feature = "capi")]

use std::path::PathBuf;
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

fn header() -> String {
    std::fs::read_to_string(root().join("include/exi.h")).unwrap()
}

/// Every C export of layer 1 is declared in `exi.h`.
#[test]
fn header_declares_every_export() {
    let src = std::fs::read_to_string(root().join("src/capi/mod.rs")).unwrap();
    let h = header();
    let mut n = 0;
    for line in src.lines() {
        if let Some(rest) = line
            .strip_prefix("pub unsafe extern \"C\" fn ")
            .or_else(|| line.strip_prefix("pub extern \"C\" fn "))
        {
            let name = &rest[..rest.find('(').unwrap()];
            assert!(
                h.contains(&format!(" {name}(")) || h.contains(&format!("*{name}(")),
                "{name} is not in exi.h"
            );
            n += 1;
        }
    }
    assert!(n > 30, "{n} exports");
}

/// The committed header is what cbindgen generates from the code now.
#[test]
fn header_is_current() {
    if !have("cbindgen") {
        eprintln!("no cbindgen: skipped");
        return;
    }
    let out = Command::new("cbindgen")
        .current_dir(root())
        .args(["--config", "cbindgen.toml", "--crate", "exi", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8(out.stdout).unwrap() == header(),
        "include/exi.h is out of date: cbindgen --config cbindgen.toml --crate exi -o include/exi.h"
    );
}

/// The headers compile as strict C11 and C++17, alone and together in either order.
#[test]
fn headers_compile() {
    let tmp = std::env::temp_dir().join(format!("exi-c-api-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let inc = format!("-I{}", root().join("include").display());
    for (i, order) in [&["exi.h"][..], &["exi.h", "exi_dom.h"], &["exi_dom.h", "exi.h"]]
        .iter()
        .enumerate()
    {
        let mut src: String = order.iter().map(|h| format!("#include \"{h}\"\n")).collect();
        src.push_str("int main(void) { return exi_abi_version() >> 16 != EXI_ABI_MAJOR; }\n");
        let file = tmp.join(format!("t{i}.c"));
        std::fs::write(&file, src).unwrap();
        for (cc, std, lang) in [("cc", "-std=c11", "c"), ("c++", "-std=c++17", "c++")] {
            if !have(cc) {
                eprintln!("no {cc}: skipped");
                continue;
            }
            let out = Command::new(cc)
                .args([std, "-Wall", "-Wextra", "-pedantic", "-Werror", &inc, "-fsyntax-only"])
                .args(["-x", lang])
                .arg(&file)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{cc} {order:?}:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    std::fs::remove_dir_all(&tmp).ok();
}
