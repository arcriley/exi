// tests/core.rs
//
// Core event-level EXI tests that run without optional features (`xml`,
// `bit-packed`, `compression`, `std`). Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Event-level EXI tests for the minimal codec and optional codings.

use exi::coding::SliceSink;
use exi::grammar::GrammarSet;
use exi::grammar::informed::{IntegerRep, Prod, Representation};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{
    Coding, Doctype, ErrorKind, Event, EventSink, Header, NsDecl, Options,
    OwnedEvent, OwnedName, OwnedValue, Preserve, Reader, ReaderConfig,
    Sequence, Status, Step, Value, Writer, WriterConfig,
};

fn read_all<const N: usize>(
    r: &mut Reader<'_, N>,
    bytes: &[u8],
    chunk: usize,
) -> exi::Result<Vec<OwnedEvent>> {
    let mut out = Vec::new();
    for part in bytes.chunks(chunk) {
        let mut rest = part;
        while !rest.is_empty() {
            let n = r.feed(rest);
            rest = &rest[n..];
            loop {
                match r.next()? {
                    Step::Event(e, _) => out.push(e.to_owned()),
                    Step::NeedInput => break,
                    Step::End => return Ok(out),
                }
            }
        }
    }
    r.end_input();
    loop {
        match r.next()? {
            Step::Event(e, _) => out.push(e.to_owned()),
            Step::NeedInput => panic!("unexpected NeedInput after end_input"),
            Step::End => return Ok(out),
        }
    }
}

#[test]
fn schemaless_byte_aligned_chunked_and_skip_element() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings).unwrap();
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .preserve(Preserve::all())
        .self_contained(true)
        .build()
        .unwrap();

    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), &grammars)
            .header(Header::WithOptions { cookie: true }),
    )
    .unwrap();
    w.event(&Event::Doctype(Doctype {
        name: "root",
        public_id: "",
        system_id: "",
        text: "",
    }))
    .unwrap();
    w.event(&Event::Comment("top")).unwrap();
    w.event(&Event::ProcessingInstruction { target: "pi", data: "x" })
        .unwrap();
    w.start_element(("urn:x", "root")).unwrap();
    w.event(&Event::Namespace(NsDecl {
        prefix: "x",
        uri: "urn:x",
        local_element_ns: true,
    }))
    .unwrap();
    w.attribute("k", "same").unwrap();
    w.start_element("skip").unwrap();
    w.event(&Event::SelfContained).unwrap();
    w.attribute("inner", "same").unwrap();
    w.start_element("child").unwrap();
    w.text("ignored").unwrap();
    w.end_element().unwrap();
    w.end_element().unwrap();
    w.start_element("keep").unwrap();
    w.text("same").unwrap();
    w.end_element().unwrap();
    w.end_element().unwrap();
    w.end_document().unwrap();
    let bytes = w.into_sink();
    assert!(bytes.starts_with(b"$EXI"));

    // Complete read, 1 byte at a time.
    let mut r = Reader::<64>::new(ReaderConfig::new(&grammars)).unwrap();
    let events = read_all(&mut r, &bytes, 1).unwrap();
    assert!(matches!(events.first(), Some(OwnedEvent::StartDocument)));
    assert!(matches!(events.last(), Some(OwnedEvent::EndDocument)));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnedEvent::Characters(OwnedValue::Str(s)) if &**s == "ignored"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnedEvent::StartElement(n) if n.as_name().local_name() == "skip"
    )));

    // Mid-stream skip_element over the self-contained <skip> subtree
    // across 1-byte feeds.
    let mut r = Reader::<64>::new(ReaderConfig::new(&grammars)).unwrap();
    let mut skipping = false;
    let mut texts = Vec::new();
    for b in bytes.chunks(1) {
        r.feed(b);
        loop {
            if skipping {
                match r.skip_element().unwrap() {
                    Status::NeedInput => break,
                    Status::End => {
                        skipping = false;
                        continue;
                    }
                }
            }
            match r.next().unwrap() {
                Step::Event(Event::StartElement(n), _)
                    if n.local_name() == "skip" =>
                {
                    skipping = true;
                }
                Step::Event(Event::Characters(v), _) => {
                    texts.push(v.as_str().unwrap().to_owned());
                }
                Step::Event(..) => {}
                Step::NeedInput => break,
                Step::End => break,
            }
        }
    }
    r.end_input();
    assert!(!skipping);
    assert!(matches!(r.next().unwrap(), Step::End));
    assert_eq!(texts, ["same"]);
}

#[test]
fn schema_informed_non_strict_and_slice_sink() {
    const NS: &str = "urn:core";
    let mut b = GrammarSet::builder();
    let item_ty =
        b.simple_type(None, Representation::String { charset: None });
    let bounded_ty = b.simple_type(
        None,
        Representation::Integer(IntegerRep::Bounded { min: -5, max: 5 }),
    );
    let enum_ty = b.simple_type(
        None,
        Representation::Enumeration {
            values: ["alpha", "beta"].into_iter().map(Into::into).collect(),
            base: item_ty,
        },
    );
    let item_ty =
        b.simple_type(None, Representation::String { charset: None });
    let leaf_ty = b.simple_content_type(None, bounded_ty);
    let leaf = b.element(OwnedName::new(NS, "leaf"), leaf_ty, true);
    let root_ty = b.complex_type(
        None,
        vec![
            vec![
                Prod::At {
                    name: OwnedName::new("", "mode"),
                    ty: enum_ty,
                    next: 0,
                },
                Prod::Se { element: leaf, next: 1 },
            ],
            vec![Prod::EE],
        ],
    );
    b.castable(root_ty);
    let ext_ty = b.complex_type(
        None,
        vec![
            vec![
                Prod::At {
                    name: OwnedName::new("", "extra"),
                    ty: item_ty,
                    next: 0,
                },
                Prod::Se { element: leaf, next: 1 },
            ],
            vec![Prod::EE],
        ],
    );
    b.derives(ext_ty, root_ty);
    let root = b.element(OwnedName::new(NS, "root"), root_ty, false);
    b.global(root);
    let set = b.build().unwrap();
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let grammars = set.attach(&strings).unwrap();

    // Non-strict mode exercises cached non_strict_table and deviation
    // fallback.
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .strict(false)
        .schema_id(exi::SchemaId::Id("urn:core".into()))
        .build()
        .unwrap();

    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), &grammars)
            .header(Header::Bare { cookie: false }),
    )
    .unwrap();
    w.start_element((NS, "root")).unwrap();
    w.attribute("mode", "beta").unwrap();
    w.attribute("undeclared", "fallback").unwrap();
    w.start_element((NS, "leaf")).unwrap();
    w.event(&Event::Characters(Value::Int(4))).unwrap();
    w.end_element().unwrap();
    w.end_element().unwrap();
    w.end_document().unwrap();
    let bytes = w.into_sink();

    let mut r =
        Reader::<128>::new(ReaderConfig::new(&grammars).options(opts.clone()))
            .unwrap();
    let events = read_all(&mut r, &bytes, 3).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        OwnedEvent::Attribute(_, OwnedValue::Enum { ordinal: 1, .. })
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnedEvent::Characters(OwnedValue::Int(4))))
    );

    // SliceSink reports BufferFull when too small for the stream.
    let mut small = [0u8; 4];
    let err = (|| -> exi::Result<()> {
        let mut sw = Writer::new(
            SliceSink::new(&mut small),
            WriterConfig::new(opts, &grammars)
                .header(Header::Bare { cookie: false }),
        )?;
        sw.start_element((NS, "root"))?;
        sw.start_element((NS, "leaf"))?;
        sw.event(&Event::Characters(Value::Int(1)))?;
        sw.end_element()?;
        sw.end_element()?;
        sw.end_document()
    })()
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::BufferFull);
}

#[test]
fn frames_boundaries_and_errors() {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings).unwrap();
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .fragment(true)
        .build()
        .unwrap();

    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts.clone(), &grammars)
            .header(Header::Bare { cookie: false })
            .sequence(Sequence::Frames),
    )
    .unwrap();
    w.start_element("m").unwrap();
    w.text("one").unwrap();
    w.end_element().unwrap();
    let f1 = w.end_frame().unwrap().to_vec();
    w.start_element("m").unwrap();
    w.text("two").unwrap();
    w.end_element().unwrap();
    let f2 = w.end_frame().unwrap().to_vec();

    // Valid two-frame decode followed by clean end_input.
    let mut r = Reader::<128>::new(
        ReaderConfig::new(&grammars)
            .options(opts.clone())
            .sequence(Sequence::Frames),
    )
    .unwrap();
    for f in [&f1, &f2] {
        r.feed(f);
        r.end_frame();
        while let Step::Event(..) = r.next().unwrap() {}
    }
    r.end_input();
    assert!(matches!(r.next().unwrap(), Step::End));

    // Truncated frame boundary returns ErrorKind::Grammar ("a frame
    // ends inside an element").
    r.reset();
    r.feed(&f1[..f1.len() - 1]);
    r.end_frame();
    let mut trunc_err = None;
    for _ in 0..10 {
        match r.next() {
            Ok(Step::Event(..)) => {}
            Ok(Step::NeedInput | Step::End) => break,
            Err(e) => {
                trunc_err = Some(e.kind());
                break;
            }
        }
    }
    assert_eq!(trunc_err, Some(ErrorKind::Grammar));
    assert_eq!(r.next().unwrap_err().kind(), ErrorKind::State);

    // Trailing octets in a frame also fail with ErrorKind::Grammar.
    r.reset();
    let mut extra = f1.clone();
    extra.push(0x00);
    r.feed(&extra);
    r.end_frame();
    let mut saw_err = None;
    for _ in 0..10 {
        match r.next() {
            Ok(Step::Event(..)) => {}
            Ok(Step::NeedInput | Step::End) => break,
            Err(e) => {
                saw_err = Some(e.kind());
                break;
            }
        }
    }
    assert_eq!(saw_err, Some(ErrorKind::Grammar));
}

#[cfg(any(feature = "bit-packed", feature = "compression"))]
fn chunked_skip(coding: Coding) -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder().coding(coding).block_size(1).build()?;
    let mut w = Writer::new(
        Vec::new(),
        WriterConfig::new(opts, &grammars)
            .header(Header::WithOptions { cookie: false }),
    )?;
    w.start_element("root")?;
    w.start_element("drop")?;
    w.start_element("nested")?;
    w.text("first")?;
    w.end_element()?;
    w.text("second")?;
    w.end_element()?;
    w.start_element("keep")?;
    w.text("kept")?;
    w.end_element()?;
    w.end_element()?;
    w.end_document()?;
    let bytes = w.into_sink();

    for truncate in [false, true] {
        let mut r = Reader::<64>::new(ReaderConfig::new(&grammars))?;
        let mut input = bytes.iter();
        let mut feed = |r: &mut Reader<'_, 64>| {
            if let Some(byte) = input.next() {
                assert_eq!(r.feed(core::slice::from_ref(byte)), 1);
            } else {
                r.end_input();
            }
        };
        loop {
            match r.next()? {
                Step::Event(Event::StartElement(n), _)
                    if n.local_name() == "drop" =>
                {
                    break;
                }
                Step::Event(..) => {}
                Step::NeedInput => feed(&mut r),
                Step::End => panic!("missing the subtree to skip"),
            }
        }

        assert_eq!(r.skip_element()?, Status::NeedInput);
        if truncate {
            r.end_input();
            assert_eq!(
                r.skip_element().unwrap_err().kind(),
                ErrorKind::Truncated
            );
            continue;
        }
        loop {
            feed(&mut r);
            if r.skip_element()? == Status::End {
                break;
            }
        }
        let mut texts = Vec::new();
        loop {
            match r.next()? {
                Step::Event(Event::Characters(v), _) => {
                    texts.push(v.as_str().unwrap().to_owned())
                }
                Step::Event(..) => {}
                Step::NeedInput => feed(&mut r),
                Step::End => break,
            }
        }
        assert_eq!(texts, ["kept"]);
    }
    Ok(())
}

#[cfg(feature = "bit-packed")]
#[test]
fn bit_packed_skip_element() -> exi::Result<()> {
    chunked_skip(Coding::BitPacked)
}

#[cfg(feature = "compression")]
#[test]
fn compressed_skip_element() -> exi::Result<()> {
    chunked_skip(Coding::Compression)
}

#[test]
fn independent_documents_on_reused_codec() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    let mut w =
        Writer::new(Vec::new(), WriterConfig::new(opts.clone(), &grammars))?;
    let mut r = Reader::<64>::new(ReaderConfig::new(&grammars).options(opts))?;
    let mut first = None;
    for _ in 0..2 {
        w.start_element("message")?;
        w.text("repeated")?;
        w.end_element()?;
        w.end_document()?;
        let bytes = w.reset(Vec::new());
        if let Some(first) = &first {
            assert_eq!(&bytes, first);
        }
        let events = read_all(&mut r, &bytes, 1)?;
        assert!(matches!(events.first(), Some(OwnedEvent::StartDocument)));
        assert!(events.contains(&OwnedEvent::Characters(OwnedValue::Str(
            "repeated".into()
        ))));
        assert!(matches!(events.last(), Some(OwnedEvent::EndDocument)));
        r.reset();
        first = Some(bytes);
    }
    let bytes = first.unwrap();
    let truncated = &bytes[..bytes.len() - 1];
    assert_eq!(
        read_all(&mut r, truncated, 1).unwrap_err().kind(),
        ErrorKind::Truncated
    );
    Ok(())
}

/// Events of `bytes`, reading through `Reader::buffer` and
/// `Reader::commit` a few octets at a time.
fn read_committed<const N: usize>(
    r: &mut Reader<'_, N>,
    bytes: &[u8],
) -> exi::Result<Vec<OwnedEvent>> {
    let mut rest = bytes;
    let mut out = Vec::new();
    loop {
        let buf = r.buffer();
        let n = buf.len().min(rest.len()).min(7);
        buf[..n].copy_from_slice(&rest[..n]);
        r.commit(n);
        rest = &rest[n..];
        if rest.is_empty() {
            r.end_input();
        }
        match r.next()? {
            Step::Event(e, _) => out.push(e.to_owned()),
            Step::NeedInput => {}
            Step::End => return Ok(out),
        }
    }
}

#[test]
fn buffer_and_commit_while_an_event_spills() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    let text = "y".repeat(300); // larger than the 64-octet ring
    let mut w =
        Writer::new(Vec::new(), WriterConfig::new(opts.clone(), &grammars))?;
    w.start_element("r")?;
    for _ in 0..2 {
        w.start_element("a")?;
        w.text(text.as_str())?; // the second: a hit after the spill
        w.end_element()?;
    }
    w.end_element()?;
    w.end_document()?;
    let bytes = w.into_sink();
    let mut r = Reader::<64>::new(ReaderConfig::new(&grammars))?;
    let events = read_committed(&mut r, &bytes)?;
    let long = OwnedEvent::Characters(OwnedValue::Str(text.into()));
    assert_eq!(events.iter().filter(|e| **e == long).count(), 2);
    Ok(())
}

/// `<r><skip k="v"><x:in xmlns:x="urn:x">v</x:in><sc>v</sc></skip>
/// <after k="v">v</after></r>`, with `<sc>` self-contained: values
/// learned inside the skipped element are hits afterwards.
#[test]
fn skip_element_keeps_the_string_tables() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .preserve(Preserve::PREFIXES)
        .self_contained(true)
        .build()?;
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &grammars))?;
    w.start_element("r")?;
    w.start_element("skip")?;
    w.attribute("k", "v")?;
    w.start_element(exi::Name::new("urn:x", "in").with_prefix("x"))?;
    w.namespace("x", "urn:x")?;
    w.text("v")?;
    w.end_element()?;
    w.start_element("sc")?;
    w.event(&Event::SelfContained)?;
    w.text("v")?;
    w.end_element()?;
    w.end_element()?;
    w.start_element("after")?;
    w.attribute("k", "v")?;
    w.text("v")?;
    w.end_element()?;
    w.end_element()?;
    w.end_document()?;
    let bytes = w.into_sink();

    for (skipped, depth) in [("skip", 1), ("sc", 2), ("in", 2)] {
        let mut r: Reader = Reader::new(ReaderConfig::new(&grammars))?;
        r.feed(&bytes);
        r.end_input();
        let mut names = Vec::new();
        let mut values = Vec::new();
        loop {
            let skip = match r.next()? {
                Step::Event(Event::StartElement(n), _) => {
                    names.push(n.local_name().to_owned());
                    n.local_name() == skipped
                }
                Step::Event(Event::Attribute(_, v), _)
                | Step::Event(Event::Characters(v), _) => {
                    values.push(v.as_str().unwrap().to_owned());
                    false
                }
                Step::Event(..) => false,
                _ => break,
            };
            if skip {
                assert_eq!(r.skip_element()?, Status::End);
                assert_eq!(r.depth(), depth, "after skipping {skipped}");
            }
        }
        assert_eq!(names.last().unwrap(), "after");
        assert!(values.ends_with(&["v".into(), "v".into()]));
    }
    Ok(())
}

/// `next` between `NeedInput` and the next `skip_element` gives up the
/// skip: a later skip starts from the element open then.
#[test]
fn next_gives_up_an_unfinished_skip() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    let cfg = WriterConfig::new(opts.clone(), &grammars)
        .header(Header::Bare { cookie: false });
    let mut w = Writer::new(Vec::new(), cfg)?;
    // <r><a/><c><d><e/></d><f/></c></r>, cut after <a>
    w.start_element("r")?;
    w.start_element("a")?;
    w.flush()?;
    let cut = 1 + w.position().byte as usize; // the header is one octet
    w.end_element()?;
    w.start_element("c")?;
    w.start_element("d")?;
    w.start_element("e")?;
    w.end_element()?;
    w.end_element()?;
    w.start_element("f")?;
    w.end_element()?;
    w.end_element()?;
    w.end_element()?;
    w.end_document()?;
    let bytes = w.into_sink();

    let mut r = Reader::<64>::new(ReaderConfig::new(&grammars).options(opts))?;
    r.feed(&bytes[..cut]);
    while let Step::Event(..) = r.next()? {}
    assert_eq!(r.depth(), 2); // in <a>
    assert_eq!(r.skip_element()?, Status::NeedInput);
    r.feed(&bytes[cut..]);
    r.end_input();
    loop {
        match r.next()? {
            Step::Event(Event::StartElement(n), _)
                if n.local_name() == "d" =>
            {
                break;
            }
            Step::Event(..) => {}
            _ => panic!("no <d>"),
        }
    }
    assert_eq!(r.skip_element()?, Status::End); // skips <d> only
    assert!(matches!(
        r.next()?,
        Step::Event(Event::StartElement(n), _) if n.local_name() == "f"
    ));
    Ok(())
}

/// Rule 4 of the writer: an `xmlns` attribute after `flush` still gives
/// the element its prefix.
#[test]
fn xmlns_attribute_after_flush_keeps_the_prefix() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .preserve(Preserve::PREFIXES)
        .build()?;
    let xmlns = exi::Name::new("http://www.w3.org/2000/xmlns/", "p");
    let mut seen = Vec::new();
    for flush in [false, true] {
        let mut w = Writer::new(
            Vec::new(),
            WriterConfig::new(opts.clone(), &grammars),
        )?;
        w.start_element(exi::Name::new("urn:a", "el").with_prefix("p"))?;
        if flush {
            w.flush()?;
        }
        w.attribute(xmlns, "urn:a")?;
        w.end_element()?;
        w.end_document()?;
        let mut r = Reader::<64>::new(ReaderConfig::new(&grammars))?;
        seen.push(read_all(&mut r, &w.into_sink(), 64)?);
    }
    let el = OwnedName::new("urn:a", "el").with_prefix("p");
    assert_eq!(seen[1][1], OwnedEvent::StartElement(el));
    assert_eq!(seen[0], seen[1]);
    Ok(())
}

/// `Sequence::Streams`: each stream takes the options of its own
/// header, or `ReaderConfig::options` for a bare one, never the
/// previous stream's. Input that ends inside a later stream is
/// truncated.
#[cfg(all(feature = "bit-packed", feature = "compression"))]
#[test]
fn streams_with_mixed_headers_and_codings() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let byte = Options::builder().coding(Coding::ByteAligned).build()?;
    let streams = [
        (Coding::BitPacked, Header::WithOptions { cookie: true }),
        (Coding::Compression, Header::WithOptions { cookie: false }),
        (Coding::ByteAligned, Header::Bare { cookie: false }),
    ];
    let mut bytes = Vec::new();
    for (i, (coding, header)) in streams.into_iter().enumerate() {
        let opts = Options::builder().coding(coding).build()?;
        let cfg = WriterConfig::new(opts, &grammars).header(header);
        let mut w = Writer::new(Vec::new(), cfg)?;
        w.start_element("s")?;
        w.text(stream_text(i).as_str())?;
        w.end_element()?;
        w.end_document()?;
        bytes.extend(w.into_sink());
    }
    let cfg =
        ReaderConfig::new(&grammars).options(byte).sequence(Sequence::Streams);

    let mut r: Reader = Reader::new(cfg.clone())?;
    r.feed(&bytes);
    r.end_input();
    let mut texts = Vec::new();
    let mut codings = Vec::new();
    loop {
        match r.next()? {
            Step::Event(Event::StartDocument, _) => {
                codings.push(r.options().unwrap().coding());
            }
            Step::Event(Event::Characters(v), _) => {
                texts.push(v.as_str().unwrap().to_owned());
            }
            Step::Event(..) => {}
            _ => break,
        }
    }
    assert_eq!(codings, streams.map(|(c, _)| c));
    assert_eq!(texts, (0..3).map(stream_text).collect::<Vec<_>>());

    let mut r: Reader = Reader::new(cfg)?;
    r.feed(&bytes[..bytes.len() - 1]); // the third stream lacks its ED
    r.end_input();
    let e = loop {
        match r.next() {
            Ok(Step::Event(..)) => {}
            Ok(s) => panic!("{s:?} before the end"),
            Err(e) => break e,
        }
    };
    assert_eq!(e.kind(), ErrorKind::Truncated);
    Ok(())
}

fn stream_text(i: usize) -> String {
    format!("stream {i}")
}

/// One step of a canonical-EXI test document.
enum Ev<'a> {
    Se(&'a str),
    Space(&'a str), // xml:space
    Text(Value<'a>),
    Comment,
    Pi,
    Ee,
}

/// `events` written with or without `WriterConfig::canonical`.
fn write_doc(canonical: bool, events: &[Ev<'_>]) -> exi::Result<Vec<u8>> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let grammars = set.attach(&strings)?;
    let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    let cfg = WriterConfig::new(opts, &grammars).canonical(canonical);
    let mut w = Writer::new(Vec::new(), cfg)?;
    let space =
        exi::Name::new("http://www.w3.org/XML/1998/namespace", "space")
            .with_prefix("xml");
    for e in events {
        match e {
            Ev::Se(n) => w.start_element(*n)?,
            Ev::Space(v) => w.attribute(space, *v)?,
            Ev::Text(v) => w.event(&Event::Characters(*v))?,
            Ev::Comment => w.comment("c")?,
            Ev::Pi => w.event(&Event::ProcessingInstruction {
                target: "p",
                data: "d",
            })?,
            Ev::Ee => w.end_element()?,
        }
    }
    w.end_document()?;
    Ok(w.into_sink())
}

fn s(t: &str) -> Ev<'_> {
    Ev::Text(Value::Str(t))
}

/// Canonical EXI: a comment or PI the options leave out joins the text
/// around it into one CH.
#[test]
fn canonical_text_joins_across_dropped_comments_and_pis() -> exi::Result<()> {
    use Ev::*;
    let canonical =
        write_doc(true, &[Se("a"), s("x"), Comment, s("y"), Pi, s("z"), Ee])?;
    assert_eq!(canonical, write_doc(false, &[Se("a"), s("xyz"), Ee])?);
    Ok(())
}

/// Canonical EXI: a typed value followed by text is one CH, written as
/// text.
#[test]
fn canonical_typed_value_joins_following_text() -> exi::Result<()> {
    use Ev::*;
    let canonical =
        write_doc(true, &[Se("a"), Ev::Text(Value::Int(1)), s("2"), Ee])?;
    assert_eq!(canonical, write_doc(false, &[Se("a"), s("12"), Ee])?);
    Ok(())
}

/// Canonical EXI: whitespace between elements is dropped, except under
/// `xml:space="preserve"`, which children inherit until one says
/// `default`.
#[test]
#[rustfmt::skip]
fn canonical_whitespace_and_xml_space() -> exi::Result<()> {
    use Ev::*;
    let canonical = write_doc(
        true,
        &[
            Se("r"), s("\n "),
            Se("p"), Space("preserve"), s("\n "),
            Se("i"), s("\n "), Se("b"), Ee, s("\n "), Ee, // inherits
            s("\n "),
            Se("q"), Space("default"), s("\n "), Se("b"), Ee, s("\n "), Ee,
            s("\n "),
            Ee,
            s("\n"),
            Ee,
        ],
    )?;
    let expected = write_doc(
        false,
        &[
            Se("r"),
            Se("p"), Space("preserve"), s("\n "),
            Se("i"), s("\n "), Se("b"), Ee, s("\n "), Ee,
            s("\n "),
            Se("q"), Space("default"), Se("b"), Ee, Ee,
            s("\n "),
            Ee,
            Ee,
        ],
    )?;
    assert_eq!(canonical, expected);
    Ok(())
}
