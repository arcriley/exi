//! Whole and split byte streams across all supported body codings.

use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{
    Coding, ErrorKind, EventSink, Header, Options, OwnedEvent, Reader,
    ReaderConfig, Step, Writer, WriterConfig,
};

include!("support/cases.rs");

fn decode(
    g: &Grammars<'_>,
    options: &Options,
    mut bytes: &[u8],
    chunk: usize,
) -> exi::Result<Vec<OwnedEvent>> {
    let mut reader =
        Reader::<64>::new(ReaderConfig::new(g).options(options.clone()))?;
    let mut events = Vec::new();
    loop {
        let fed = reader.feed(&bytes[..bytes.len().min(chunk)]);
        bytes = &bytes[fed..];
        if bytes.is_empty() {
            reader.end_input();
        }
        loop {
            match reader.next()? {
                Step::Event(event, _) => events.push(event.to_owned()),
                Step::End => return Ok(events),
                Step::NeedInput => {
                    assert!(fed > 0 && !bytes.is_empty());
                    break;
                }
            }
        }
    }
}

fn split_document(coding: Coding, depth: usize, attributes: usize) {
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let options =
        Options::builder().coding(coding).block_size(3).build().unwrap();
    let mut writer = Writer::new(
        Vec::new(),
        WriterConfig::new(options.clone(), &g)
            .header(Header::Bare { cookie: true }),
    )
    .unwrap();
    for level in 0..depth {
        writer.start_element("node").unwrap();
        for i in 0..attributes {
            writer.attribute(format!("attribute{i}").as_str(), "é😀").unwrap();
        }
        if level + 1 == depth && depth > 1 {
            writer.text("value in the deepest element").unwrap();
        }
    }
    for _ in 0..depth {
        writer.end_element().unwrap();
    }
    writer.end_document().unwrap();
    let bytes = writer.into_sink();
    let expected = decode(&g, &options, &bytes, bytes.len()).unwrap();
    assert_eq!(
        expected
            .iter()
            .filter(|e| matches!(e, OwnedEvent::StartElement(_)))
            .count(),
        depth
    );
    assert_eq!(
        expected
            .iter()
            .filter(|e| matches!(e, OwnedEvent::Attribute(..)))
            .count(),
        depth * attributes
    );
    for chunk in [1, 2, 3, 11] {
        assert_eq!(decode(&g, &options, &bytes, chunk).unwrap(), expected);
    }
    for end in 0..bytes.len() {
        let error = decode(&g, &options, &bytes[..end], 3).unwrap_err();
        assert_eq!(
            error.kind(),
            ErrorKind::Truncated,
            "cut {end} of {}: {error}",
            bytes.len()
        );
    }
}

cases!(split_document;
    byte_empty_element(Coding::ByteAligned, 1, 0),
    byte_many_attributes(Coding::ByteAligned, 1, 16),
    byte_deep_tree(Coding::ByteAligned, 48, 0),
);
#[cfg(feature = "bit-packed")]
cases!(split_document;
    bit_empty_element(Coding::BitPacked, 1, 0),
    bit_many_attributes(Coding::BitPacked, 1, 16),
    bit_deep_tree(Coding::BitPacked, 48, 0),
);
#[cfg(feature = "compression")]
cases!(split_document;
    pre_empty_element(Coding::PreCompression, 1, 0),
    pre_many_attributes(Coding::PreCompression, 1, 16),
    pre_deep_tree(Coding::PreCompression, 48, 0),
    compressed_empty_element(Coding::Compression, 1, 0),
    compressed_many_attributes(Coding::Compression, 1, 16),
    compressed_deep_tree(Coding::Compression, 48, 0),
);
