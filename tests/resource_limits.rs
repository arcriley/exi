//! Public XML and EXI resource limits at the accepted and rejected
//! boundary.

use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{ErrorKind, Limits};

include!("support/cases.rs");

#[cfg(feature = "xml")]
fn xml_limit(input: &[u8], limits: Limits, accepted: bool) {
    use exi::Step;
    use exi::xml::{XmlConfig, XmlReader};

    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let mut reader = XmlReader::<64>::new(XmlConfig::new().limits(limits), &g);
    let mut rest = input;
    loop {
        let fed = reader.feed(rest);
        rest = &rest[fed..];
        if rest.is_empty() {
            reader.end_input();
        }
        loop {
            match reader.next() {
                Ok(Step::Event(..)) => {}
                Ok(Step::NeedInput) => {
                    assert!(fed > 0 && !rest.is_empty());
                    break;
                }
                Ok(Step::End) => {
                    assert!(accepted, "document exceeded a limit");
                    return;
                }
                Err(error) => {
                    assert!(!accepted, "{error}");
                    assert_eq!(error.kind(), ErrorKind::Limit);
                    return;
                }
            }
        }
    }
}

#[cfg(feature = "xml")]
fn depth(depth: u32) {
    let mut doc = "<a>".repeat(depth as usize);
    doc.push_str(&"</a>".repeat(depth as usize));
    xml_limit(doc.as_bytes(), Limits::new().max_depth(depth), true);
    xml_limit(doc.as_bytes(), Limits::new().max_depth(depth - 1), false);
}
#[cfg(feature = "xml")]
cases!(depth; depth_200(200), depth_1000(1000), depth_5000(5000));
#[cfg(feature = "xml")]
cases!(xml_limit;
    single_attribute(b"<a x='1'/>", Limits::new().max_attributes(1), true),
    excess_attribute(b"<a x='1' y='2'/>", Limits::new().max_attributes(1), false),
    unicode_value_at_limit("<a>é😀</a>".as_bytes(), Limits::new().max_value_chars(2), true),
    unicode_value_over_limit("<a>é😀x</a>".as_bytes(), Limits::new().max_value_chars(2), false),
    cdata_over_limit(b"<a><![CDATA[abc]]>de</a>", Limits::new().max_value_chars(4), false),
    zero_depth(b"<a/>", Limits::new().max_depth(0), false),
);

fn exi_limit(limits: Limits, detail: &str) {
    use exi::{
        Coding, EventSink, Header, Options, Reader, ReaderConfig, Writer,
        WriterConfig,
    };
    let strings = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings).unwrap();
    let options =
        Options::builder().coding(Coding::ByteAligned).build().unwrap();
    let mut writer = Writer::new(
        Vec::new(),
        WriterConfig::new(options.clone(), &g)
            .header(Header::Bare { cookie: false }),
    )
    .unwrap();
    writer.start_element("reading").unwrap();
    writer.attribute("unit", "celsius").unwrap();
    writer.start_element("sample").unwrap();
    writer.text("12").unwrap();
    writer.end_element().unwrap();
    writer.end_element().unwrap();
    writer.end_document().unwrap();
    let mut reader = Reader::<64>::new(
        ReaderConfig::new(&g).options(options).limits(limits),
    )
    .unwrap();
    let error =
        reader.decode_all(&writer.into_sink(), &mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Limit);
    assert!(error.detail().contains(detail), "{error}");
}
cases!(exi_limit;
    exi_depth(Limits::new().max_depth(1), "depth"),
    exi_names(Limits::new().max_names(1), "name"),
    exi_values(Limits::new().max_value_chars(3), "value"),
);
