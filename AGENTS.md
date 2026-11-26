# AGENTS.md

This file is for LLM agents working on this repository. It covers how Arc
works, how the code is laid out, how to build and test it, the decisions already made, and what is unfinished. This was added by Claude Code with advice for itself and other LLMs for efficiency.

## The project

- `exi` is a W3C EXI 1.0 implementation in Rust with a C ABI.
- Hosted publicly at https://github.com/arcriley/exi
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
| `src/coding/block.rs` | Pre-compression and compression: blocks, channels, DEFLATE (feature `compression`) |
| `src/header.rs`, `src/options.rs` | Header, cookie and the options document |
| `src/engine.rs`, `src/engine/wire.rs` | Shared grammar state and event/value coding |
| `src/reader.rs`, `src/reader/blocks.rs` | Layer 1 reader, frame boundaries and compressed blocks |
| `src/writer.rs`, `src/writer/attributes.rs` | Layer 1 writer, canonical writing and pending start tags |
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

Defaults: `std`, `xml`, `bit-packed`, `compression`, `exig`.

| Feature | Adds |
|---|---|
| `std` | I/O adapters and conveniences |
| `xml` | XML text reader and writer |
| `bit-packed` | Bit-packed bodies. Headers are always bit-packed |
| `compression` | Pre-compression and compression; DEFLATE through flate2 (needs `std`) |
| `exig` | Grammar file I/O and host-side Rust generation |
| `xsd` | XSD compiler |
| `infer` | Schema inference (implies `xsd`) |
| `dom` | Layer 2 tree |
| `html`, `html5ever`, `xhtml5` | HTML input, HTML text parsing, XHTML5 profile grammars |
| `capi` | C ABI (implies `std`, `xml`, `dom`, `exig`) |

## Build and test

```sh
cargo test                      # default features: 167 tests
cargo test --all-features       # everything: 240 tests
cargo build --lib --no-default-features   # the no_std + alloc engine
cargo doc --all-features --no-deps
```

For devices with pre-agreed options, build with `--no-default-features --features
bare-header-only` and explicitly select byte alignment and a bare header. The
reader also needs the options out of band. Default features and `exig` enable
`header-options`, which takes precedence when features are combined.

```sh
rustup target add --toolchain nightly riscv32imac-unknown-none-elf wasm32-unknown-unknown
cargo +nightly build --release --lib --no-default-features --features bare-header-only --target riscv32imac-unknown-none-elf
cargo +nightly build --release --lib --no-default-features --features bare-header-only --target wasm32-unknown-unknown
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

`tests/w3c.rs` runs the interoperability cases of the W3C EXI test suite against the
encodings EXIficient made of them in 2013. `fixtures/fetch-w3c.sh` puts both in
`fixtures/w3c` (git-ignored, 21 MB); without them the test is skipped. Set `EXI_REQUIRE_W3C=1` to fail when
fixtures are missing. Each encoding's
options come from its file name. The test decodes EXIficient's encoding, encodes the
input with the same options, and expects the same events from both. It compares events
after allowing for quirks of the 2013 run, which the test's `normalize` documents.
Failures listed in `tests/w3c-known.tsv` are expected. Anything else fails the test,
including a listed case that now passes, so the list can only shrink.

## Interoperability policy

Interoperability with EXIficient matters more than strict compliance. Where EXIficient
differs from EXI 1.0, libexi follows EXIficient and documents the difference in the
code. Arc plans to raise these with the EXIficient developer, for an EXI 1.0 erratum or
a fix in EXIficient.

Differences followed (documented in `src/coding/block.rs` and `src/canonical.rs`):

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

- `EXI_ABI_MAJOR` 0, `EXI_ABI_MINOR` 12. Config structs grow only at the end.
- Status codes: `EXI_OK` 0, `EXI_END` 1, `EXI_NEED_INPUT` 2, negative codes one per
  `ErrorKind`.
- Strings are `exi_str { ptr, len }`, UTF-8. Strings libexi returns also end with a NUL
  that `len` does not count.
- `include/exi.h` is written by hand: declarations, defines and types only, no
  generator. Change it with `src/capi/mod.rs`. `tests/c_api.rs` checks the function
  types, `#define` values, and struct sizes and offsets against the Rust, and compiles
  it as C11 and C++17 with `exi_dom.h`.

## Unfinished work

### In the code

- These options or streams return `ErrorKind::Unsupported`:
  - `schemaId ""` (built-in types only);
  - element fragment grammars;
  - `selfContained` in schema-informed streams that are not strict;
  - `datatypeRepresentationMap` and user meta-data in the options document.
- The `whiteSpace` facet of schema types is not applied (noted in `src/canonical.rs`).
- Strict lexical `xsi:type` differs from EXIficient 1.0.7. A small
  reproduction using `fixtures/input/complex.xsd` and `byte,strict,lexical`
  is `<shape xmlns="urn:c" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="Derived" kind="mid"><name>one</name><extra>9</extra></shape>`.
  EXIficient emits the lexical string `Derived`, while libexi emits
  a QName and cannot decode the reference stream.
- The generator for `schemas/xhtml5-1.xsd` (`gen_xhtml5.py`) lives outside the repo.
  Add it, or record how the schema is maintained.
- No CI yet. Planned: `cargo hack --each-feature`, `no_std` builds for
  `thumbv7em-none-eabihf` and `riscv32imac-unknown-none-elf`, `wasm32-unknown-unknown`,
  the shared library, and header checks.
- No fuzzing harness yet. The goal is 24 hours without a failure on every target.
- The W3C EXI test suite: `tests/w3c.rs` runs 3,172 interoperability encodings. 385 of
  them fail, and `tests/w3c-known.tsv` lists each with its reason; 284 of those are
  marked `investigate`.

### Performance

- XHTML5 decoding is about twice as slow as schema-less decoding.
- XHTML5 encoding is slower than HTML text + zstd.

### Benchmarks

- LZ4 with a dictionary is not measured.
- The XMPP sessions are synthetic. Real logged sessions (chat, IoT, ad hoc commands)
  would make the numbers trustworthy.

### Designed, not built yet

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
- XMPP support: the EXI binding of XEP-0322, which Arc is revising outside this repo
  (see "Outside this repository").
- A device crate for known schemas that builds for Cortex-M and RISC-V, with memory per
  connection measured. Devices are expected to have a heap (the MicroPython class) and
  load grammars from `.exig`.
- The browser build (`wasm32`, `xhtml5`).
- ABI 1.0 frozen after layer 2 is finished.

Open questions:

- Should C programs be able to supply their own allocator (per reader like expat, or
  per process like libxml2)? It can be added after 1.0.
- Coarse error kinds with a message, or stable numbers for each specific fault?
- `.exig` (`EXIG.md`): the namespace URI (`urn:exig:draft-1` is a placeholder) and W3C
  location; whether to carry datatype representation maps; whether `.exig` files should
  carry compression dictionaries, which is still being discussed. Order is settled: the
  canonical order `EXIG.md` describes. The XML form `exi exig` prints serves as the text
  form.
- HTML profile: permanent profile identifiers in place of `urn:libexi:`; typed
  attributes in profile version 2 (`width`, `tabindex`); registering the `schema`
  parameter of `application/exi`.
- XMPP: proposing grammars instead of schemas in XEP-0322 configurations.

### Not designed yet

These have not been discussed in depth. Ask Arc before starting any of them.

- Python bindings, including `xml.parsers.exi`. Arc writes Python extension modules in
  Rust through the CPython C API, not PyO3.
- MicroPython bindings.
- Lua bindings.
- nodetree (layer 3) on top of layer 2. nodetree is a separate project.
- Possible later library: XSLT 4, XPath 4 and XQuery 4 on top of libexi.

## Outside this repository

- Arc's design drafts are never added to the repo, and nothing here depends on them.
  Whatever the code relies on is written in this file, `EXIG.md` or the module docs.
- The XEP-0322 revision is a separate `xep-0322.xml` (draft 0.7.0). Keep XEP work out of
  this repo.
- Arc is also writing a separate XEP to negotiate compression in XMPP.
