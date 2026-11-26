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
