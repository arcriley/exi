//! XML token boundaries, character normalization and encoding
//! detection.
#![cfg(feature = "xml")]

use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader};
use exi::{ErrorKind, OwnedEvent, OwnedValue, Step};

include!("support/cases.rs");

fn events(input: &[u8], chunk: usize) -> exi::Result<Vec<OwnedEvent>> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let grammar = GrammarSet::schemaless();
    let grammars = grammar.attach(&strings)?;
    let cfg = XmlConfig::new()
        .report_pis(true)
        .report_comments(true)
        .report_doctype(true);
    let mut reader = XmlReader::<4096>::new(cfg, &grammars);
    let mut events = Vec::new();
    let mut rest = input;
    loop {
        let count = reader.feed(&rest[..rest.len().min(chunk)]);
        rest = &rest[count..];
        if rest.is_empty() {
            reader.end_input();
        }
        loop {
            match reader.next()? {
                Step::Event(e, _) => events.push(e.to_owned()),
                Step::End => return Ok(events),
                Step::NeedInput => {
                    assert!(count > 0 && !rest.is_empty(), "reader stalled");
                    break;
                }
            }
        }
    }
}

// Put the cut between left and right at the specified offset in the
// input.
fn split(left: &str, right: &str, boundary: usize) {
    let xml = format!("{left}{right}");
    let expected = events(xml.as_bytes(), 4096).unwrap();
    let padded = format!("{}{xml}", " ".repeat(boundary - left.len()));
    assert_eq!(events(padded.as_bytes(), boundary).unwrap(), expected);
}

macro_rules! splits {
    ($name:ident, $left:expr, $right:expr) => {
        mod $name {
            use super::*;
            cases!(split;
                before_ring($left, $right, 4094), last_byte($left, $right, 4095),
                at_wrap($left, $right, 4096), after_wrap($left, $right, 4097),
            );
        }
    };
}

splits!(element_name, "<name", "space/>");
splits!(attribute_name, "<r num", "ber='42'/>");
splits!(attribute_value, "<r n='one", " two'/>");
splits!(named_reference, "<r>&am", "p;</r>");
splits!(character_reference, "<r>&#x1F", "600;</r>");
splits!(end_cdata, "<r><![CDATA[a]]", "></r>");
splits!(end_comment, "<r><!--ok-", "-></r>");
splits!(end_pi, "<r><?p data?", "></r>");
splits!(line_end, "<r>one\r", "\ntwo</r>");

fn split_codepoint(c: char, boundary: usize) {
    let xml = format!("<r>{c}</r>");
    let expected = events(xml.as_bytes(), 4096).unwrap();
    for inside in 1..c.len_utf8() {
        let padded = format!("{}{xml}", " ".repeat(boundary - 3 - inside));
        assert_eq!(events(padded.as_bytes(), boundary).unwrap(), expected);
    }
}

macro_rules! utf8_splits {
    ($name:ident, $ch:expr) => {
        mod $name {
            use super::*;
            cases!(split_codepoint;
                before_ring($ch, 4094), last_byte($ch, 4095),
                at_wrap($ch, 4096), after_wrap($ch, 4097),
            );
        }
    };
}
utf8_splits!(two_bytes, 'é');
utf8_splits!(three_bytes, '界');
utf8_splits!(four_bytes, '😀');

fn rejects(xml: &str) {
    for chunk in [1, 3, 4096] {
        assert_eq!(
            events(xml.as_bytes(), chunk).unwrap_err().kind(),
            ErrorKind::Xml
        );
    }
}
cases!(rejects;
    wrong_end("<a></b>"), second_root("<a/><b/>"),
    preceding_text("outside<a/>"), following_text("<a/>outside"),
    repeated_attr("<a n='1' n='2'/>"),
    expanded_attr("<a xmlns:p='u' xmlns:q='u' p:n='1' q:n='2'/>"),
    attr_markup("<a n='<'/>"), text_cdata_close("<a>]]></a>"),
    comment_dash("<a><!--a--b--></a>"), misplaced_decl(" <?xml version='1.0'?><a/>"),
    undefined_entity("<a>&unknown;</a>"), null_reference("<a>&#0;</a>"),
    surrogate_reference("<a>&#xD800;</a>"), out_of_range_reference("<a>&#x110000;</a>"),
    unbound_element("<p:a/>"), unbound_attr("<a p:n='1'/>"),
    rebind_xml("<r xmlns:xml='u'/>"), rebind_xmlns("<r xmlns:xmlns='u'/>"),
    reserved_uri("<r xmlns:p='http://www.w3.org/2000/xmlns/'/>"),
    null_char("<a>\0</a>"), backspace("<a>\x08</a>"), vertical_tab("<a>\x0b</a>"),
    form_feed("<a>\x0c</a>"), shift_out("<a>\x0e</a>"), unit_separator("<a>\x1f</a>"),
    noncharacter_fffe("<a>\u{fffe}</a>"), noncharacter_ffff("<a>\u{ffff}</a>"),
);

fn normalized(xml: &str, expected: OwnedEvent) {
    for chunk in [1, 3, 4096] {
        let parsed = events(xml.as_bytes(), chunk).unwrap();
        assert!(parsed.contains(&expected), "{parsed:?}");
    }
}
cases!(normalized;
    text_cr("<r>a\rb</r>", OwnedEvent::Characters(OwnedValue::Str("a\nb".into()))),
    cdata_crlf("<r><![CDATA[a\r\nb]]></r>", OwnedEvent::Characters(OwnedValue::Str("a\nb".into()))),
    comment_cr("<!--a\rb--><r/>", OwnedEvent::Comment("a\nb".into())),
    pi_crlf("<r/><?p a\r\nb?>", OwnedEvent::ProcessingInstruction {target: "p".into(), data:"a\nb".into()}),
    attribute_space("<r a='\t\r\n\r'/>", OwnedEvent::Attribute(exi::OwnedName::new("","a"), OwnedValue::Str("   ".into()))),
    reference_space("<r a='&#9;&#10;&#13;'/>", OwnedEvent::Attribute(exi::OwnedName::new("","a"), OwnedValue::Str("\t\n\r".into()))),
);

fn utf16(little: bool, suffix: &[u16], valid: bool) {
    let mut bytes = Vec::new();
    for unit in [0xfeff]
        .into_iter()
        .chain("<r>".encode_utf16())
        .chain(suffix.iter().copied())
        .chain("</r>".encode_utf16())
    {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    for chunk in [1, 2, 3, 4096] {
        let parsed = events(&bytes, chunk);
        if valid {
            assert_eq!(
                parsed.unwrap(),
                events(b"<r>&#x1F600;</r>", 4096).unwrap()
            );
        } else {
            assert_eq!(parsed.unwrap_err().kind(), ErrorKind::Xml);
        }
    }
}
cases!(utf16;
    little_pair(true, &[0xd83d,0xde00], true), big_pair(false, &[0xd83d,0xde00], true),
    little_lone_high(true, &[0xd83d], false), big_lone_high(false, &[0xd83d], false),
    little_lone_low(true, &[0xde00], false), big_lone_low(false, &[0xde00], false),
    little_double_high(true, &[0xd83d,0xd83d], false), big_double_high(false, &[0xd83d,0xd83d], false),
);
