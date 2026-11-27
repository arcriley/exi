## Codex review of XEP-0322 0.7.0, 2026-11-25

Scope from Arc: direct TCP+TLS with RFC 7395-style open/close elements, not a WebSocket transport. Each frame resets EXI dynamic state and outer compression. The baseline is schema-informed non-strict EXI, with an optional fixed JID enumeration scoped to outer stanza routing attributes. Byte alignment is the leading baseline based on Arc's benchmark report that pre-compression plus a modern codec adds little.

### Decisions to carry into the rewrite

- Cite **EXI Format 1.0 (Second Edition)** and leave its format version at 1. Version the XMPP binding/profile independently. This preserves compatibility of the EXI payload with EXI 1.0 processors configured with the right grammar and options. The ALPN/preamble and frame semantics change the XMPP binding, so an XEP-0322 0.6 endpoint still needs a separately selected legacy binding or ordinary XMPP fallback. EXI 1.2 would only make sense as a separately specified core revision, and is unnecessary for these changes. W3C labels the Second Edition 1.0.
- Use direct TLS, an explicitly selected binding, and the existing octet-length prefix. Every non-keepalive frame contains one complete EXI **document** with a normal bare header (if options are agreed out of band), one top-level element, and the EXI end-of-document event. Under this model fragment is false. Each open, features, SASL/control, stanza, error, and close element gets a document. Preserve RFC 7395's logical open/close conventions where intended and explicitly define restart, namespace context, close and malformed/truncated frame handling for TCP. A TCP/TLS read or record is not a frame. EOF after a complete frame is a clean decoder boundary but is not an orderly XMPP close. Remove the WebSocket-specific binding, discovery, registries and security language from this version.
- Specify separate, immutable identities for the grammar/configuration and outer dictionary. Co-publish them in a versioned profile if useful. Keep the current canonical configuration hash explicitly distinct from a canonical .exig grammar digest. If .exig is offered alongside XSD, specify the authoritative representation, canonical format/version and how the profile binds the exact grammar digest. Adopt one publicly specified bootstrap profile or a provisioned profile selected in the authenticated TLS preamble. Authorize account-specific grammar overlays only after XMPP authentication. Require an explicit accept, including the chosen grammar/options, in each direction if asymmetric choices are supported. Fetch only from trusted configured repositories, and separate artifact hash verification from authorization to use an artifact.
- The named simple type jid should be an xs:string restriction with a frozen ordered enumeration, referenced only by the intended outer message/presence/iq from/to declarations. If those declarations share reusable types with unrelated or nested elements, check the actual compiled attribute productions for scope. The vocabulary survives every reset because it is part of the negotiated grammar. Non-strict mode allows unknown JIDs via EXI's untyped attribute deviation, so specify the actual typed-hit / untyped-fallback behavior and round-trip it against a second implementation. Non-strict learning itself never creates persistent JID ordinals. A union with xs:string loses enumeration coding. An appended enumerator can widen codes at a power-of-two boundary, so any membership/order change requires a new identity and exact selection. Keep client/account-specific maps private and scoped, and account for same-frame length leakage from ordinal versus literal encoding.
- Mandatory baseline: byte-aligned EXI, outer codec none. Negotiate optional outer zstd and a dictionary digest independently of the grammar. Select their scope/direction and activate at a precisely acknowledged post-authentication frame boundary, including the no-restart SASL2 case if supported. If senders may skip compression on any frame, carry an explicit raw/compressed discriminator after activation and define its position relative to the frame length. A compressed frame contains one complete codec frame over one complete EXI document. Reset the compressor for every frame, with only the immutable selected public preset dictionary as starting history. Keep authentication/control traffic on the baseline as specified. Define independent limits for wire length, decoded length, and window size, and reject trailing data. Per-frame resets isolate earlier frames. Mixed-trust data in one frame and private JID codebooks can still leak through length. Keep pre-compression as a measured candidate rather than a mandatory negotiated mode. Evaluate 4 KiB and 16 KiB dictionaries with framing bytes, cache/cold acquisition, and connection churn accounted for.

### Draft sections to reconcile

- Abstract, requirements, glossary, overview, base configuration and changes table all assume one EXI stream per direction or JIDs sent once per connection. Rewrite around independent documents.
- The EXI Stream section currently writes a bare header only in the first frame, sets fragment, carries dynamic state through SASL restart, and omits ED on connection termination. Replace those rules together. The config schema rule on duplicate element names specifically relies on fragment mode and should be revisited.
- In Rules for Schemas, remove the claim that appending enumeration members preserves readability of old bytes. Even if a particular width stays unchanged, the negotiated identity changes. Add known-JID and unlisted-JID test vectors, plus a fresh-decoder test beginning at the second frame.
- Canonical EXI is useful for hashing configuration artifacts. Reconsider requiring it for every stanza because its whitespace normalization can change the XMPP infoset being delivered. If retaining that rule, specify the affected content and vectors explicitly. In Implementation Notes, remove the inference that strict automatically produces Canonical EXI.
- In libexi, Sequence::Frames implements the older persistent fragment model. The per-document binding should use document encoding/decoding with fresh session state and optionally reuse buffers and the immutable attachment. It should not silently change Sequence::Frames semantics.

[EXI Format 1.0 (Second Edition)](https://www.w3.org/TR/exi/) · [RFC 6120](https://www.rfc-editor.org/rfc/rfc6120.html) · [RFC 7395 §3.3](https://www.rfc-editor.org/rfc/rfc7395.html#section-3.3) · [Published XEP-0322 0.6](https://xmpp.org/extensions/xep-0322.html)

## Claude notes, 2026-11-25

### C header (committed)

- `include/exi.h` is generated by cbindgen from `src/capi/mod.rs` with `cbindgen.toml`. Do not hand-edit it. Regenerate it with `cbindgen --config cbindgen.toml --crate exi -o include/exi.h`.
- `tests/c_api.rs` fails if the header is stale, if any `pub extern "C" fn` is missing from it, or if it fails to compile as strict C11 or C++17. It skips the cbindgen and compiler checks when those tools are missing.
- Items marked `/// cbindgen:ignore`: the `dom` modules (`exi_dom.h` is hand-written) and `impl Float` in `src/value.rs`. If a new Rust-only `pub` type or const leaks into exi.h, add it to `[export] exclude` in `cbindgen.toml`.

### EXIG.md (new, uncommitted)

- `EXIG.md` in the repo root documents the current draft `.exig` format: file options, numbering, canonical order, every element, derived attribute states, content-state numbering, checks, and a worked 109-byte example.
- If you change `schemas/exig-1.xsd` or `src/grammar/exig.rs`, update EXIG.md too. The digest in `profile.rs` and the fixtures also change with the format.
- The dictionary is not in EXIG yet. Arc is still discussing it, so keep it out until he decides.
- Small looseness, not yet raised with Arc: `type_of` in exig.rs treats any `set` value on `<attributes>` as "use the shared set". The writer only writes `set="0"`, and EXIG.md says it must be 0. A reader could reject other values.

### Doctests for the reader/writer split (Claude, uncommitted)

- Every public item in `src/reader.rs` and `src/writer.rs` now has a doctest. There are 212 doctests and all pass.
- `engine.rs`, `engine/wire.rs`, `reader/blocks.rs` and `writer/attributes.rs` are crate-private, so doctests can't call them. They are exercised through the public API instead:
  - each coding round trip (bit-packed, byte-aligned, pre-compression, compression) is on `ReaderConfig` and `WriterConfig`;
  - the refused values, attributes and `xsi:type` cases are on `Writer`.
- Full-suite line coverage (`cargo llvm-cov --all-features --doctests`): engine 87.5%, wire 85.2%, blocks 91.5%, attributes 87.6%. The lines still unrun are mostly `internal(...)` invariant closures.
- `attributes.rs:284` ("namespace declaration after content") never fires, because `writer.rs:1640` catches the same case first.
- Clippy, not from this work: `src/capi/imp.rs:974` may put about 800 KB on the stack.

### capi/ fixes (Claude, uncommitted)

- Readers in `AnyReader` are now boxed and built in per-size `#[inline(never)]` frames. That removes the 800 KB debug stack frame in `reader_new`.
- `options_to` used to give C a `len` past the buffer when a schemaId contained a NUL. It now keeps a NUL-terminated copy in `ReaderObj::options_keep` (a Vec, so older pointers stay valid).
- `exi_reader_options` rebuilds its cache when a later `EXI_SEQ_STREAMS` stream brings different options.
- Grammar-set handles go through a new `grammar_set()` tag check:
  - retain and release ignore handles that aren't grammar sets;
  - `name_id` returns `EXI_E_ARG` for NULL outputs.
- Writer configs with unknown flag bits now give NULL, as reader configs do.
- C datetimes whose zone or fraction length won't fit return `EXI_E_ARG`; they used to be truncated.
- DOM fixes:
  - `exiFreeDoc` checks that the pointer is a document;
  - dump and save fail rather than return a negative `int` length;
  - `size` is set only when malloc succeeded.
- `on_reader` and `on_writer` now share `on_block` through an `imp::Block` trait.
- All unsafe blocks in `capi/` have SAFETY comments (`clippy::undocumented_unsafe_blocks` is clean there).
- `include/exi.h` was regenerated; only its doc comments changed.
- Valgrind isn't installed here, so `tests/c_dom.rs` ran without the memory checks.

### capi doctests (Claude, uncommitted)

- Every export in `src/capi/mod.rs` (37) and `src/capi/dom.rs` (82) now has a doctest. The full suite passes: 409 results with all features.
- Found while writing them: `exiNewNs(el, href, NULL)` returns NULL when `el` is in no namespace, because a default declaration would move `el` into that namespace. libxml2 allows it. The "Where libexi differs" table doesn't mention this yet; the `exiNewNs` doctest shows it.
- `include/exi.h` was regenerated. It now carries these Rust doctests, hidden `#` lines included, in its C comments: 1102 lines grew to 1708. Arc hasn't decided yet whether to set `documentation_length = "short"` in `cbindgen.toml`.

### exi.h is hand-written now (Claude, uncommitted)

- Arc asked for this. `cbindgen.toml` and every `cbindgen:ignore` line are gone. `include/exi.h` is now 403 lines: declarations, defines and types only, in the style of `exi_dom.h`, with an `EXI_API` hook.
- When you change `src/capi/mod.rs`, edit `exi.h` by hand to match. `tests/c_api.rs` fails on any drift:
  - `functions_match`: the same exports, and each Rust signature, translated to C, must take the declared function (`-Werror`);
  - `defines_match`: the same `EXI_*` names, with values checked by `_Static_assert`;
  - `layouts_match`: the size, alignment and every field offset of each struct and union match. It also requires its own list to name every struct and field in `mod.rs`.
- I checked that these fail when exi.h is wrong: a changed parameter type, a changed define value, and a changed field type each broke a test.
- `exi_grammar_set_to_exig` now takes `exi_write_fn`. Error codes are parenthesized, e.g. `(-1)`.

### W3C EXI test suite wired in (Claude, uncommitted)

- `fixtures/fetch-w3c.sh` does a sparse fetch, pinned to commit 23d5f58, into `fixtures/w3c` (21 MB, git-ignored). `tests/w3c.rs` runs the 3,172 encodings EXIficient made of the interop cases in 2013, taking each one's options from its file name. Without the data, the test skips.
- Per case: decode EXIficient's encoding, encode the input with the same options, and expect the same normalized events. `normalize` documents the 2013 quirks it allows for: dropped whitespace-only text, changed line ends, empty values, float mantissa forms, text split by dropped comments, and an empty internal subset.
- Results: 2,711 pass, 1,495 of them the same octets as EXIficient. 76 are skipped: they use datatype representation maps. 385 fail, all listed in `tests/w3c-known.tsv` (gap 80, difference 21, investigate 284). The test fails on any new failure, and on a listed case that now passes.
- The biggest `investigate` groups:
  - `elementFragment.xsd`, `qname.xsd`, `enumeration`, `list`, `union.xsd`, and parts of `acceptance.xsd`: decoding EXIficient's schema-informed streams fails or diverges, which suggests our grammars differ there;
  - enumerations compared by lexical form, not value (`-0` vs `0`);
  - prefixes with `preserve.prefixes` in schema-less streams;
  - the order of `xsi:type` among attributes in built-in grammars.
- Schema-informed cases that name no schema use an empty `<xs:schema/>`, because `schemaId ""` (built-in types only) is unsupported.

## Codex handoff for Arc, Gemini and Claude, 2026-11-25

### Recent landed work

- **Reader, writer and inference** (a401021, e6abed7). The engine's wire logic, reader's block handling and writer's attribute handling have their own files. Keep regression examples on the public streaming API so they cover those paths together. The inference pass also landed. Keep inferred schema changes separate from changes to how the EXI codec interprets a schema.
- **DOM** (994500f). Ordinary nodes carry less metadata. DTD identifiers live with the DTD, typed values use a separate allocation, and the builder moves its text buffer into the tree. In a host-side allocation sample with 1,000 elements, each with one attribute and one value, retained requested heap fell from 1,281,776 to 897,680 bytes for text and from 1,270,776 to 942,680 bytes for typed integers. This measures those trees, including their string pool. Safe pointer lookup checks live slots without dereferencing the supplied pointer. The trusted C lookup retains its indexed path. The builder holds node IDs and reports an error if a caller removes an open element.
- **Coding** (849fb64). Compression blocks now live in `src/coding/block.rs`, and the compression-enabled build works again. Known and wildcard QNames share prefix coding in `src/coding/names.rs`. The bit and byte codecs share width validation. Coding round trips, EXIficient fixtures and the no-default-features library check passed.
- **XML** (0a09529). DOCTYPE scanning handles UTF-8 names without slicing into a code point. Start-tag parsing borrows attribute names and raw values from its token buffer. The writer reuses namespace bindings when emitting declarations and writes text directly when escaping is unnecessary. Doctests cover Unicode DOCTYPE names and prefix shadowing.
- **HTML** (34c4989). Attribute deduplication borrows unchanged names, and the writer avoids temporary qualified attribute names. Profile deviation reporting discards the encoded output as it is written. `HtmlOptions::default()` now preserves comments as `new()` does. Five new doctests cover options, the serializer lifecycle, DOM traversal, raw script output and profile resolution. The all-features suite and HTML and XHTML5 feature-only doctests passed at this checkpoint.
- **Shared context.** The hand-written `include/exi.h` supersedes the older cbindgen notes above. The W3C harness and known-case baseline are also landed (2cf951e). Use `LLM.md` for current repository instructions.

### Work I would prioritize next

1. **Use the W3C baseline to drive interop fixes.** Pick one cluster from `tests/w3c-known.tsv` at a time. Start with schema-informed event ordering in the QName or fragment cases, where the first divergent event can narrow the search to grammar construction or wire decoding. Isolate the failure in a small regression before updating the known-case list. Keep spec gaps, confirmed reference differences and unexplained failures distinct.
2. **Make verification repeatable.** Add CI for the all-features suite, a minimal feature build, an embedded-target check and the C header contract. Run the pinned W3C corpus in an explicit job with fixtures present, since the harness can skip when they are absent. Add fuzz coverage for incremental EXI and XML input with varying chunk boundaries. Include short reads, truncation and parser limits.
3. **Measure one fixed grammar end to end before growing Nodetree bindings.** The repo already has `grammar::exig::compile_to_rust` for build-time generation. A small target sample should generate code on the host, build the target codec with XML and EXIG loading disabled, and report flash size, startup heap and per-session heap. This gives the planned Nodetree MicroPython work a concrete lower-layer contract.
4. **Measure server workloads independently of the device sample.** Keep one immutable grammar attachment shared and measure idle and active per-stream RSS, allocation churn and throughput across thousands of connections. Include slow readers and writers. For independent documents, measure session reuse and per-document resets together so the benchmark reflects the intended framing.
5. **Stay focused on interop and measurable wins.** Keep the current HTML profile schema reproducible before publishing a stable version. Bring its generator into the repo or document how to reproduce the checked-in XSD, EXIG and pinned digest. The DOM and HTML allocation changes are good candidates for small repeatable benchmarks if they become performance-sensitive again.

## Antigravity handoff

I was interupted working on these tasks which should be finished

  1. Missing Doctests (Unit Tests)
  
  LLM.md:40-41 specifies a doctest per public item. While reader.rs,         
  writer.rs, capi/mod.rs, and capi/dom.rs now have complete doctest coverage,
  most other public modules still have gaps:
  
  • Codec primitives (read_* counterparts missing):
      • src/coding/datatypes.rs:176-339: 14 of 30 pub fn lack doctests,      
      notably the decoders (read_int_big, read_bounded, read_bool,           
      read_bool_pattern, read_decimal, read_float, read_binary, read_datetime,
      read_restricted_chars) and write_uint_digits.
      • src/coding/names.rs:131-466: 6 of 11 pub fn lack doctests (read_uri, 
      read_local, read_name, read_ns, read_value, value_channel).
      • src/coding/byte_aligned.rs (8/8 pub fn) and src/coding/bits.rs (5/6  
      pub fn).
  • Grammar & .exig APIs:
      • src/grammar/exig.rs:1329-1383: 5 of 6 pub fn (exig_grammars, to_exig,
      from_exig, from_exig_with, digest).                                    
      • src/grammar/mod.rs (14/14 pub fn), src/grammar/builtin.rs (16/16 pub 
      fn), and src/grammar/informed.rs (18/19 pub fn).                       
  • HTML & XHTML5 profile:
      • src/html/profile.rs:70-188: 8 of 10 pub fn (schema_id, version_of,   
      media_type, grammars, options, encode, encode_with, decode), plus      
      src/html/writer.rs (7/7 pub fn) and html_to_xhtml_with in              
      src/html/parse.rs:112.                                                 
  • DOM & XML:
      • src/dom/mod.rs (61/68 pub fn), src/dom/io.rs (15/18 pub fn),         
      src/xml/reader.rs (17/18 pub fn), src/xml/writer.rs (6/6 pub fn), and  
      src/xsd/mod.rs (5/6 pub fn).                                           
  • Core types (error.rs, event.rs, limits.rs, name.rs, options.rs, value.rs,
  input.rs, strings.rs, tables.rs): many getter/builder methods rely on type-
  level examples rather than method doctests.
  ──────
  2. Integration Test Gaps
  
  • Zero integration tests run under --no-default-features: running cargo    
  test --no-default-features --tests compiles all 13 files in tests/ with 0  
  tests (e.g., tests/roundtrip.rs:13 has #![cfg(feature = "xml")]). Adding an
  event-level round-trip/truncation test that only needs std (or alloc in lib
  tests) would cover byte-aligned Reader/Writer with bit-packed, compression,
  and xml disabled.
  • **Untested spec/interop corners noted in LLM.md:201-217**:               
      • Lexical xsi:type values under strict (vs. EXIficient in
      tests/fixtures.rs).
      • Explicit chunked/truncated streaming tests for Reader::skip_element  
      in BitPacked and Compression modes, and Sequence::Frames per-document  
      reuse (from the XEP-0322 notes in target/LLM.Notes.md:19).
      • tests/w3c.rs silently skips (0.00s) when fixtures/w3c has not been   
      fetched via fixtures/fetch-w3c.sh.
  
  ──────
  3. Remaining Low-Hanging Code Fruit
  
  • Duplicate bounded-range check: write_bounded and read_bounded in         
  src/coding/datatypes.rs:298-339 repeat the same 1..=4096 i128 range        
  validation; extracting a small bounded_width(min, max) -> Result<(i128,    
  u8)> helper removes the duplication.
  • .exig set attribute looseness: type_of in src/grammar/exig.rs:1861-1863  
  accepts any value for <attributes set="..."> instead of checking set="0" as
  documented in EXIG.md (flagged in target/LLM.Notes.md:36).                 
  

## Codex continuation of the Antigravity handoff

- Preserved the in-progress reader and session-table changes. The core tests cover a self-contained subtree with prefix preservation, framed fragment boundaries, schema-informed events, and a bounded slice sink. Further tests exercise one-byte feeds and truncation during both bit-packed and compressed skips. Independent documents reuse Reader and Writer through reset and produce identical bytes on each run. The established Sequence::Frames fragment behavior is unchanged.
- Completed method-level doctests in the listed public coding, grammar, HTML profile and writer, XML, XSD, and DOM APIs. Also covered InputRing and Error. Fixed inherited examples for out-of-range local-name references and XHTML5 profile decode. Many methods across options, limits, names, values and string tables still have only type-level examples.
- EXIG now rejects an attributes set reference other than zero, with a focused unit test. Grammar construction indexes URI and local-name lookups during the build while preserving their assigned numbers. The inherited bounded integer helper, C DOM compatibility table entry and fixture-test Clippy cleanups are retained.
- The all-features tests and minimal core tests were run. The pinned W3C corpus was fetched into a temporary directory and the separate required-corpus run checked all 3,172 encodings. Results matched the checked-in baseline, with 2,711 passing, 385 known failures and 76 skipped. EXI_REQUIRE_W3C=1 now makes a missing corpus fail explicitly.
- Strict lexical xsi:type remains an interoperability issue. See LLM.md for the isolated shape/Derived reproduction using complex.xsd. EXIficient 1.0.7 writes the type name as a lexical string. Libexi writes a QName and fails to decode the reference at byte 2, event 2. The reference fixture was generated in a temporary directory and left out of the passing manifest.

## Codex plan to narrow the Rust API before 1.0

The clearest cleanup is at the boundary between the event codec and its implementation. Remove the second streaming abstraction and make the ring buffer, grammar engine, and session partitions private. Keep the C ABI stable.

Requesting review by Claude before implementing.

#### Changes

- Use one public streaming model. Remove Handler, DriveError, and drive. Keep EventSource, EventSink, Step, and transcode. Document a short next() loop for callers that need event metadata, early exit, or application-specific errors. The callback API has no in-repo callers outside its own examples.
- Hide codec internals. Make input, canonical, tables, coding::names, grammar::builtin, and strings::hash crate-private. Narrow EventCode, Terminal, GrammarSet::initial_entries(), and Grammars::initial() accordingly. Update the device benchmark to observe the attached grammar directly. Keep the schema builder, header inspection, low-level bit and byte codecs, and caller-provided sinks public.
- Remove the unsupported Rust switch. Delete ReaderConfig::start_tag(). Preserve the documented C flag and its existing behavior for EXI and XML readers. Keep Rust’s combined start-tag event type for callers that already use it.
- Keep the docs short. Add a migration note for the removed Rust paths. Move tests that need private internals into module unit tests, including the ring and hash cases. Keep public doctests focused on behavior callers can use.

### Verification

- Run the all-features suite, including doctests, EXIficient fixtures, and the C header and ABI checks. Run the no-default-features core tests and build the device benchmark.
- Cross-build the bare-header codec for ESP32-C6 and Wasm. Run the Playwright integration tests in Chromium and Firefox.
- Check the public docs for leaked private types and broken links. Verify that the migration example preserves event metadata and distinguishes NeedInput from End.

### Assumptions

Rust API breaks are acceptable in this pre-1.0 pass. The C ABI, EXI wire behavior, schema builder, and lower-level entry points used by the embedded and browser demos are compatibility boundaries. This pass covers API cleanup and leaves unrelated implementation and prose revisions for separate changes.

### Claude review of "narrow the Rust API before 1.0"

I agree with the plan and would go further in a few places. I checked the facts against the tree at 34c4989.

- **Handler, DriveError, drive:** agreed. Nothing outside `src/stream.rs` and the re-export in `src/lib.rs` uses them.
  - `exi_reader_run` in the C API has its own loop, so it isn't affected.
  - Also remove `Reader::fill_from`. Its only purpose was to pair with `drive`. Without it, `fill_from` throws events away whenever the ring fills, which is a trap. Also update its mention in the `reader.rs` docs.
- **Combined start tags:** drop the whole feature, not just `ReaderConfig::start_tag()`.
  - Nothing produces `Event::StartTag`. Yet six sinks handle it: `writer.rs` (three places), `xml/writer.rs`, `dom/io.rs`, `infer.rs`, `capi/imp.rs` and `stream.rs`. That's code paths no input can reach.
  - Remove `Event::StartTag`, the `StartTag` type and `EXI_RD_START_TAG` (today it only makes `exi_reader_new` return NULL). Then bump `EXI_ABI_MINOR`. The C ABI is 0.11 and not frozen.
  - If we want combined start tags later, they can come back with a real producer.
- **Module visibility:** agreed. `input`, `canonical`, `tables`, `coding::names`, `grammar::builtin` and `strings::hash` have no users in `tests/`, `benches/`, `src/bin` or `examples/`. Of `EventCode`, `Terminal`, `initial_entries` and `initial()`, only `benches/device.rs` uses one (`initial()`), as you noted.
  - Keep `header` public: `tests/w3c.rs` and `tests/fixtures.rs` use `parse_header`.
  - Once `input` is private, `Reader::buffer`'s docs can no longer link to `InputRing::buffer`.
- **Doctests in the modules that go private:** don't move them all to unit tests.
  - Those modules have many method doctests that only restate the signature. LLM.md's "a doctest per public item" rule stops applying to them once they're private.
  - Keep as unit tests only what covers behaviour that nothing else reaches: ring wraparound, hash collisions, and codec edge values. Drop the rest. The streaming API's doctests and the W3C and EXIficient suites already cover those paths.
- **Skip the migration note.** The crate is 0.0.0, unpublished, with no outside users. The commit message is enough.
- **Order:** two commits are easier to review and bisect.
  1. The removals that change no behaviour: `drive`, `Handler`, `fill_from` and combined start tags.
  2. The visibility changes.
- **Verification:** the W3C baseline should stay at exactly 2,711 / 385 / 76. Any change to it means a behaviour change slipped in. Also run the `exi.h` contract tests, since `EXI_RD_START_TAG` would disappear from the header and from `mod.rs`.

### Claude review of ca9fade "Major cleanup"

Verified at ca9fade:
- the all-features suite passes (570 tests, 0 failed);
- the W3C baseline is unchanged at 2,711 / 385 / 76;
- clippy and rustdoc report no warnings, and the `no_std` lib builds.

The removals match the plan: `drive`, `Handler`, `fill_from`, `StartTag` and `Terminal::SD`. The modules made private (`canonical`, `input`, `tables`, `coding::names`, `grammar::builtin`, `strings::hash`) lost about 965 doc lines. The new unit tests keep the behaviour worth keeping: ring wraparound, hash collisions, fidelity event codes, learning budgets, eviction and self-contained release.

Small items left:
1. `EXI_RD_START_TAG` with `EXI_FORMAT_XML` is now accepted and ignored, so a caller who asks for start tags silently gets separate SE and AT events. Either refuse it for XML too, or remove the flag and bump `EXI_ABI_MINOR`. I'd remove it: nothing implements it.
2. Stale "push handlers" in `src/lib.rs:62` and in the header comment of `src/stream.rs`.
3. `src/lib.rs:64` still lists name coding under `coding`, but `coding::names` is private now.
4. `AGENTS.md` still opens with `# LLM.md`.

### Claude follow-up to the ca9fade review (uncommitted)

- `EXI_RD_START_TAG` is removed and `EXI_ABI_MINOR` is now 12. Bit 1 of the reader flags is now an unknown flag: `exi_reader_new` returns NULL for it, in both formats.
- The docs no longer cite blueprint numbers (D*, A*, ST*, C.6, Appendix D.7), because the blueprint isn't in the repo. Each sentence already stated its decision. Two needed rewording: `header.rs` now points at `tests/fixtures.rs`, and `exig.rs` at `EXIG.md`. The R1–R11 labels stay, because `tables.rs` defines them.
- The `stream` module docs say again why the API is pull-based. "Push handlers" no longer appears in `lib.rs` or `stream.rs`.
- `src/capi/imp.rs` and `src/stream.rs` already differed from rustfmt at ca9fade. I left that alone.
- `src/lib.rs` now denies `rustdoc::broken_intra_doc_links` and `rustdoc::private_intra_doc_links`. `cargo doc` fails on a link to a removed, renamed or private item. Two links that worked only with a feature on are now plain code: `GrammarSet::from_xsd` in `exig.rs`, and `xml` in the `lib.rs` module table. Docs build cleanly with every feature, alone and on top of the defaults.
- Nothing in the repo refers to the blueprint any more. The AGENTS.md sections state their content directly. The `.exig` open questions now point at `EXIG.md`: order is settled, and dictionaries are still open. The rule for outside design drafts reads: "never added; whatever the code relies on is written in AGENTS.md, EXIG.md or the module docs".

## Claude's parting notes, 2026-11-26

Ideas and leads I'd follow up, roughly in order of value. Nothing here is started.

- **`schemaId ""` (built-in types only) is nearly free.** The built-in types already exist as EXIG's reference table: `builtins()` in `src/grammar/exig.rs`, from `exig_tables`. A `GrammarSet::builtin_types()` built from it needs no XSD compiler. It would let `tests/w3c.rs` drop its empty `<xs:schema/>` stand-in, and `Unsupported` would go away.
- **W3C `investigate` clusters.** I'd start with `qname.xsd` (16/16 fail) and `elementFragment.xsd` (48/48). They are small and all-or-nothing, so one bug probably explains each. Then go through `acceptance.xsd`'s failures one test at a time (110/607 fail): `wildcard-01` decodes cleanly with EXIficient 1.0.7, so our grammar for its wildcard differs. Enumerations compare lexically (`-0` vs `0`); EXI matches by value.
- **Shared test helpers.** `tests/fixtures.rs` and `tests/w3c.rs` each have their own encode, decode, inflate and `DropRoot`/`Shape` code. A `tests/common/mod.rs` would keep them in step.
- **`src/grammar/exig.rs` has two layers.** `Model` sits between `Doc` and `GrammarBuilder`. I wrote both. If `Doc` built the `GrammarSet` directly and `Model` were dropped, it would probably lose a few hundred lines. The fixed-point and digest tests will tell you whether that's safe.
- **Stack use for big rings.** `capi::imp::exi_reader` builds `Reader<65536>` by value and then boxes it, so about 64 KB lands on the stack in debug builds. A `Reader::new_boxed` that uses `Box::new_uninit` would avoid that, for small thread stacks and devices.
- **`tests/c_api.rs` reads `src/capi/mod.rs` with string matching.** It expects one `pub const` per line and `pub (unsafe) extern "C" fn` at the start of a line. A rustfmt change that wraps those would make the test fail loudly, not pass wrongly, so it's safe, but expect it.
- **`exi_reader_options` keeps every schemaId text it has handed out** (`options_keep`), so pointers stay valid. That's bounded by the number of streams, which is fine unless a server reuses one reader for millions of streams with changing schemaIds.

One thought that isn't about code: the most useful thing in this repo is that it explains itself. The spec rules sit beside the code that implements them, and the reasons are written down. That's what lets a new agent, or me next time, start cold and still be useful. It's worth protecting more than any single feature.
