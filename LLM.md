# LLM.md

This file is for LLM sessions working on this repository. It covers how Arc
works, how the code is laid out, how to build and test it, the decisions already made, and what is unfinished. This was added by Claude Code with advice for itself and other LLMs for efficiency.

## The project

- `exi` is a W3C EXI 1.0 implementation in Rust with a C ABI.
- The crate is `exi`; the shared library built from it is `libexi.so`.
- Rust users depend on `exi`.
- License `MIT OR Apache-2.0`.
- Edition 2024, `rust-version = "1.85"`.
- One crate with features, no workspace.
- The engine is `no_std + alloc` and always compiled; `std` adds I/O and conveniences.
- Each layer should build on the one below:
  1. Layer 1 is events (similar to expat or SAX)
  2. Layer 2 is the document tree (`dom`, modeled after libxml2's API)
  3. Layer 3 is XML ORM libraries such as nodetree (outside this repo).
- libexi must handle any XML document.
- XHTML5 with inline SVG and XMPP streams are key use cases

## Coworking with Arc

- Consider Arc an expert in Python with more than 30 years experience with Python (starting with Python 1.2 on 68k NetBSD) and responds well to framing by PEP-8 and PEP-20 terms
- Use direct, simple language.
- Avoid double negatives and nested conditions.
- Say what happens and why.
- Say plainly whether you are reporting what you changed, recommending a change, or
  asking a question. Keep those apart.
- Read the actual files before giving advice.
- Ask when something is unclear instead of guessing.
- Commit or push only when Arc asks.
- Arc edits the doc comments and doctests himself before committing.
- Keep changes to comments small and in the existing style unless asked.
- Spec rules and their reasons belong in doc comments, next to the code they explain.

## How code is written here

- Test-driven, doctests first.
- In this project "doctests" means Rust examples in rustdoc comments. Each public item gets a doctest that works as its unit test.
- Module docs cite the EXI 1.0 sections they implement and often carry a table of
  rules. Follow that pattern.
- Tests that depend on a coding should set it explicitly (for example
  `Coding::ByteAligned`), so they pass in feature subsets.

## Layout

| Path | What it holds |
|---|---|
| `src/lib.rs` | Crate docs and module list |
| `src/coding/` | Bit and byte coding, datatypes, names, `BodyEncoder`/`BodyDecoder` |
| `src/block.rs` | Pre-compression and compression: blocks, channels, DEFLATE (feature `compression`) |
| `src/header.rs`, `src/options.rs` | Header, cookie and the options document |
| `src/engine.rs` | The grammar engine shared by reader and writer |
| `src/reader.rs`, `src/writer.rs` | Layer 1 reader and writer, frames, canonical writing |
| `src/canonical.rs` | Canonical EXI rules, value normalization, EXIficient differences |
| `src/grammar/` | Built-in and schema-informed grammars, `.exig` reading and writing, code-table cache |
| `src/strings.rs`, `src/strings/hash.rs`, `src/tables.rs` | Global string pool and per-stream string tables |
| `src/lexical.rs`, `src/value.rs` | Lexical forms and typed values |
| `src/xml/` | The XML 1.0 text reader and writer (libexi has its own parser on purpose) |
| `src/xsd/` | XSD compiler (feature `xsd`) |
| `src/infer.rs` | Schemas learned from example documents (feature `infer`) |
| `src/html/` | HTML pipeline and the XHTML5 profile (features `html`, `html5ever`, `xhtml5`) |
| `src/dom/` | Layer 2 document tree (feature `dom`) |
| `src/capi/` | C ABI (feature `capi`); `dom.rs` is the libxml2-style C DOM |
| `src/bin/exi.rs` | The `exi` command-line tool |
| `include/` | Generated `exi.h` and hand-written `exi_dom.h` and `exi_libxml_compat.h` |
| `schemas/` | `exig-1.xsd`, `xhtml5-1.xsd`, `xhtml5-1.exig` |
| `fixtures/` | EXIficient reference fixtures and the program that makes them |
| `tests/` | Integration tests; `tests/c/` holds C programs |
| `benches/` | `exi.rs`, `codecs.rs`, `compare.sh`, XMPP session generator |

## Features

Defaults: `std`, `xml`, `bit-packed`, `compression`.

| Feature | Adds |
|---|---|
| `std` | I/O adapters and conveniences |
| `xml` | XML text reader and writer |
| `bit-packed` | Bit-packed bodies. Headers are always bit-packed |
| `compression` | Pre-compression and compression; DEFLATE through flate2 (needs `std`) |
| `xsd` | XSD compiler |
| `infer` | Schema inference (implies `xsd`) |
| `dom` | Layer 2 tree |
| `html`, `html5ever`, `xhtml5` | HTML input, HTML text parsing, XHTML5 profile grammars |
| `capi` | C ABI (implies `std`, `xml`, `dom`) |

## Build and test

```sh
cargo test                      # default features: 167 tests
cargo test --all-features       # everything: 240 tests
cargo build --lib --no-default-features   # the no_std + alloc engine
cargo doc --all-features --no-deps
```

- The C tests (`tests/c_dom.rs`) need `cc`. They build `libexi.so` with
  `cargo rustc --crate-type cdylib` into `target/c-api`, and run under valgrind when it
  is installed. They link a Layer 1 round trip against the generated header and
  compare libexi's libxml2-compatible DOM with real libxml2 where it is installed.
- `tests/html.rs` and `tests/xhtml5.rs` also read saved web pages from the directory
  in `EXI_HTML_CORPUS` when it is set.
- `XHTML5_REGEN=1 cargo test --features xhtml5,html5ever,xsd --test xhtml5` rewrites
  `schemas/xhtml5-1.exig` from `schemas/xhtml5-1.xsd`.

## Fixtures (EXIficient 1.0.7)

`tests/fixtures.rs` compares libexi's output with files EXIficient wrote. Bit-packed and
pre-compression fixtures must match byte for byte. Compression fixtures are compared by
their inflated streams. Fixtures for codings not built are skipped.

To regenerate them (Java 11 or later):

```sh
fixtures/fetch-exificient.sh                  # once: jars into fixtures/jars (git-ignored)
java -cp 'fixtures/jars/*' fixtures/MakeFixtures.java fixtures/manifest.tsv
```

`fixtures/manifest.tsv` has one line per fixture: name, options, input, schema. The
options are listed at the top of `fixtures/MakeFixtures.java`. Fixtures left out because
of EXIficient bugs are noted in the manifest.

## Interoperability policy

Interoperability with EXIficient matters more than strict compliance. Where EXIficient
differs from EXI 1.0, libexi follows EXIficient and documents the difference in the
code. Arc plans to raise these with the EXIficient developer, for an EXI 1.0 erratum or
a fix in EXIficient.

Differences followed (documented in `src/block.rs` and `src/canonical.rs`):

- In blocks of more than 100 values, EXIficient writes empty DEFLATE streams: one for
  the small-channel group when there are small channels, and one for each large channel.
  libexi writes them too and reads input with or without them.
- In schema-informed streams, canonical ones included, `xsi:nil="false"` is removed.
  Canonical EXI (C14N §4.4) says it should stay.
- Hour 24 is written as 00:00 of the next day, in every stream.
- `utcTime` applies to every date kind, with the midnight rule.

EXIficient results that libexi does **not** follow: `+14:00` handling under `utcTime`,
and `gDay`/`gMonth` with a time zone under `utcTime`.

EXIficient bugs (fixtures left out):

- nil-strict and types-strict with pre-compression or compression and blocks of 1 or 3
  values.
- zerobits-strict with compression and blocks of 110 values.

## Benchmarks

```sh
python3 benches/xmpp/gen_sessions.py          # regenerate the synthetic XMPP sessions
cargo bench --features infer                  # general harness
cargo bench --bench codecs --features infer,xhtml5,html5ever [-- --quick | -- stanzas]
benches/compare.sh [files...]                 # against xmllint, gzip, xz, zstd, EXIficient
```

`codecs` writes `target/bench/codecs.csv`, `stanzas.csv` and `pages.csv`. HTML pages come
from `target/html-corpus` or `EXI_HTML_CORPUS`. That corpus (31 pages) is not in the repo.

Findings so far (synthetic XMPP data):

- Per stanza, LZ4 adds little. Bit-packing is smaller schema-less and about equal with a
  strict schema.
- Per stanza, zstd with a trained dictionary is smallest: about 18–29 % of the text with
  strict pre-compression. XML text with the same dictionary comes close.
- Session-wide zlib gives 10–22 %.
- For whole documents, pre-compression + LZ4 (7.1 %) beats bit-packing (22.1 %).
- Resetting compression per stanza stops one stanza's content from changing another's
  size. Within a stanza, the compressed length still leaks content (CRIME-style).

## C ABI

- `EXI_ABI_MAJOR` 0, `EXI_ABI_MINOR` 11. Config structs grow only at the end.
- Status codes: `EXI_OK` 0, `EXI_END` 1, `EXI_NEED_INPUT` 2, negative codes one per
  `ErrorKind`.
- Strings are `exi_str { ptr, len }`, UTF-8. Strings libexi returns also end with a NUL
  that `len` does not count.
- `include/exi.h` is generated from `src/capi/mod.rs` by cbindgen and committed:
  `cbindgen --config cbindgen.toml --crate exi -o include/exi.h`. `tests/c_api.rs`
  fails when it is out of date. The `dom` modules and `impl Float` are marked
  `cbindgen:ignore`.

## Unfinished work

### In the code

- Start-tag reader mode (`ReaderConfig::start_tag`) returns `ErrorKind::Unsupported`.
- These options or streams return `ErrorKind::Unsupported`:
  - `schemaId ""` (built-in types only);
  - element fragment grammars;
  - `selfContained` in schema-informed streams that are not strict;
  - `datatypeRepresentationMap` and user meta-data in the options document.
- The `whiteSpace` facet of schema types is not applied (noted in `src/canonical.rs`).
- Lexical `xsi:type` values under strict are untested against EXIficient.
- The generator for `schemas/xhtml5-1.xsd` (`gen_xhtml5.py`) lives outside the repo.
  Add it, or record how the schema is maintained.
- No CI yet. The blueprint plans `cargo hack --each-feature`, `no_std` builds for
  `thumbv7em-none-eabihf` and `riscv32imac-unknown-none-elf`, `wasm32-unknown-unknown`,
  the shared library, and header checks.
- No fuzzing harness yet (the blueprint asks for 24 h clean on every target).
- The W3C EXI test suite (github.com/w3c/exi-testsuite) is not wired in yet.

### Performance

- XHTML5 decoding is about twice as slow as schema-less decoding.
- XHTML5 encoding is slower than HTML text + zstd.

### Benchmarks

- LZ4 with a dictionary is not measured.
- The XMPP sessions are synthetic. Real logged sessions (chat, IoT, ad hoc commands)
  would make the numbers trustworthy.

### Blueprint items not built yet (draft 14)

Draft 14 of the blueprint is the current design. It lives outside the repo and is never added to it.

- Global string pool for many threads (`SyncStringPool`). Open: append-only chunks that
  readers reach without a lock, with locks only for the index and counts. Needs a
  prototype.
- Name tables with or without reference counts: decide from benchmarks.
- Resolvers and a `GrammarCache`: a table of every grammar libexi can encode and
  decode, so a server can convert between grammars quickly.
- Built-in grammar for XSD documents (`bootstrap`), so schemas sent as EXI can be read.
- EXI Profile parameters.
- Datatype representation maps (`dtrm`, optional under EXI §10.2).
- Spec-notation event tracing (`trace`).
- XMPP support (blueprint Appendix E).
- A device crate for known schemas that builds for Cortex-M and RISC-V, with memory per
  connection measured. Devices are expected to have a heap (the MicroPython class) and
  load grammars from `.exig`.
- The browser build (`wasm32`, `xhtml5`).
- ABI 1.0 frozen after layer 2 is finished.

Open questions in draft 14:

- Should C programs be able to supply their own allocator (per reader like expat, or
  per process like libxml2)? It can be added after 1.0.
- Coarse error kinds with a message, or stable numbers for each specific fault?
- `.exig` (Appendix C): the namespace URI and W3C location; ordering by canonical order
  or by source-schema order; whether to carry representation maps; whether to define a
  text form.
- HTML (Appendix D): permanent profile identifiers in place of `urn:libexi:`; typed
  attributes in profile version 2 (`width`, `tabindex`); registering the `schema`
  parameter of `application/exi`.
- XMPP (Appendix E): proposing grammars instead of schemas in XEP-0322 configurations.

### Not designed yet

These have not been discussed in depth. Ask Arc before starting any of them.

- Python bindings, including `xml.parsers.exi`. Arc writes Python extension modules in
  Rust through the CPython C API, not PyO3.
- MicroPython bindings.
- Lua bindings.
- nodetree (layer 3) on top of layer 2. nodetree is a separate project.
- Possible later library: XSLT 4, XPath 4 and XQuery 4 on top of libexi.

## Outside this repository

- The blueprint documents (draft 14: core, stringtables, exig, html, xmpp, history,
  examples) are design documents. They are never added to the repo.
- The XEP-0322 revision is a separate `xep-0322.xml` (draft 0.7.0). Keep XEP work out of
  this repo and out of the blueprint.
- Arc is also writing a separate XEP to negotiate compression in XMPP.
