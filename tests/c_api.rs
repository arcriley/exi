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
//! `include/exi.h` is written by hand. These tests hold it to
//! `src/capi/mod.rs`: the same functions with the same C types, the
//! same `#define` values, the same struct sizes, alignments and field
//! offsets, and it compiles as strict C and C++ with `exi_dom.h` in
//! either order.
//!
//! The checks that need a C compiler are skipped without one.
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

fn header() -> String {
    std::fs::read_to_string(root().join("include/exi.h")).unwrap()
}

fn rust() -> String {
    std::fs::read_to_string(root().join("src/capi/mod.rs")).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("exi-c-api-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn include() -> String {
    format!("-I{}", root().join("include").display())
}

/// Compiles `src` with `cc` and the C standard `std` (warnings are
/// errors); `out` NULL for a syntax check only.
fn cc(cc: &str, std: &str, lang: &str, src: &Path, out: Option<&Path>) {
    let mut c = Command::new(cc);
    c.args([
        std,
        "-Wall",
        "-Wextra",
        "-pedantic",
        "-Werror",
        &include(),
        "-x",
        lang,
    ])
    .arg(src);
    match out {
        Some(o) => c.arg("-o").arg(o),
        None => c.arg("-fsyntax-only"),
    };
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{cc} {}:\n{}",
        src.display(),
        String::from_utf8_lossy(&o.stderr)
    );
}

// ----- reading src/capi/mod.rs -----

/// `(name, value)` of every `pub const`.
fn consts(src: &str) -> Vec<(String, String)> {
    src.lines()
        .filter_map(|l| l.strip_prefix("pub const "))
        .map(|l| {
            let (name, rest) = l.split_once(':').unwrap();
            let value =
                rest.split_once('=').unwrap().1.trim().trim_end_matches(';');
            (name.to_string(), value.to_string())
        })
        .collect()
}

/// Every `pub struct` and `pub union` with public fields: name and
/// field names.
fn structs(src: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut lines = src.lines();
    while let Some(l) = lines.next() {
        let Some(name) = l
            .strip_prefix("pub struct ")
            .or_else(|| l.strip_prefix("pub union "))
            .and_then(|r| r.strip_suffix(" {"))
        else {
            continue;
        };
        let mut fields = Vec::new();
        for f in lines.by_ref() {
            if f == "}" {
                break;
            }
            if let Some(f) = f.trim().strip_prefix("pub ") {
                let n = f.split_once(':').unwrap().0;
                fields.push(n.trim_start_matches("r#").to_string());
            }
        }
        if !fields.is_empty() {
            out.push((name.to_string(), fields));
        }
    }
    out
}

/// A Rust type of the C ABI as C.
fn c_type(t: &str) -> String {
    let t = t.trim();
    if let Some(inner) = t.strip_prefix("*const ") {
        return format!("{} const *", c_type(inner));
    }
    if let Some(inner) = t.strip_prefix("*mut ") {
        return format!("{} *", c_type(inner));
    }
    if t.starts_with("Option<extern \"C\" fn(*mut c_void, *const u8, usize)") {
        return "exi_write_fn".into();
    }
    match t {
        "u8" => "uint8_t",
        "u32" => "uint32_t",
        "u64" => "uint64_t",
        "i32" => "int32_t",
        "usize" => "size_t",
        "c_char" => "char",
        "c_int" => "int",
        "c_void" | "()" => "void",
        _ => {
            assert!(t.starts_with("exi_"), "no C form for `{t}`");
            t
        }
    }
    .into()
}

/// Splits `s` at the commas outside brackets.
fn split_top(s: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '(' | '<' => depth += 1,
            ')' | '>' if !s[..i].ends_with('-') => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// Every export: name, C return type, C parameter types.
fn exports(src: &str) -> Vec<(String, String, Vec<String>)> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("extern \"C\" fn ") {
        let before = &rest[..i];
        let line_start = before.rfind('\n').map_or(0, |n| n + 1);
        let is_export = before[line_start..].starts_with("pub ");
        rest = &rest[i + "extern \"C\" fn ".len()..];
        if !is_export {
            continue;
        }
        let open = rest.find('(').unwrap();
        let name = rest[..open].to_string();
        // The parameter list: up to the matching parenthesis.
        let mut depth = 0;
        let close = rest
            .char_indices()
            .skip(open)
            .find(|&(_, c)| {
                depth += match c {
                    '(' => 1,
                    ')' => -1,
                    _ => 0,
                };
                depth == 0
            })
            .unwrap()
            .0;
        let params = split_top(&rest[open + 1..close])
            .into_iter()
            .map(|p| c_type(p.split_once(':').unwrap().1))
            .collect();
        let tail = &rest[close + 1..rest[close..].find('{').unwrap() + close];
        let ret = tail.trim().strip_prefix("->").map_or("void".into(), c_type);
        out.push((name, ret, params));
    }
    out
}

// ----- the tests -----

/// The header declares exactly the exports, and C sees each with the
/// Rust types: a function pointer of the Rust signature takes the
/// declared function, so a different parameter or return type does not
/// compile.
#[test]
fn functions_match() {
    let ex = exports(&rust());
    assert!(ex.len() > 30, "{} exports", ex.len());
    let h = header();
    let declared: Vec<&str> = h
        .lines()
        .filter_map(|l| l.strip_prefix("EXI_API "))
        .map(|l| l[..l.find('(').unwrap()].rsplit([' ', '*']).next().unwrap())
        .collect();
    let mut names: Vec<&str> = ex.iter().map(|e| e.0.as_str()).collect();
    names.sort_unstable();
    let mut d = declared.clone();
    d.sort_unstable();
    assert_eq!(
        d, names,
        "the functions exi.h declares and the ones src/capi/mod.rs exports"
    );
    if !have("cc") {
        eprintln!("no cc: types not checked");
        return;
    }
    let mut c = String::from("#include \"exi.h\"\n");
    for (name, ret, params) in &ex {
        let p =
            if params.is_empty() { "void".into() } else { params.join(", ") };
        c.push_str(&format!("{ret} (*const check_{name})({p}) = {name};\n"));
    }
    let dir = tmp("functions");
    let file = dir.join("functions.c");
    std::fs::write(&file, c).unwrap();
    cc("cc", "-std=c11", "c", &file, None);
    std::fs::remove_dir_all(&dir).ok();
}

/// Every `#define EXI_*` is a Rust constant of the same value, and
/// every constant is defined.
#[test]
fn defines_match() {
    let rust = consts(&rust());
    let h = header();
    let mut defined: Vec<&str> = h
        .lines()
        .filter_map(|l| l.strip_prefix("#define "))
        .map(|l| l.split_whitespace().next().unwrap())
        .filter(|n| !matches!(*n, "EXI_H" | "EXI_API"))
        .collect();
    defined.sort_unstable();
    let mut names: Vec<&str> = rust.iter().map(|c| c.0.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        defined, names,
        "the defines of exi.h and the constants of src/capi/mod.rs"
    );
    if !have("cc") {
        eprintln!("no cc: values not checked");
        return;
    }
    let mut c = String::from("#include \"exi.h\"\n");
    for (name, value) in &rust {
        let v = value
            .replace("u32::MAX", "UINT32_MAX")
            .replace("u64::MAX", "UINT64_MAX");
        c.push_str(&format!(
            "_Static_assert((unsigned long long)({name}) == (unsigned long long)({v}), \"{name}\");\n"
        ));
    }
    let dir = tmp("defines");
    let file = dir.join("defines.c");
    std::fs::write(&file, c).unwrap();
    cc("cc", "-std=c11", "c", &file, None);
    std::fs::remove_dir_all(&dir).ok();
}

/// Every struct and union: the same size, alignment and field offsets
/// in C as in Rust.
#[test]
fn layouts_match() {
    use core::mem::{align_of, offset_of, size_of};
    use exi::capi::*;
    macro_rules! layouts {
        ($($t:ident { $($f:ident),* $(,)? })*) => {
            vec![$((
                stringify!($t),
                size_of::<$t>(),
                align_of::<$t>(),
                vec![$((stringify!($f).trim_start_matches("r#"), offset_of!($t, $f))),*],
            )),*]
        };
    }
    let layout = layouts! {
        exi_str { ptr, len }
        exi_bytes { ptr, len }
        exi_name { uri, local, prefix, uri_id, local_id, global_uri, global_local }
        exi_datetime { kind, tz_minutes, year, month, day, hour, minute, second, has_tz,
            has_frac, _pad, frac_digits, frac }
        exi_decimal { negative, fraction_digits, integral, fraction }
        exi_float { mantissa, exponent, _pad }
        exi_list { items, count }
        exi_value_u { boolean, integer, dec, flt, binary, datetime, qname, list, enum_ordinal }
        exi_value { r#type, flags, text, u }
        exi_nsdecl { prefix, uri, local_element_ns, _pad }
        exi_event { r#type, depth, name, value, pubid, sysid, ns, ns_count }
        exi_options { size, coding, flags, preserve, block_size, value_max_length,
            value_partition_capacity, schema_id_kind, _pad, schema_id }
        exi_limits { size, max_depth, max_attributes, max_integer_octets, max_names,
            max_list_items, max_options_bytes, max_entity_depth, max_value_chars,
            max_binary_bytes, max_string_table_bytes, max_learned_productions,
            max_expanded_bytes }
        exi_strings_config { size, hash_bits, chunk_bytes, _pad, max_bytes, max_strings }
        exi_reader_config { size, flags, sequence, format, options, grammars, limits,
            strings, input_buffer_size, _pad }
        exi_handlers { size, start_document, end_document, start_element, end_element,
            attribute, characters, namespace_decl, comment, processing_instruction,
            doctype, entity_reference, self_contained }
        exi_writer_config { size, flags, header, sequence, format, _pad, options, grammars,
            write, write_user, limits, strings }
    };
    // The list above has every struct of src/capi/mod.rs, field for
    // field.
    let mut listed: Vec<(String, Vec<String>)> = layout
        .iter()
        .map(|(t, _, _, f)| {
            (t.to_string(), f.iter().map(|x| x.0.to_string()).collect())
        })
        .collect();
    let mut parsed = structs(&rust());
    listed.sort();
    parsed.sort();
    assert_eq!(listed, parsed, "add new structs and fields to layouts_match");

    if !have("cc") {
        eprintln!("no cc: layouts not checked");
        return;
    }
    let mut c = String::from(
        "#include <stdio.h>\n#include <stddef.h>\n#include \"exi.h\"\nint main(void) {\n",
    );
    for (t, _, _, fields) in &layout {
        c.push_str(&format!(
            "    printf(\"{t} %zu %zu\\n\", sizeof({t}), _Alignof({t}));\n"
        ));
        for (f, _) in fields {
            c.push_str(&format!(
                "    printf(\"{t}.{f} %zu\\n\", offsetof({t}, {f}));\n"
            ));
        }
    }
    c.push_str("    return 0;\n}\n");
    let dir = tmp("layouts");
    let (file, exe) = (dir.join("layouts.c"), dir.join("layouts"));
    std::fs::write(&file, c).unwrap();
    cc("cc", "-std=c11", "c", &file, Some(&exe));
    let printed =
        String::from_utf8(Command::new(&exe).output().unwrap().stdout)
            .unwrap();
    let mut expected = String::new();
    for (t, size, align, fields) in &layout {
        expected.push_str(&format!("{t} {size} {align}\n"));
        for (f, off) in fields {
            expected.push_str(&format!("{t}.{f} {off}\n"));
        }
    }
    assert_eq!(printed, expected);
    std::fs::remove_dir_all(&dir).ok();
}

/// The headers compile as strict C11 and C++17, alone and together in
/// either order.
#[test]
fn headers_compile() {
    let dir = tmp("compile");
    for (i, order) in
        [&["exi.h"][..], &["exi.h", "exi_dom.h"], &["exi_dom.h", "exi.h"]]
            .iter()
            .enumerate()
    {
        let mut src: String =
            order.iter().map(|h| format!("#include \"{h}\"\n")).collect();
        src.push_str("int main(void) { return exi_abi_version() >> 16 != EXI_ABI_MAJOR; }\n");
        let file = dir.join(format!("t{i}.c"));
        std::fs::write(&file, src).unwrap();
        for (compiler, std, lang) in
            [("cc", "-std=c11", "c"), ("c++", "-std=c++17", "c++")]
        {
            if have(compiler) {
                cc(compiler, std, lang, &file, None);
            } else {
                eprintln!("no {compiler}: skipped");
            }
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}
