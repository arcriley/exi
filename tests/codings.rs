// tests/codings.rs
//
// EXI coding round-trip tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Every coding against itself: bit-packed, byte-aligned, pre-compression and compression
//! at many block sizes give back the same events, whatever pieces the input arrives in.
//! `tests/fixtures.rs` compares the same codings with EXIficient.
#![cfg(all(feature = "xml", feature = "bit-packed", feature = "compression"))]

use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{
    Coding, ErrorKind, Header, Limits, Options, OwnedEvent, Preserve, Reader, ReaderConfig, Result,
    Sequence, Step, Writer, WriterConfig,
};

const CODINGS: [Coding; 4] = [
    Coding::BitPacked,
    Coding::ByteAligned,
    Coding::PreCompression,
    Coding::Compression,
];

fn events_of_xml(g: &Grammars<'_>, xml: &[u8], comments: bool) -> Vec<OwnedEvent> {
    let mut r: XmlReader = XmlReader::new(
        XmlConfig::new()
            .report_comments(comments)
            .report_pis(comments),
        g,
    );
    let mut out = Vec::new();
    let mut rest = xml;
    loop {
        rest = &rest[r.feed(rest)..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

fn encode(g: &Grammars<'_>, opts: &Options, evs: &[OwnedEvent]) -> Result<Vec<u8>> {
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), g).header(Header::WithOptions { cookie: false }),
    )?;
    for e in evs {
        exi::EventSink::event(&mut w, &e.as_event())?;
    }
    Ok(w.into_sink())
}

/// Decodes `exi`, giving the reader `chunk` octets at a time.
fn decode<const N: usize>(
    g: &Grammars<'_>,
    exi: &[u8],
    chunk: usize,
    limits: Limits,
    seq: Sequence,
) -> Result<Vec<OwnedEvent>> {
    let mut r: Reader<'_, N> = Reader::new(ReaderConfig::new(g).limits(limits).sequence(seq))?;
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        if at < exi.len() {
            let end = at.saturating_add(chunk).min(exi.len());
            at += r.feed(&exi[at..end]);
        }
        if at == exi.len() {
            r.end_input();
        }
        loop {
            match r.next()? {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return Ok(out),
            }
        }
    }
}

struct Doc {
    name: &'static str,
    schema: Option<&'static str>,
    preserve: Preserve,
}

fn docs() -> Vec<Doc> {
    let p = Preserve::empty();
    let mut v = vec![
        Doc {
            name: "channels.xml",
            schema: None,
            preserve: p,
        },
        Doc {
            name: "values.xml",
            schema: None,
            preserve: p,
        },
        Doc {
            name: "hits.xml",
            schema: None,
            preserve: p,
        },
        Doc {
            name: "xsi-type.xml",
            schema: None,
            preserve: p,
        },
        Doc {
            name: "xsi-nil.xml",
            schema: None,
            preserve: p,
        },
        Doc {
            name: "prefixes.xml",
            schema: None,
            preserve: Preserve::PREFIXES,
        },
        Doc {
            name: "comments-pis.xml",
            schema: None,
            preserve: Preserve::COMMENTS | Preserve::PIS,
        },
        Doc {
            name: "infer/logo.svg",
            schema: None,
            preserve: Preserve::PREFIXES | Preserve::COMMENTS,
        },
    ];
    if cfg!(feature = "xsd") {
        v.push(Doc {
            name: "nil.xml",
            schema: Some("nil.xsd"),
            preserve: p,
        });
        v.push(Doc {
            name: "types.xml",
            schema: Some("types.xsd"),
            preserve: p,
        });
        v.push(Doc {
            name: "zero.xml",
            schema: Some("zero.xsd"),
            preserve: p,
        });
        v.push(Doc {
            name: "zerobits.xml",
            schema: Some("zerobits.xsd"),
            preserve: p,
        });
        v.push(Doc {
            name: "reading.xml",
            schema: Some("reading.xsd"),
            preserve: p,
        });
    }
    v
}

fn grammars(schema: Option<&str>) -> GrammarSet {
    match schema {
        None => GrammarSet::schemaless(),
        #[cfg(feature = "xsd")]
        Some(x) => {
            GrammarSet::from_xsd(&std::fs::read(format!("fixtures/input/{x}")).unwrap()).unwrap()
        }
        #[cfg(not(feature = "xsd"))]
        Some(_) => unreachable!(),
    }
}

#[test]
fn every_coding_gives_back_the_same_events() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let mut checked = 0;
    for d in docs() {
        let set = grammars(d.schema);
        let g = set.attach(&strings).unwrap();
        let xml = std::fs::read(format!("fixtures/input/{}", d.name)).unwrap();
        let comments = d.preserve.contains(Preserve::COMMENTS);
        let input = events_of_xml(&g, &xml, comments);
        let strict = d.schema.is_some();
        // The events a stream carries: what byte-aligned coding gives back.
        let byte = Options::builder()
            .coding(Coding::ByteAligned)
            .strict(strict)
            .preserve(d.preserve)
            .build()
            .unwrap();
        let want = decode::<4096>(
            &g,
            &encode(&g, &byte, &input).unwrap(),
            usize::MAX,
            Limits::new(),
            Sequence::Single,
        )
        .unwrap();
        for coding in CODINGS {
            let blocks: &[u32] = match coding {
                Coding::PreCompression | Coding::Compression => {
                    &[1, 2, 3, 7, 100, 101, 250, 1_000_000]
                }
                _ => &[1_000_000],
            };
            for &block in blocks {
                let opts = Options::builder()
                    .coding(coding)
                    .strict(strict)
                    .preserve(d.preserve)
                    .block_size(block)
                    .build()
                    .unwrap();
                let exi = encode(&g, &opts, &input)
                    .unwrap_or_else(|e| panic!("{} {coding:?} block {block}: encode {e}", d.name));
                let whole = decode::<4096>(&g, &exi, usize::MAX, Limits::new(), Sequence::Single);
                assert_eq!(
                    whole.as_ref().ok(),
                    Some(&want),
                    "{} {coding:?} block {block}: {:?}",
                    d.name,
                    whole.as_ref().err()
                );
                // One octet at a time, through a small ring that has to spill.
                let bytewise = decode::<64>(&g, &exi, 1, Limits::new(), Sequence::Single);
                assert_eq!(
                    bytewise.as_ref().ok(),
                    Some(&want),
                    "{} {coding:?} block {block}, octet by octet: {:?}",
                    d.name,
                    bytewise.as_ref().err()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "{checked}");
}

#[test]
fn streams_follow_one_another_in_every_coding() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let a = events_of_xml(&g, b"<a x='1'>one<b/>two</a>", false);
    let b = events_of_xml(&g, b"<b y='2'><c>three</c></b>", false);
    for coding in CODINGS {
        let opts = Options::builder()
            .coding(coding)
            .block_size(2)
            .build()
            .unwrap();
        let mut exi = encode(&g, &opts, &a).unwrap();
        exi.extend(encode(&g, &opts, &b).unwrap());
        let got = decode::<4096>(&g, &exi, 3, Limits::new(), Sequence::Streams).unwrap();
        let want: Vec<OwnedEvent> = a.iter().chain(&b).cloned().collect();
        assert_eq!(got, want, "{coding:?}");
    }
}

#[test]
fn a_deflate_stream_that_inflates_too_far_is_a_limit() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    // 200 000 identical characters: a few hundred octets of DEFLATE.
    let text = "a".repeat(200_000);
    let input = events_of_xml(&g, format!("<r>{text}</r>").as_bytes(), false);
    let opts = Options::builder()
        .coding(Coding::Compression)
        .build()
        .unwrap();
    let exi = encode(&g, &opts, &input).unwrap();
    assert!(exi.len() < 2000, "{}", exi.len());
    let e = decode::<4096>(
        &g,
        &exi,
        usize::MAX,
        Limits::new().max_block_bytes(100_000),
        Sequence::Single,
    )
    .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Limit, "{e}");
    assert!(decode::<4096>(&g, &exi, usize::MAX, Limits::new(), Sequence::Single).is_ok());
}

#[test]
fn a_block_with_too_many_events_is_a_limit() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let xml = std::fs::read("fixtures/input/channels.xml").unwrap();
    let input = events_of_xml(&g, &xml, false);
    for coding in [Coding::PreCompression, Coding::Compression] {
        let opts = Options::builder().coding(coding).build().unwrap();
        let exi = encode(&g, &opts, &input).unwrap();
        let e = decode::<4096>(
            &g,
            &exi,
            usize::MAX,
            Limits::new().max_block_events(50),
            Sequence::Single,
        )
        .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Limit, "{coding:?}: {e}");
        // With small blocks, the same limit is enough.
        let opts = Options::builder()
            .coding(coding)
            .block_size(10)
            .build()
            .unwrap();
        let exi = encode(&g, &opts, &input).unwrap();
        assert!(
            decode::<4096>(
                &g,
                &exi,
                usize::MAX,
                Limits::new().max_block_events(50),
                Sequence::Single
            )
            .is_ok(),
            "{coding:?}"
        );
    }
}

#[test]
fn damaged_compressed_input_fails_without_panicking() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let xml = std::fs::read("fixtures/input/channels.xml").unwrap();
    let input = events_of_xml(&g, &xml, false);
    let want = events_of_xml(&g, &xml, false);
    for coding in [Coding::PreCompression, Coding::Compression] {
        let opts = Options::builder()
            .coding(coding)
            .block_size(37)
            .build()
            .unwrap();
        let exi = encode(&g, &opts, &input).unwrap();
        // Truncated anywhere: an error, never a panic or a wrong complete document.
        for cut in (2..exi.len()).step_by(97) {
            if let Ok(evs) =
                decode::<4096>(&g, &exi[..cut], usize::MAX, Limits::new(), Sequence::Single)
            {
                assert_ne!(evs, want, "{coding:?} cut at {cut}");
            }
        }
        // Octets changed: an error or other events, never a panic.
        for at in (2..exi.len()).step_by(53) {
            let mut bad = exi.clone();
            bad[at] ^= 0x5A;
            let _ = decode::<4096>(
                &g,
                &bad,
                usize::MAX,
                Limits::new().max_block_bytes(1 << 20),
                Sequence::Single,
            );
        }
    }
}

#[test]
fn deflate_levels_all_decode() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let xml = std::fs::read("fixtures/input/channels.xml").unwrap();
    let input = events_of_xml(&g, &xml, false);
    let opts = Options::builder()
        .coding(Coding::Compression)
        .build()
        .unwrap();
    let mut sizes = Vec::new();
    for level in [0, 1, 6, 9] {
        let mut w = Writer::new(
            Vec::new(),
            WriterConfig::new(opts.clone(), &g).deflate_level(level),
        )
        .unwrap();
        for e in &input {
            exi::EventSink::event(&mut w, &e.as_event()).unwrap();
        }
        let exi = w.into_sink();
        assert_eq!(
            decode::<4096>(&g, &exi, usize::MAX, Limits::new(), Sequence::Single).unwrap(),
            input,
            "level {level}"
        );
        sizes.push(exi.len());
    }
    // Level 0 stores; 9 is no larger than 1.
    assert!(sizes[0] > sizes[1] && sizes[3] <= sizes[1], "{sizes:?}");
}

/// The same stream without its empty DEFLATE streams: the layout EXI §9.3 describes.
fn without_empty_streams(exi: &[u8]) -> Vec<u8> {
    let h = exi::header::parse_header(exi, &Limits::new())
        .unwrap()
        .unwrap();
    let start = (h.body_offset_bits / 8) as usize;
    let mut out = exi[..start].to_vec();
    let mut at = start;
    while at < exi.len() {
        let mut z = flate2::Decompress::new(false);
        let mut v = Vec::with_capacity(1 << 16);
        loop {
            let st = z
                .decompress_vec(
                    &exi[at + z.total_in() as usize..],
                    &mut v,
                    flate2::FlushDecompress::Finish,
                )
                .unwrap();
            if st == flate2::Status::StreamEnd {
                break;
            }
            v.reserve(v.capacity());
        }
        let used = z.total_in() as usize;
        if !v.is_empty() {
            out.extend_from_slice(&exi[at..at + used]);
        }
        at += used;
    }
    out
}

#[cfg(feature = "xsd")]
#[test]
fn streams_with_or_without_empty_deflate_streams() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    for (doc, schema, blocks) in [
        ("onevalue.xml", "onevalue.xsd", &[1_000_000u32, 120, 7][..]),
        ("zero60.xml", "zero60.xsd", &[60, 30][..]),
    ] {
        let set = grammars(Some(schema));
        let g = set.attach(&strings).unwrap();
        let input = events_of_xml(
            &g,
            &std::fs::read(format!("fixtures/input/{doc}")).unwrap(),
            false,
        );
        let byte = Options::builder()
            .coding(Coding::ByteAligned)
            .strict(true)
            .build()
            .unwrap();
        let want = decode::<4096>(
            &g,
            &encode(&g, &byte, &input).unwrap(),
            usize::MAX,
            Limits::new(),
            Sequence::Single,
        )
        .unwrap();
        for &block in blocks {
            let opts = Options::builder()
                .coding(Coding::Compression)
                .strict(true)
                .block_size(block)
                .build()
                .unwrap();
            let ours = encode(&g, &opts, &input).unwrap();
            let lean = without_empty_streams(&ours);
            for (what, exi) in [("as written", &ours), ("without empty streams", &lean)] {
                for chunk in [usize::MAX, 1] {
                    let got = decode::<64>(&g, exi, chunk, Limits::new(), Sequence::Single);
                    assert_eq!(
                        got.as_ref().ok(),
                        Some(&want),
                        "{doc} block {block}, {what}, chunks of {chunk}: {:?}",
                        got.as_ref().err()
                    );
                }
            }
            if doc == "onevalue.xml" && block == 1_000_000 {
                assert!(
                    lean.len() < ours.len(),
                    "the k channel's empty stream is gone"
                );
            }
        }
    }
}
