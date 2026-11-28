// tests/wasm.rs
//
// WebAssembly EXI and EXIG integration tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Compiles `tests/wasm` (`exi-wasm-test`) to `wasm32-unknown-unknown` in both
//! byte-aligned and bit-packed configurations with `exig` enabled, then runs
//! the compiled `.wasm` modules against repository EXI and EXIG fixtures.
//!
//! Needs the `wasm32-unknown-unknown` target and `node` on `PATH`; skipped
//! cleanly when either is missing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn have_tool(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn have_target(target: &str) -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .is_ok_and(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|line| line.trim() == target)
        })
}

fn build_wasm(profile_name: &str, target: &str, features: &str) -> PathBuf {
    let manifest = root().join("tests/wasm/Cargo.toml");
    let target_dir = root().join("target/wasm-test").join(profile_name);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(root())
        .args([
            "build",
            "--manifest-path",
        ])
        .arg(&manifest)
        .args([
            "--locked",
            "--offline",
            "--release",
            "--target",
            target,
            "--features",
            features,
            "--target-dir",
        ])
        .arg(&target_dir)
        .output()
        .expect("cargo runs");
    assert!(
        out.status.success(),
        "building wasm ({target}, {features}):\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    target_dir.join(format!("{target}/release/exi_wasm_test.wasm"))
}

fn run_node_suite(byte_wasm: &Path, bit_wasm: &Path) {
    let script = r#"
const fs = require("node:fs");
const assert = require("node:assert/strict");

async function load(wasmPath) {
    const bytes = fs.readFileSync(wasmPath);
    const { instance } = await WebAssembly.instantiate(bytes, {});
    const w = instance.exports;
    const withBuf = (data, fn) => {
        const ptr = w.alloc_buf(data.length);
        try {
            if (data.length > 0) {
                new Uint8Array(w.memory.buffer, ptr, data.length).set(data);
            }
            return fn(ptr, data.length);
        } finally {
            w.free_buf(ptr, data.length);
        }
    };
    return {
        raw: w,
        parseSchemaless: (data) => withBuf(data, (p, n) => w.wasm_parse_exi_schemaless(p, n)),
        parseExig: (data) => withBuf(data, (p, n) => w.wasm_parse_exig(p, n)),
        parseWithExig: (exig, exi, strict) =>
            withBuf(exig, (ep, en) =>
                withBuf(exi, (xp, xn) => w.wasm_parse_exi_with_exig(ep, en, xp, xn, strict ? 1 : 0))
            ),
    };
}

(async () => {
    const root = process.argv[1];
    const byte = await load(process.argv[2]);
    const bit = await load(process.argv[3]);

    const read = (rel) => fs.readFileSync(`${root}/${rel}`);

    // 1. Existing esp32c6 reading exports remain intact.
    for (const m of [byte, bit]) {
        const len = m.raw.encode_reading(21n);
        assert(len > 0, "encode_reading failed");
        assert.equal(m.raw.decode_reading(len), 0);
        assert.equal(m.raw.reading(), 21n);
    }

    // 2. Schema-less EXI parsing (byte-aligned & bit-packed + malformed rejection).
    assert.equal(byte.parseSchemaless(read("fixtures/exificient/a-byte.exi")), 4);
    assert.equal(byte.parseSchemaless(read("fixtures/exificient/a-byte-options.exi")), 4);
    assert.equal(byte.parseSchemaless(read("fixtures/exificient/a-byte-cookie.exi")), 4);
    assert(byte.parseSchemaless(read("fixtures/exificient/plain.exi")) > 4);
    assert(byte.parseSchemaless(Uint8Array.of()) < 0);
    assert(byte.parseSchemaless(Uint8Array.of(0xff, 0xff)) < 0);
    const plainByte = read("fixtures/exificient/plain.exi");
    assert(byte.parseSchemaless(plainByte.subarray(0, plainByte.length - 1)) < 0);
    assert(byte.parseSchemaless(read("fixtures/exificient/a-bit-options.exi")) < 0);

    assert.equal(bit.parseSchemaless(read("fixtures/exificient/a-bit.exi")), 4);
    assert.equal(bit.parseSchemaless(read("fixtures/exificient/a-bit-options.exi")), 4);
    assert.equal(bit.parseSchemaless(read("fixtures/exificient/a-byte-options-bitp.exi")), 4);
    assert.equal(bit.parseSchemaless(read("fixtures/exificient/a-byte-cookie-bitp.exi")), 4);
    assert.equal(bit.parseSchemaless(read("fixtures/exificient/a-byte-options.exi")), 4);
    assert(bit.parseSchemaless(read("fixtures/exificient/plain-bitp.exi")) > 4);
    assert(bit.parseSchemaless(Uint8Array.of()) < 0);
    assert(bit.parseSchemaless(Uint8Array.of(0xff, 0xff)) < 0);
    const plainBit = read("fixtures/exificient/plain-bitp.exi");
    assert(bit.parseSchemaless(plainBit.subarray(0, plainBit.length - 1)) < 0);

    // 3. EXIG parsing, round-trip, digest check, and malformed rejection.
    const complexExig = read("fixtures/input/complex.exig");
    const exig1 = read("fixtures/input/exig-1.exig");
    const xhtml5Exig = read("schemas/xhtml5-1.exig");
    for (const m of [byte, bit]) {
        assert.equal(m.parseExig(complexExig), 0);
        assert.equal(m.parseExig(exig1), 0);
        assert.equal(m.parseExig(xhtml5Exig), 0);
        assert(m.parseExig(Uint8Array.of()) < 0);
        assert(m.parseExig(Buffer.from("not-an-exig")) < 0);
        assert(m.parseExig(complexExig.subarray(0, complexExig.length - 1)) < 0);
    }

    // 4. Schema-informed EXI decoding using loaded .exig grammars.
    const complexStrictByte = read("fixtures/exificient/complex-strict.exi");
    const complexStrictBit = read("fixtures/exificient/complex-strict-bitp.exi");
    const complexExigByte = read("fixtures/exificient/complex-exig-encode.exi");
    const complexExigBit = read("fixtures/exificient/complex-exig-encode-bitp.exi");

    assert(byte.parseWithExig(complexExig, complexStrictByte, true) > 0);
    assert(byte.parseWithExig(exig1, complexExigByte, true) > 0);
    assert(byte.parseWithExig(complexExig, complexStrictByte.subarray(0, complexStrictByte.length - 1), true) < 0);

    assert(bit.parseWithExig(complexExig, complexStrictBit, true) > 0);
    assert(bit.parseWithExig(exig1, complexExigBit, true) > 0);
    assert(bit.parseWithExig(complexExig, complexStrictBit.subarray(0, complexStrictBit.length - 1), true) < 0);
})().catch((err) => {
    console.error(err);
    process.exit(1);
});
"#;

    let out = Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(root())
        .arg(byte_wasm)
        .arg(bit_wasm)
        .output()
        .expect("node runs");
    assert!(
        out.status.success(),
        "node wasm suite failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn wasm_exi_and_exig() {
    if !have_target("wasm32-unknown-unknown") {
        eprintln!("skipping wasm_exi_and_exig: wasm32-unknown-unknown target not installed");
        return;
    }
    if !have_tool("node") {
        eprintln!("skipping wasm_exi_and_exig: node not found on PATH");
        return;
    }
    let byte_wasm = build_wasm("byte-aligned", "wasm32-unknown-unknown", "exig");
    let bit_wasm = build_wasm("bit-packed", "wasm32-unknown-unknown", "exig,bit-packed");
    run_node_suite(&byte_wasm, &bit_wasm);
}

#[test]
fn wasm_emscripten_exi_and_exig() {
    if !have_target("wasm32-unknown-emscripten") {
        eprintln!(
            "skipping wasm_emscripten_exi_and_exig: wasm32-unknown-emscripten target not installed"
        );
        return;
    }
    if !have_tool("emcc") {
        eprintln!("skipping wasm_emscripten_exi_and_exig: emcc not found on PATH");
        return;
    }
    if !have_tool("node") {
        eprintln!("skipping wasm_emscripten_exi_and_exig: node not found on PATH");
        return;
    }
    let byte_wasm = build_wasm("emscripten-byte-aligned", "wasm32-unknown-emscripten", "exig");
    let bit_wasm = build_wasm(
        "emscripten-bit-packed",
        "wasm32-unknown-emscripten",
        "exig,bit-packed",
    );
    run_node_suite(&byte_wasm, &bit_wasm);
}
