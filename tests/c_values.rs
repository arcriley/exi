//! Values through the C DOM, including flags and the lexical form
//! visible to C.
#![cfg(feature = "capi")]

use exi::capi::{dom::*, *};
use exi::dom::{Document, raw::RawNode};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{
    BinaryKind, DateTime, DateTimeKind, Decimal, ErrorKind, Float, ListRef,
    Name, Value,
};

include!("support/cases.rs");

fn round_trip(value: Value<'_>, tag: u32, flags: u32, lexical: &str) {
    let mut buffer = vec![0; lexical.len()];
    assert_eq!(value.write_lexical(&mut buffer).unwrap(), lexical);
    if !buffer.is_empty() {
        buffer.pop();
        assert_eq!(
            value.write_lexical(&mut buffer).unwrap_err().kind(),
            ErrorKind::BufferFull
        );
    }
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let mut doc = Document::new(&strings);
    let input = doc.new_value(value.to_owned());
    let output = doc.new_text("before");
    let src = doc.raw(input).cast::<RawNode>();
    let dst = doc.raw(output).cast::<RawNode>().cast_mut();
    let raw = doc.into_raw();
    // SAFETY: both nodes belong to raw. The C view of src lives through
    // the setter on dst. Take back ownership after the C calls finish.
    let doc = unsafe {
        let mut view = std::mem::MaybeUninit::uninit();
        assert_eq!(exiNodeGetValue(src, view.as_mut_ptr()), EXI_OK);
        let view = view.assume_init();
        assert_eq!((view.r#type, view.flags), (tag, flags));
        assert_eq!(exiNodeSetValue(dst, &view), EXI_OK);
        Document::from_raw(raw)
    };
    assert_eq!(doc.value(output), Some(value));
    assert_eq!(doc.content(output), lexical);
}

cases!(round_trip;
    empty_text(Value::Str(""), EXI_V_STRING, 0, ""),
    unicode_text(Value::Str("aé界😀"), EXI_V_STRING, 0, "aé界😀"),
    truth(Value::Bool(true), EXI_V_BOOL, 0, "true"),
    falsehood(Value::Bool(false), EXI_V_BOOL, 0, "false"),
    digit_zero(Value::BoolDigit(false), EXI_V_BOOL, EXI_VF_BOOL_DIGIT, "0"),
    digit_one(Value::BoolDigit(true), EXI_V_BOOL, EXI_VF_BOOL_DIGIT, "1"),
    min_signed(Value::Int(i64::MIN), EXI_V_INT, 0, "-9223372036854775808"),
    large_integer(Value::BigInt("9223372036854775808"), EXI_V_INT, EXI_VF_TEXT_ONLY, "9223372036854775808"),
    negative_decimal(Value::Decimal(Decimal::new(true, 7, 5, 2)), EXI_V_DECIMAL, 0, "-7.05"),
    large_decimal(Value::BigDecimal("18446744073709551616.5"), EXI_V_DECIMAL, EXI_VF_TEXT_ONLY, "18446744073709551616.5"),
    float_trailing_zeros(Value::Float(Float { mantissa: 1200, exponent: -2 }), EXI_V_FLOAT, 0, "1.2E1"),
    infinity(Value::Float(Float::INFINITY), EXI_V_FLOAT, 0, "INF"),
    minus_infinity(Value::Float(Float::NEG_INFINITY), EXI_V_FLOAT, 0, "-INF"),
    not_a_number(Value::Float(Float::NAN), EXI_V_FLOAT, 0, "NaN"),
    empty_binary(Value::Binary(&[], BinaryKind::Base64), EXI_V_BINARY, 0, ""),
    one_octet(Value::Binary(&[0xff], BinaryKind::Base64), EXI_V_BINARY, 0, "/w=="),
    two_octets(Value::Binary(&[0xff,0], BinaryKind::Base64), EXI_V_BINARY, 0, "/wA="),
    three_octets(Value::Binary(&[0xff,0,1], BinaryKind::Base64), EXI_V_BINARY, 0, "/wAB"),
    hex_octets(Value::Binary(&[0x0f,0xaa], BinaryKind::Hex), EXI_V_BINARY, EXI_VF_HEX, "0FAA"),
    local_qname(Value::QName(Name::new("", "name")), EXI_V_QNAME, 0, "name"),
    prefixed_qname(Value::QName(Name::new("urn:test", "name").with_prefix("p")), EXI_V_QNAME, 0, "p:name"),
    enumeration(Value::Enum { ordinal: 2, text: "blue" }, EXI_V_ENUM, 0, "blue"),
    empty_list(Value::List(ListRef::Values(&[])), EXI_V_LIST, 0, ""),
    mixed_list(Value::List(ListRef::Values(&[Value::Int(2),Value::Bool(true)])), EXI_V_LIST, 0, "2 true"),
);

fn calendar(kind: DateTimeKind, zone: Option<i16>, form: &str) {
    let value = DateTime {
        kind,
        year: -12,
        month: 2,
        day: 29,
        hour: 0,
        minute: 1,
        second: 2,
        fraction: Some((50, 3)),
        tz_minutes: zone,
    };
    round_trip(Value::DateTime(value), EXI_V_DATETIME, 0, form);
}

macro_rules! calendar_cases {
    ($name:ident, $kind:ident, $form:literal) => {
        mod $name {
            use super::*;
            use DateTimeKind::$kind;
            cases!(calendar;
                local($kind, None, $form),
                utc($kind, Some(0), concat!($form, "Z")),
                west($kind, Some(-840), concat!($form, "-14:00")),
                east($kind, Some(840), concat!($form, "+14:00")),
            );
        }
    };
}
calendar_cases!(year, GYear, "-0012");
calendar_cases!(year_month, GYearMonth, "-0012-02");
calendar_cases!(date, Date, "-0012-02-29");
calendar_cases!(datetime, DateTime, "-0012-02-29T00:01:02.05");
calendar_cases!(month, GMonth, "--02");
calendar_cases!(month_day, GMonthDay, "--02-29");
calendar_cases!(day, GDay, "---29");
calendar_cases!(time, Time, "00:01:02.05");
