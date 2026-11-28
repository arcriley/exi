// benches/esp32c6/src/lib.rs
//
// ESP32-C6 EXI benchmark codecs and schemas
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Fixed sensor schemas and EXI/XML benchmark codecs shared by the
//! XIAO ESP32-C6 firmware, host tests, and WebAssembly tests.

#![no_std]

extern crate alloc;

use alloc::vec;
#[cfg(feature = "extended")]
use alloc::vec::Vec;

use exi::coding::{Sink, SliceSink};
use exi::grammar::informed::{IntegerRep, Prod, Representation};
use exi::grammar::{GrammarSet, Grammars};
#[cfg(feature = "extended")]
use exi::xml::{XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::{
    BinaryKind, Coding, Error, ErrorKind, Event, EventSink, Header, Limits,
    Options, OwnedName, Reader, ReaderConfig, Result, Step, Value, Writer,
    WriterConfig,
};

pub const MAX_DOCUMENT: usize = 32;
pub const SENSOR_NS: &str = "urn:example:sensor";
pub const SENSOR_TEMPERATURE: i64 = 215;

/// Single-element `<reading>` grammar with signed integer content.
pub fn grammar() -> Result<GrammarSet> {
    let mut builder = GrammarSet::builder();
    let scalar =
        builder.simple_type(None, Representation::Integer(IntegerRep::Signed));
    let ty = builder.simple_content_type(None, scalar);
    let root = builder.element(OwnedName::new("", "reading"), ty, false);
    builder.global(root);
    builder.build()
}

/// Fixed-schema sensor grammar matching `benches/device.rs`:
/// `<reading xmlns="urn:example:sensor"><temperature>int</temperature><samples>base64</samples></reading>`.
pub fn sensor_grammar() -> Result<GrammarSet> {
    let mut b = GrammarSet::builder();
    let int = b.simple_type(None, Representation::Integer(IntegerRep::Signed));
    let binary =
        b.simple_type(None, Representation::Binary(BinaryKind::Base64));
    let int_type = b.simple_content_type(None, int);
    let binary_type = b.simple_content_type(None, binary);
    let temperature =
        b.element(OwnedName::new(SENSOR_NS, "temperature"), int_type, false);
    let samples =
        b.element(OwnedName::new(SENSOR_NS, "samples"), binary_type, false);
    let reading_type = b.complex_type(
        None,
        vec![
            vec![Prod::Se { element: temperature, next: 1 }],
            vec![Prod::Se { element: samples, next: 2 }],
            vec![Prod::EE],
        ],
    );
    let reading =
        b.element(OwnedName::new(SENSOR_NS, "reading"), reading_type, false);
    b.global(reading);
    b.build()
}

pub fn options() -> Result<Options> {
    Options::builder().coding(Coding::ByteAligned).strict(true).build()
}

pub fn sensor_options(coding: Coding) -> Result<Options> {
    Options::builder()
        .coding(coding)
        .strict(true)
        .value_partition_capacity(Some(0))
        .build()
}

/// Deterministic sample bytes matching `benches/device.rs`.
pub fn fill_samples(out: &mut [u8]) {
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (i % 251) as u8;
    }
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
    write_reading(&mut writer, reading)?;
    Ok(writer.into_sink().written().len())
}

pub fn write_reading<S: Sink>(
    writer: &mut Writer<'_, S>,
    reading: i64,
) -> Result<()> {
    writer.start_element("reading")?;
    writer.text(Value::Int(reading))?;
    writer.end_element()?;
    writer.end_document()
}

pub fn decode(
    grammars: &Grammars<'_>,
    opts: &Options,
    bytes: &[u8],
) -> Result<i64> {
    let mut reader =
        Reader::<64>::new(ReaderConfig::new(grammars).options(opts.clone()))?;
    decode_reading_with(&mut reader, bytes)
}

pub fn decode_reading_with<const N: usize>(
    reader: &mut Reader<'_, N>,
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

pub fn encode_sensor(
    grammars: &Grammars<'_>,
    opts: &Options,
    temperature: i64,
    samples: &[u8],
    out: &mut [u8],
) -> Result<usize> {
    let cfg = WriterConfig::new(opts.clone(), grammars)
        .header(Header::Bare { cookie: false })
        .limits(Limits::device());
    let mut writer = Writer::new(SliceSink::new(out), cfg)?;
    write_sensor(&mut writer, temperature, samples)?;
    Ok(writer.into_sink().written().len())
}

pub fn write_sensor<S: Sink>(
    writer: &mut Writer<'_, S>,
    temperature: i64,
    samples: &[u8],
) -> Result<()> {
    writer.start_element((SENSOR_NS, "reading"))?;
    writer.start_element((SENSOR_NS, "temperature"))?;
    writer.event(&Event::Characters(Value::Int(temperature)))?;
    writer.end_element()?;
    writer.start_element((SENSOR_NS, "samples"))?;
    writer
        .event(&Event::Characters(Value::Binary(samples, BinaryKind::Base64)))?;
    writer.end_element()?;
    writer.end_element()?;
    writer.end_document()
}

pub fn decode_sensor_with<const N: usize>(
    reader: &mut Reader<'_, N>,
    mut input: &[u8],
    chunk: usize,
    expected_temp: i64,
    expected_samples: &[u8],
) -> Result<()> {
    let mut values = 0u8;
    loop {
        let n = reader.feed(&input[..input.len().min(chunk)]);
        input = &input[n..];
        if input.is_empty() {
            reader.end_input();
        }
        loop {
            match reader.next()? {
                Step::Event(Event::Characters(value), _) => {
                    match (values, value) {
                        (0, Value::Int(t)) if t == expected_temp => {}
                        (1, Value::Binary(data, _))
                            if data == expected_samples => {}
                        _ => {
                            return Err(Error::new(
                                ErrorKind::Value,
                                "unexpected sensor value",
                            ));
                        }
                    }
                    values += 1;
                }
                Step::Event(_, _) => {}
                Step::NeedInput => break,
                Step::End => {
                    if values != 2 || !input.is_empty() {
                        return Err(Error::new(
                            ErrorKind::Grammar,
                            "incomplete sensor reading",
                        ));
                    }
                    return Ok(());
                }
            }
        }
        if n == 0 || input.is_empty() {
            return Err(Error::new(
                ErrorKind::State,
                "reader stopped making progress",
            ));
        }
    }
}

/// Pre-encoded `.exig` bytes for `sensor_grammar()`, built once at startup
/// for benchmarking `GrammarSet::from_exig_with`.
#[cfg(feature = "extended")]
pub fn sensor_exig_bytes() -> Result<Vec<u8>> {
    sensor_grammar()?.to_exig()
}

/// Encodes `<reading>N</reading>` as UTF-8 XML text into `out`.
#[cfg(feature = "extended")]
pub fn encode_reading_xml(reading: i64, out: &mut [u8]) -> Result<usize> {
    let cfg = XmlWriterConfig::new().declaration(false);
    let mut w = XmlWriter::new(SliceSink::new(out), cfg);
    w.start_element("reading")?;
    w.text(Value::Int(reading))?;
    w.end_element()?;
    w.end_document()?;
    Ok(w.into_sink().written().len())
}

/// Decodes `<reading>N</reading>` from XML text with `XmlReader::<256>`.
#[cfg(feature = "extended")]
pub fn decode_reading_xml(
    grammars: &Grammars<'_>,
    bytes: &[u8],
) -> Result<i64> {
    let mut r = XmlReader::<256>::new(
        XmlConfig::new().limits(Limits::device()),
        grammars,
    );
    let mut rest = bytes;
    let mut reading = None;
    loop {
        let n = r.feed(rest);
        rest = &rest[n..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next()? {
                Step::Event(Event::Characters(Value::Str(s)), _) => {
                    reading = Some(s.parse::<i64>().map_err(|_| {
                        Error::new(ErrorKind::Value, "bad xml integer")
                    })?);
                }
                Step::Event(_, _) => {}
                Step::NeedInput => break,
                Step::End => {
                    return reading.ok_or_else(|| {
                        Error::new(ErrorKind::Grammar, "missing xml reading")
                    });
                }
            }
        }
    }
}

/// Encodes the sensor document as UTF-8 XML text into `out`.
#[cfg(feature = "extended")]
pub fn encode_sensor_xml(
    temperature: i64,
    samples: &[u8],
    out: &mut [u8],
) -> Result<usize> {
    let cfg = XmlWriterConfig::new().declaration(false);
    let mut w = XmlWriter::new(SliceSink::new(out), cfg);
    w.start_element((SENSOR_NS, "reading"))?;
    w.start_element((SENSOR_NS, "temperature"))?;
    w.event(&Event::Characters(Value::Int(temperature)))?;
    w.end_element()?;
    w.start_element((SENSOR_NS, "samples"))?;
    w.event(&Event::Characters(Value::Binary(samples, BinaryKind::Base64)))?;
    w.end_element()?;
    w.end_element()?;
    w.end_document()?;
    Ok(w.into_sink().written().len())
}

/// Decodes the XML sensor document and checks the temperature and base64
/// payload length/content.
#[cfg(feature = "extended")]
pub fn decode_sensor_xml(
    grammars: &Grammars<'_>,
    bytes: &[u8],
    expected_temp: i64,
    expected_b64: &str,
) -> Result<()> {
    let mut r = XmlReader::<256>::new(
        XmlConfig::new().limits(Limits::device()),
        grammars,
    );
    let mut rest = bytes;
    let mut field = 0u8;
    let mut checked = 0u8;
    loop {
        let n = r.feed(rest);
        rest = &rest[n..];
        if rest.is_empty() {
            r.end_input();
        }
        loop {
            match r.next()? {
                Step::Event(Event::StartElement(name), _) => {
                    field = if name.is(SENSOR_NS, "temperature") {
                        1
                    } else if name.is(SENSOR_NS, "samples") {
                        2
                    } else {
                        0
                    };
                }
                Step::Event(Event::Characters(Value::Str(s)), _) => {
                    match field {
                        1 => {
                            let t = s.parse::<i64>().map_err(|_| {
                                Error::new(ErrorKind::Value, "bad temp")
                            })?;
                            if t != expected_temp {
                                return Err(Error::new(
                                    ErrorKind::Value,
                                    "temp mismatch",
                                ));
                            }
                            checked += 1;
                        }
                        2 => {
                            if s != expected_b64 {
                                return Err(Error::new(
                                    ErrorKind::Value,
                                    "samples mismatch",
                                ));
                            }
                            checked += 1;
                        }
                        _ => {}
                    }
                }
                Step::Event(Event::EndElement(_), _) => field = 0,
                Step::Event(_, _) => {}
                Step::NeedInput => break,
                Step::End => {
                    if checked != 2 {
                        return Err(Error::new(
                            ErrorKind::Grammar,
                            "incomplete xml sensor",
                        ));
                    }
                    return Ok(());
                }
            }
        }
    }
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
        assert_eq!(&bytes[..len], b"\x80\0\0\x15");
        assert!(decode(&g, &opts, &bytes[..len - 1]).is_err());
        assert!(decode(&g, &opts, &bytes[..len + 1]).is_err());
        Ok(())
    }

    #[test]
    fn readings_reused_codecs() -> Result<()> {
        let set = grammar()?;
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let g = set.attach(&strings)?;
        let opts = options()?;
        let cfg = WriterConfig::new(opts.clone(), &g)
            .header(Header::Bare { cookie: false });
        let mut spare = alloc::vec::Vec::with_capacity(MAX_DOCUMENT);
        let mut reader =
            Reader::<64>::new(ReaderConfig::new(&g).options(opts.clone()))?;
        let mut writer =
            Writer::new(alloc::vec::Vec::with_capacity(MAX_DOCUMENT), cfg)?;
        for v in 0..10i64 {
            write_reading(&mut writer, v)?;
            spare.clear();
            spare = writer.reset(spare);
            assert_eq!(decode_reading_with(&mut reader, &spare)?, v);
            reader.reset();
        }
        Ok(())
    }

    #[test]
    fn sensor_byte_round_trip() -> Result<()> {
        let set = sensor_grammar()?;
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let g = set.attach(&strings)?;
        let opts = sensor_options(Coding::ByteAligned)?;
        let mut samples = [0u8; 64];
        fill_samples(&mut samples);
        let mut buf = [0u8; 128];
        let len = encode_sensor(&g, &opts, SENSOR_TEMPERATURE, &samples, &mut buf)?;
        let mut reader = Reader::<256>::new(
            ReaderConfig::new(&g).options(opts).limits(Limits::device()),
        )?;
        decode_sensor_with(&mut reader, &buf[..len], 32, SENSOR_TEMPERATURE, &samples)?;
        reader.reset();
        decode_sensor_with(&mut reader, &buf[..len], 1, SENSOR_TEMPERATURE, &samples)?;
        Ok(())
    }

    #[cfg(feature = "extended")]
    #[test]
    fn extended_bit_exig_and_xml() -> Result<()> {
        let exig = sensor_exig_bytes()?;
        let set = GrammarSet::from_exig_with(&exig, Limits::device())?;
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let g = set.attach(&strings)?;
        let opts = sensor_options(Coding::BitPacked)?;
        let mut samples = [0u8; 64];
        fill_samples(&mut samples);
        let mut buf = [0u8; 128];
        let len = encode_sensor(&g, &opts, SENSOR_TEMPERATURE, &samples, &mut buf)?;
        let mut reader = Reader::<256>::new(
            ReaderConfig::new(&g).options(opts).limits(Limits::device()),
        )?;
        decode_sensor_with(&mut reader, &buf[..len], 32, SENSOR_TEMPERATURE, &samples)?;

        // XML reading and sensor round trips.
        let mut xbuf = [0u8; 256];
        let xlen = encode_reading_xml(42, &mut xbuf)?;
        assert_eq!(decode_reading_xml(&g, &xbuf[..xlen])?, 42);

        let mut b64_buf = [0u8; 128];
        let expected_b64 = Value::Binary(&samples, BinaryKind::Base64)
            .write_lexical(&mut b64_buf)?;
        let sxlen = encode_sensor_xml(SENSOR_TEMPERATURE, &samples, &mut xbuf)?;
        decode_sensor_xml(&g, &xbuf[..sxlen], SENSOR_TEMPERATURE, expected_b64)?;
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
        let opts = options()?;
        let two_63 = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
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
