//! Wire vectors and split input for the two primitive coders.
use exi::coding::{
    Channel::Structure as S,
    Decoder, Encoder,
    bits::{BitDecoder, BitEncoder},
    byte_aligned::{ByteDecoder, ByteEncoder},
    datatypes::*,
};
use exi::header::{Sniff, parse_header, sniff};
use exi::{ErrorKind, Float, Limits};

include!("support/cases.rs");

fn uint(value: u64, expected: &[u8], packed: bool) {
    if packed {
        let mut enc = BitEncoder::new(Vec::new());
        enc.nbit(S, 5, 3).unwrap();
        write_uint(&mut enc, S, value).unwrap();
        enc.align().unwrap();
        let bytes = enc.into_sink();
        for split in 0..=bytes.len() {
            let mut dec =
                BitDecoder::split(&bytes[..split], &bytes[split..], 0);
            assert_eq!(dec.nbit(S, 3).unwrap(), 5);
            for octet in expected {
                assert_eq!(dec.nbit(S, 8).unwrap(), u32::from(*octet));
            }
            let mut dec =
                BitDecoder::split(&bytes[..split], &bytes[split..], 3);
            assert_eq!(
                read_uint(&mut dec, S, expected.len() as u32).unwrap(),
                value
            );
        }
    } else {
        let mut enc = ByteEncoder::new(Vec::new());
        write_uint(&mut enc, S, value).unwrap();
        let bytes = enc.into_sink();
        assert_eq!(bytes, expected);
        for split in 0..=bytes.len() {
            let mut dec = ByteDecoder::new(&bytes[..split], &bytes[split..]);
            assert_eq!(
                read_uint(&mut dec, S, expected.len() as u32).unwrap(),
                value
            );
            assert_eq!(dec.remaining(), 0);
        }
    }
}

macro_rules! unsigned_cases {
    ($module:ident, $packed:expr) => {
        mod $module {
            use super::*;
            cases!(uint;
                zero(0, &[0], $packed), one(1, &[1], $packed),
                seven_bits(127, &[127], $packed), eight_bits(128, &[128,1], $packed),
                max_byte(255, &[255,1], $packed), beyond_byte(256, &[128,2], $packed),
                fourteen_bits(16383, &[255,127], $packed), fifteen_bits(16384, &[128,128,1], $packed),
                max_word(65535, &[255,255,3], $packed), beyond_word(65536, &[128,128,4], $packed),
                max_dword(u32::MAX.into(), &[255,255,255,255,15], $packed),
                beyond_dword(1u64 << 32, &[128,128,128,128,16], $packed),
                max_qword(u64::MAX, &[255,255,255,255,255,255,255,255,255,1], $packed),
            );
        }
    };
}
unsigned_cases!(unsigned_byte, false);
unsigned_cases!(unsigned_bit, true);

fn reject_uint(bytes: &[u8], limit: u32, kind: ErrorKind) {
    for split in 0..=bytes.len() {
        let mut dec = ByteDecoder::new(&bytes[..split], &bytes[split..]);
        assert_eq!(read_uint(&mut dec, S, limit).unwrap_err().kind(), kind);
        let mut dec = BitDecoder::split(&bytes[..split], &bytes[split..], 0);
        assert_eq!(read_uint(&mut dec, S, limit).unwrap_err().kind(), kind);
    }
}
cases!(reject_uint;
    zero_limit(&[0], 0, ErrorKind::Limit), continuation_limit(&[128,1], 1, ErrorKind::Limit),
    missing_octet(&[], 10, ErrorKind::Truncated), missing_terminator(&[128], 10, ErrorKind::Truncated),
    tenth_octet_overflow(&[128,128,128,128,128,128,128,128,128,2], 10, ErrorKind::Value),
    eleventh_octet_overflow(&[128,128,128,128,128,128,128,128,128,128,1], 11, ErrorKind::Value),
);

fn binary(len: usize, offset: u8) {
    let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let mut enc = BitEncoder::new(Vec::new());
    enc.nbit(S, 0, offset).unwrap();
    write_binary(&mut enc, S, &data).unwrap();
    enc.align().unwrap();
    let bytes = enc.into_sink();
    for split in [0, bytes.len() / 2, bytes.len()] {
        let mut dec =
            BitDecoder::split(&bytes[..split], &bytes[split..], offset.into());
        assert_eq!(
            read_binary_len(&mut dec, S, 8, len as u64).unwrap(),
            len as u64
        );
        let mut out = vec![0; len];
        dec.octets(S, &mut out).unwrap();
        assert_eq!(out, data);
    }
}

macro_rules! binary_cases {
    ($module:ident, $offset:expr) => {
        mod $module {
            use super::*;
            cases!(binary;
                empty(0, $offset), one(1, $offset), two(2, $offset), three(3, $offset),
                before_copy_chunk(255, $offset), copy_chunk(256, $offset),
                after_copy_chunk(257, $offset), page(4096, $offset),
            );
        }
    };
}
binary_cases!(aligned, 0);
binary_cases!(shifted_one, 1);
binary_cases!(shifted_seven, 7);

fn float(value: Float) {
    let mut enc = ByteEncoder::new(Vec::new());
    write_float(&mut enc, S, value).unwrap();
    let bytes = enc.into_sink();
    for split in 0..=bytes.len() {
        let mut dec = ByteDecoder::new(&bytes[..split], &bytes[split..]);
        assert_eq!(read_float(&mut dec, S, 10).unwrap(), value);
    }
    for end in 0..bytes.len() {
        let mut dec = ByteDecoder::new(&bytes[..end], &[]);
        assert_eq!(
            read_float(&mut dec, S, 10).unwrap_err().kind(),
            ErrorKind::Truncated
        );
    }
}
cases!(float;
    min_exponent(Float { mantissa: i64::MIN, exponent: Float::MIN_EXPONENT }),
    max_exponent(Float { mantissa: i64::MAX, exponent: Float::MAX_EXPONENT }),
    zero_mantissa(Float { mantissa: 0, exponent: -9 }),
    subnormal_decimal_form(Float { mantissa: 5, exponent: -324 }),
    plus_inf(Float::INFINITY), minus_inf(Float::NEG_INFINITY), nan(Float::NAN),
);

fn header_error(bytes: &[u8], kind: ErrorKind) {
    assert_eq!(parse_header(bytes, &Limits::new()).unwrap_err().kind(), kind);
}
cases!(header_error;
    distinguishing_00(&[0x00], ErrorKind::Header), distinguishing_01(&[0x40], ErrorKind::Header),
    distinguishing_11(&[0xc0], ErrorKind::Header), invalid_cookie(&[b'$',b'E',b'X',b'X',0x80], ErrorKind::Header),
    invalid_after_cookie(b"$EXI\0", ErrorKind::Header),
    preview(&[0x90], ErrorKind::Header), preview_after_cookie(b"$EXI\x90", ErrorKind::Header),
    version_two(&[0x81], ErrorKind::Unsupported), version_fifteen(&[0x8e], ErrorKind::Unsupported),
    version_sixteen(&[0x8f,0], ErrorKind::Unsupported), version_thirty_one(&[0x8f,0xf0], ErrorKind::Unsupported),
);

fn partial_header(bytes: &[u8]) {
    assert!(parse_header(bytes, &Limits::new()).unwrap().is_none());
}
cases!(partial_header;
    no_header(b""), dollar(b"$"), cookie_two(b"$E"), cookie_three(b"$EX"),
    cookie_only(b"$EXI"), split_version(&[0x8f]), split_long_version(&[0x8f,0xff]),
);

#[test]
fn bare_cookie() {
    assert_eq!(sniff(b"$EXI\x80"), Sniff::Exi { cookie: true });
    let header = parse_header(b"$EXI\x80", &Limits::new()).unwrap().unwrap();
    assert!(header.cookie && header.options.is_none());
    assert_eq!(header.body_offset_bits, 40);
}
