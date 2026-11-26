// examples/esp32c6/src/lib.rs
//
// ESP32-C6 EXI serial demo
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Fixed sensor grammar and EXI wire format shared by the firmware and
//! host tests.

#![no_std]

use exi::coding::SliceSink;
use exi::grammar::informed::{IntegerRep, Representation};
use exi::grammar::{GrammarSet, Grammars};
use exi::{
    Coding, Error, ErrorKind, Event, EventSink, Header, Options, OwnedName,
    Reader, ReaderConfig, Result, Step, Value, Writer, WriterConfig,
};

pub const MAX_DOCUMENT: usize = 32;

pub fn grammar() -> Result<GrammarSet> {
    let mut builder = GrammarSet::builder();
    let scalar =
        builder.simple_type(None, Representation::Integer(IntegerRep::Signed));
    let ty = builder.simple_content_type(None, scalar);
    let root = builder.element(OwnedName::new("", "reading"), ty, false);
    builder.global(root);
    builder.build()
}

pub fn options() -> Result<Options> {
    Options::builder().coding(Coding::ByteAligned).strict(true).build()
}

pub fn encode(
    grammars: &Grammars<'_>,
    opts: &Options,
    reading: i64,
    out: &mut [u8],
) -> Result<usize> {
    let cfg = WriterConfig::new(opts.clone(), grammars)
        .header(Header::Bare { cookie: false });
    let mut writer = Writer::new(SliceSink::new(out), cfg)?;
    writer.start_element("reading")?;
    writer.text(Value::Int(reading))?;
    writer.end_element()?;
    writer.end_document()?;
    Ok(writer.into_sink().written().len())
}

pub fn decode(
    grammars: &Grammars<'_>,
    opts: &Options,
    bytes: &[u8],
) -> Result<i64> {
    let mut reader =
        Reader::<64>::new(ReaderConfig::new(grammars).options(opts.clone()))?;
    if bytes.len() > MAX_DOCUMENT {
        return Err(Error::new(ErrorKind::Limit, "serial document too long"));
    }
    if bytes.first() != Some(&0x80) {
        return Err(Error::new(
            ErrorKind::Header,
            "expected a bare EXI header",
        ));
    }
    reader.feed(bytes);
    reader.end_input();
    let bad =
        || Error::new(ErrorKind::Grammar, "expected one integer reading");
    if !matches!(reader.next()?, Step::Event(Event::StartDocument, _))
        || !matches!(reader.next()?, Step::Event(Event::StartElement(n), _) if n.is("", "reading"))
    {
        return Err(bad());
    }
    let Step::Event(Event::Characters(Value::Int(reading)), _) =
        reader.next()?
    else {
        return Err(bad());
    };
    if !matches!(reader.next()?, Step::Event(Event::EndElement(_), _))
        || !matches!(reader.next()?, Step::Event(Event::EndDocument, _))
        || !matches!(reader.next()?, Step::End)
        || reader.position().byte != bytes.len() as u64
    {
        return Err(bad());
    }
    Ok(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exi::strings::{GlobalConfig, GlobalStrings};

    #[test]
    fn readings_round_trip() -> Result<()> {
        let set = grammar()?;
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let g = set.attach(&strings)?;
        let opts = options()?;
        let mut bytes = [0; MAX_DOCUMENT];
        for reading in [i64::MIN, -21, 0, 21, i64::MAX] {
            let len = encode(&g, &opts, reading, &mut bytes)?;
            assert_eq!(decode(&g, &opts, &bytes[..len])?, reading);
        }
        let len = encode(&g, &opts, 21, &mut bytes)?;
        assert_eq!(&bytes[..len], b"\x80\0\0\x15");
        assert!(decode(&g, &opts, &bytes[..len - 1]).is_err());
        assert!(decode(&g, &opts, &bytes[..len + 1]).is_err());
        Ok(())
    }
}
