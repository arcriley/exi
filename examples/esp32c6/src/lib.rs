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
    let coding = if cfg!(feature = "bit-packed") {
        Coding::BitPacked
    } else {
        Coding::ByteAligned
    };
    Options::builder().coding(coding).strict(true).build()
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
    if bytes.len() > MAX_DOCUMENT {
        return Err(Error::new(ErrorKind::Limit, "serial document too long"));
    }
    if bytes.first() != Some(&0x80) {
        return Err(Error::new(
            ErrorKind::Header,
            "expected a bare EXI header",
        ));
    }
    let mut reader =
        Reader::<64>::new(ReaderConfig::new(grammars).options(opts.clone()))?;
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

/// Returns the encoded length of the serial reply, or a value error
/// if the reading cannot be incremented.
///
/// ```
/// use exi::strings::{GlobalConfig, GlobalStrings};
/// use exi_esp32c6::{MAX_DOCUMENT, decode, encode, grammar, options, reply};
///
/// let strings = GlobalStrings::new(GlobalConfig::device())?;
/// let set = grammar()?;
/// let g = set.attach(&strings)?;
/// let opts = options()?;
/// let mut request = [0; MAX_DOCUMENT];
/// let mut response = [0; MAX_DOCUMENT];
/// let len = encode(&g, &opts, 21, &mut request)?;
/// let len = reply(&g, &opts, &request[..len], &mut response)?;
/// assert_eq!(decode(&g, &opts, &response[..len])?, 22);
/// # Ok::<(), exi::Error>(())
/// ```
pub fn reply(
    grammars: &Grammars<'_>,
    opts: &Options,
    request: &[u8],
    out: &mut [u8],
) -> Result<usize> {
    let value = decode(grammars, opts, request)?;
    let next = value.checked_add(1).ok_or_else(|| {
        Error::new(ErrorKind::Value, "reading cannot be incremented")
    })?;
    encode(grammars, opts, next, out)
}

/// Hex text (a serial line) into `out`: the number of octets, or `None`
/// for an odd length, a character that is not a hex digit, or more
/// octets than `out` holds.
pub fn from_hex(text: &[u8], out: &mut [u8]) -> Option<usize> {
    let pairs = text.chunks_exact(2);
    if !pairs.remainder().is_empty() || pairs.len() > out.len() {
        return None;
    }
    let digit = |b| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let len = pairs.len();
    for (pair, slot) in pairs.zip(out) {
        *slot = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(len)
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
        let expected: &[u8] = if cfg!(feature = "bit-packed") {
            &[0x80, 0x05, 0x40]
        } else {
            &[0x80, 0, 0, 0x15]
        };
        assert_eq!(&bytes[..len], expected);
        assert!(decode(&g, &opts, &bytes[..len - 1]).is_err());
        assert!(decode(&g, &opts, &bytes[..len + 1]).is_err());
        Ok(())
    }

    #[test]
    fn reply_at_integer_limit() -> Result<()> {
        let set = grammar()?;
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let g = set.attach(&strings)?;
        let opts = options()?;
        let mut request = [0; MAX_DOCUMENT];
        let mut response = [0; MAX_DOCUMENT];
        let len = encode(&g, &opts, i64::MAX, &mut request)?;
        let err =
            reply(&g, &opts, &request[..len], &mut response).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Value);
        let len = encode(&g, &opts, i64::MAX - 1, &mut request)?;
        let len = reply(&g, &opts, &request[..len], &mut response)?;
        assert_eq!(decode(&g, &opts, &response[..len])?, i64::MAX);
        Ok(())
    }

    /// A reading one past either end of i64 is an error, not a wrapped
    /// value. Signed integers are a sign bit (a byte here), then the
    /// magnitude (less one when negative) as an unsigned integer: 2^63
    /// is nine octets of 0x80 and a final 0x01.
    #[test]
    fn readings_past_i64_are_errors() -> Result<()> {
        let set = grammar()?;
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let g = set.attach(&strings)?;
        let opts = Options::builder()
            .coding(Coding::ByteAligned)
            .strict(true)
            .build()?;
        let two_63 =
            [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        for sign in [0, 1] {
            let mut stream = [0x80, 0, sign, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            stream[3..].copy_from_slice(&two_63);
            assert!(decode(&g, &opts, &stream).is_err(), "sign {sign}");
        }
        // One less is the largest positive reading.
        let mut bytes = [0; MAX_DOCUMENT];
        let len = encode(&g, &opts, i64::MAX, &mut bytes)?;
        assert_eq!(
            &bytes[3..len],
            &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]
        );
        Ok(())
    }

    #[test]
    fn hex_lines() {
        let mut out = [0; 4];
        assert_eq!(from_hex(b"80001500", &mut out), Some(4));
        assert_eq!(out, [0x80, 0, 0x15, 0]);
        assert_eq!(from_hex(b"aBcD", &mut out), Some(2));
        assert_eq!(&out[..2], &[0xAB, 0xCD]);
        assert_eq!(from_hex(b"", &mut out), Some(0));
        assert_eq!(from_hex(b"801", &mut out), None); // odd length
        assert_eq!(from_hex(b"8g", &mut out), None); // not a hex digit
        assert_eq!(from_hex(b" 80", &mut out), None);
        assert_eq!(from_hex(b"0011223344", &mut out), None); // five octets
    }
}
