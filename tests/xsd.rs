// tests/xsd.rs
//
// XML Schema compiler tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The XSD compiler on small schemas written here, so nothing needs `fixtures/w3c`:
//! documents and namespaces, the content models a strict stream accepts, and the
//! representation each simple type gets (the table in `exi::xsd`).
#![cfg(feature = "xsd")]

use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::xml::{XmlConfig, XmlReader, XmlWriter, XmlWriterConfig};
use exi::xsd::XsdCompiler;
use exi::{Coding, ErrorKind, Options, Reader, ReaderConfig, Status, Step, Writer, WriterConfig};

/// `xs:schema` with the XSD namespace bound to `xs`, these attributes, and this content.
fn schema(attrs: &str, body: &str) -> Vec<u8> {
    format!(r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" {attrs}>{body}</xs:schema>"#)
        .into_bytes()
}

/// Whether `xml` can be written to a strict stream with these grammars.
fn fits(set: &GrammarSet, xml: &str) -> bool {
    let s = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let g = set.attach(&s).unwrap();
    let opts = Options::builder()
        .coding(Coding::ByteAligned)
        .strict(true)
        .build()
        .unwrap();
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, &g)).unwrap();
    let mut x: XmlReader = XmlReader::new(XmlConfig::new(), &g);
    let mut rest = xml.as_bytes();
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        match exi::transcode(&mut x, &mut w) {
            Ok(Status::End) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

/// The grammars in the XML form of `.exig`, as `exi exig` prints it.
fn exig_text(set: &GrammarSet) -> String {
    let exig = set.to_exig().unwrap();
    let gs = exi::grammar::exig::exig_grammars().unwrap();
    let s = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let g = gs.attach(&s).unwrap();
    let mut r: Reader = Reader::new(ReaderConfig::new(&g)).unwrap();
    r.feed(&exig);
    r.end_input();
    let mut w = XmlWriter::new(Vec::new(), XmlWriterConfig::new());
    exi::transcode(&mut r, &mut w).unwrap();
    String::from_utf8(w.into_sink()).unwrap()
}

/// That text read back without a schema, so every value is a string.
fn exig_xml(set: &GrammarSet) -> Vec<exi::OwnedEvent> {
    let s = GlobalStrings::new(GlobalConfig::new()).unwrap();
    let none = GrammarSet::schemaless();
    let g = none.attach(&s).unwrap();
    let mut x: XmlReader = XmlReader::new(XmlConfig::new(), &g);
    let text = exig_text(set);
    let mut rest = text.as_bytes();
    let mut out = Vec::new();
    loop {
        rest = &rest[x.feed(rest)..];
        if rest.is_empty() {
            x.end_input();
        }
        loop {
            match x.next().unwrap() {
                Step::Event(e, _) => out.push(e.to_owned()),
                Step::NeedInput => break,
                Step::End => return out,
            }
        }
    }
}

/// A string value's text.
fn text(v: &exi::OwnedValue) -> String {
    match v {
        exi::OwnedValue::Str(s) => s.to_string(),
        v => panic!("not text: {v:?}"),
    }
}

/// The representation of the named simple type `local` in the schema's one target
/// namespace, as its element name in the `.exig` XML form (`boundedInteger`, `string`…).
fn rep(set: &GrammarSet, local: &str) -> String {
    use exi::OwnedEvent::*;
    let evs = exig_xml(set);
    let attr = |i: usize, name: &str| -> Option<String> {
        evs[i + 1..]
            .iter()
            .take_while(|e| matches!(e, Attribute(..)))
            .find_map(|e| match e {
                Attribute(n, v) if n.as_name().local_name() == name => {
                    Some(text(v))
                }
                _ => None,
            })
    };
    let start = |e: &exi::OwnedEvent| match e {
        StartElement(n) => Some(n.as_name().local_name().to_string()),
        _ => None,
    };
    // Local names of the first URI after the built-in four, in order; simple types in
    // order; then the named type whose name is (4, local).
    let mut locals = Vec::new();
    let mut simple = Vec::new();
    let (mut in_uri, mut in_simple, mut depth) = (0, false, 0);
    for (i, e) in evs.iter().enumerate() {
        match (start(e).as_deref(), e) {
            (Some("uri"), _) => in_uri += 1,
            (Some("local"), _) if in_uri == 1 => {
                if let Some(Characters(v)) = evs.get(i + 1) {
                    locals.push(text(v));
                }
            }
            (Some("simpleTypes"), _) => in_simple = true,
            (Some(n), _) if in_simple => {
                if depth == 0 {
                    simple.push(n.to_string());
                }
                depth += 1;
            }
            (_, EndElement(n)) if in_simple => {
                if n.as_name().local_name() == "simpleTypes" {
                    in_simple = false;
                } else {
                    depth -= 1;
                }
            }
            _ => {}
        }
    }
    let want = locals.iter().position(|l| l == local).expect("a local name").to_string();
    for (i, e) in evs.iter().enumerate() {
        if start(e).as_deref() == Some("type")
            && attr(i, "uri").as_deref() == Some("4")
            && attr(i, "local").as_deref() == Some(want.as_str())
        {
            let scalar = evs[i..]
                .iter()
                .position(|e| start(e).as_deref() == Some("scalar"))
                .unwrap();
            let n: usize = attr(i + scalar, "type").unwrap().parse().unwrap();
            return simple[n].clone();
        }
    }
    panic!("no type {local}")
}

#[test]
fn integer_and_boolean_representations() {
    let restrict = |name: &str, base: &str, facets: &str| {
        format!(
            r#"<xs:simpleType name="{name}"><xs:restriction base="xs:{base}">{facets}</xs:restriction></xs:simpleType>"#
        )
    };
    let range = |lo: i64, hi: i64| {
        format!(r#"<xs:minInclusive value="{lo}"/><xs:maxInclusive value="{hi}"/>"#)
    };
    let body = [
        restrict("v4096", "int", &range(0, 4095)), // 4096 values: n-bit
        restrict("v4097", "int", &range(0, 4096)), // one more: unsigned
        restrict("shifted", "int", &range(-2048, 2047)), // 4096 values below zero
        restrict("shifted1", "int", &range(-2049, 2047)), // one more: signed
        restrict("plus", "int", r#"<xs:minInclusive value="+10"/><xs:maxInclusive value="+20"/>"#),
        restrict("wide", "int", &range(-5000, 0)), // signed
        restrict("excl", "int", r#"<xs:minExclusive value="-1"/><xs:maxExclusive value="4096"/>"#),
        restrict("plain", "boolean", ""),
        restrict("pat", "boolean", r#"<xs:pattern value="true|false"/>"#),
    ]
    .concat();
    let set = GrammarSet::from_xsd(&schema(r#"targetNamespace="urn:t""#, &body)).unwrap();
    for (name, want) in [
        ("v4096", "boundedInteger"),
        ("v4097", "unsignedInteger"),
        ("shifted", "boundedInteger"),
        ("shifted1", "integer"),
        ("plus", "boundedInteger"), // a leading + is part of the lexical form
        ("wide", "integer"),
        ("excl", "boundedInteger"),
        ("plain", "boolean"),
        ("pat", "booleanPattern"),
    ] {
        assert_eq!(rep(&set, name), want, "{name}");
    }
}

#[test]
fn documents_and_namespaces() {
    let main = schema(
        r#"targetNamespace="urn:m" xmlns:m="urn:m""#,
        r#"<xs:include schemaLocation="parts/c.xsd"/><xs:element name="root" type="m:C"/>"#,
    );
    // No target namespace: a chameleon, which takes the includer's.
    let chameleon = schema(
        "",
        r#"<xs:complexType name="C"><xs:sequence><xs:element name="x" type="xs:string"/></xs:sequence></xs:complexType>"#,
    );
    let set = XsdCompiler::new()
        .document("s/main.xsd", &main)
        .document("s/parts/c.xsd", &chameleon)
        .compile("s/main.xsd")
        .unwrap();
    assert!(set.name_id("urn:m", "C").is_some());
    // Its local element is unqualified: in no namespace.
    assert!(fits(&set, r#"<m:root xmlns:m="urn:m"><x>1</x></m:root>"#));
    assert!(!fits(&set, r#"<root xmlns="urn:m"><x>1</x></root>"#));

    // An included document with another target namespace.
    let other = schema(r#"targetNamespace="urn:other""#, r#"<xs:complexType name="C"/>"#);
    let e = XsdCompiler::new()
        .document("s/main.xsd", &main)
        .document("s/parts/c.xsd", &other)
        .compile("s/main.xsd")
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Schema, "{e}");

    // Cycles of includes and of imports end.
    let a = schema(r#"targetNamespace="urn:m""#, r#"<xs:include schemaLocation="b.xsd"/><xs:element name="a"/>"#);
    let b = schema(r#"targetNamespace="urn:m""#, r#"<xs:include schemaLocation="a.xsd"/><xs:element name="b"/>"#);
    let set = XsdCompiler::new().document("a.xsd", &a).document("b.xsd", &b).compile("a.xsd").unwrap();
    assert!(set.name_id("urn:m", "a").is_some() && set.name_id("urn:m", "b").is_some());
    let a = schema(
        r#"targetNamespace="urn:a""#,
        r#"<xs:import namespace="urn:b" schemaLocation="b.xsd"/><xs:element name="a"/>"#,
    );
    let b = schema(
        r#"targetNamespace="urn:b""#,
        r#"<xs:import namespace="urn:a" schemaLocation="a.xsd"/><xs:element name="b"/>"#,
    );
    let set = XsdCompiler::new().document("a.xsd", &a).document("b.xsd", &b).compile("a.xsd").unwrap();
    assert!(set.name_id("urn:b", "b").is_some());

    // An import without a location finds a document by its namespace.
    let a = schema(
        r#"targetNamespace="urn:a" xmlns:b="urn:b""#,
        r#"<xs:import namespace="urn:b"/><xs:element name="a" type="b:T"/>"#,
    );
    let b = schema(r#"targetNamespace="urn:b""#, r#"<xs:complexType name="T"/>"#);
    let set = XsdCompiler::new()
        .document("a.xsd", &a)
        .document("elsewhere/b.xsd", &b)
        .compile("a.xsd")
        .unwrap();
    assert!(set.name_id("urn:b", "T").is_some());

    for construct in ["redefine", "override"] {
        let m = schema("", &format!(r#"<xs:{construct} schemaLocation="c.xsd"/>"#));
        let e = XsdCompiler::new()
            .document("m.xsd", &m)
            .document("c.xsd", &chameleon)
            .compile("m.xsd")
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Unsupported, "{construct}: {e}");
    }
}

/// Derivation by extension and restriction, of simple and complex content.
#[test]
fn derived_types() {
    let set = GrammarSet::from_xsd(&schema(
        "",
        r#"
        <xs:complexType name="Base">
          <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
          <xs:attribute name="id" type="xs:int"/>
          <xs:attribute name="note" type="xs:string"/>
        </xs:complexType>
        <xs:complexType name="Longer"><xs:complexContent><xs:extension base="Base">
          <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
        </xs:extension></xs:complexContent></xs:complexType>
        <xs:complexType name="Quiet"><xs:complexContent><xs:restriction base="Base">
          <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
          <xs:attribute name="note" use="prohibited"/>
        </xs:restriction></xs:complexContent></xs:complexType>
        <xs:complexType name="Price"><xs:simpleContent><xs:extension base="xs:decimal">
          <xs:attribute name="cur" type="xs:string" use="required"/>
        </xs:extension></xs:simpleContent></xs:complexType>
        <xs:complexType name="Euro"><xs:simpleContent><xs:restriction base="Price">
          <xs:maxInclusive value="100"/>
        </xs:restriction></xs:simpleContent></xs:complexType>
        <xs:element name="longer" type="Longer"/>
        <xs:element name="quiet" type="Quiet"/>
        <xs:element name="price" type="Price"/>
        <xs:element name="euro" type="Euro"/>
        "#,
    ))
    .unwrap();
    // Extension appends to the base's content and keeps its attributes.
    assert!(fits(&set, r#"<longer id="1" note="n"><a>x</a><b>y</b></longer>"#));
    assert!(!fits(&set, r#"<longer><b>y</b></longer>"#));
    // Restriction can drop an attribute.
    assert!(fits(&set, r#"<quiet id="1"><a>x</a></quiet>"#));
    assert!(!fits(&set, r#"<quiet note="n"><a>x</a></quiet>"#));
    // Simple content: typed text with attributes.
    assert!(fits(&set, r#"<price cur="EUR">9.50</price>"#));
    assert!(!fits(&set, r#"<price>9.50</price>"#)); // cur is required
    assert!(!fits(&set, r#"<price cur="EUR">nine</price>"#));
    assert!(fits(&set, r#"<euro cur="EUR">9.50</euro>"#));
}

/// The content models a strict stream accepts.
#[test]
fn content_models() {
    let set = GrammarSet::from_xsd(&schema(
        r#"targetNamespace="urn:t" xmlns:t="urn:t" elementFormDefault="qualified""#,
        r###"
        <xs:group name="pair"><xs:sequence>
          <xs:element name="k" type="xs:string"/><xs:element name="v" type="xs:string"/>
        </xs:sequence></xs:group>
        <xs:attributeGroup name="common">
          <xs:attribute name="id" type="xs:int"/><xs:attribute name="lang" type="xs:string"/>
        </xs:attributeGroup>
        <xs:element name="all"><xs:complexType><xs:all>
          <xs:element name="x" type="xs:int"/><xs:element name="y" type="xs:int" minOccurs="0"/>
        </xs:all></xs:complexType></xs:element>
        <xs:element name="grouped"><xs:complexType>
          <xs:sequence><xs:group ref="t:pair" maxOccurs="2"/></xs:sequence>
          <xs:attributeGroup ref="t:common"/>
        </xs:complexType></xs:element>
        <xs:element name="mixed"><xs:complexType mixed="true">
          <xs:sequence><xs:element name="b" type="xs:string" minOccurs="0"/></xs:sequence>
        </xs:complexType></xs:element>
        <xs:element name="other"><xs:complexType>
          <xs:sequence><xs:any namespace="##other" processContents="skip"/></xs:sequence>
          <xs:anyAttribute namespace="urn:x" processContents="skip"/>
        </xs:complexType></xs:element>
        <xs:element name="local"><xs:complexType>
          <xs:sequence><xs:any namespace="##local ##targetNamespace" processContents="skip"/></xs:sequence>
        </xs:complexType></xs:element>
        <xs:element name="head" type="xs:string"/>
        <xs:element name="mid" type="xs:string" substitutionGroup="t:head"/>
        <xs:element name="leaf" type="xs:string" substitutionGroup="t:mid"/>
        <xs:element name="holder"><xs:complexType>
          <xs:sequence><xs:element ref="t:head" maxOccurs="unbounded"/></xs:sequence>
        </xs:complexType></xs:element>
        "###,
    ))
    .unwrap();
    let t =|body: &str| format!(r#"<{body}"#).replace("NS", r#"xmlns="urn:t""#);
    // xs:all: any order, optional members may be left out.
    assert!(fits(&set, &t(r#"all NS><y>2</y><x>1</x></all>"#)));
    assert!(fits(&set, &t(r#"all NS><x>1</x></all>"#)));
    // Groups by reference.
    assert!(fits(&set, &t(r#"grouped NS id="1" lang="en"><k>a</k><v>b</v><k>c</k><v>d</v></grouped>"#)));
    assert!(!fits(&set, &t(r#"grouped NS><k>a</k><k>c</k></grouped>"#)));
    assert!(!fits(&set, &t(r#"grouped NS other="1"><k>a</k><v>b</v></grouped>"#)));
    // Mixed content: text anywhere.
    assert!(fits(&set, &t(r#"mixed NS>one<b>two</b>three</mixed>"#)));
    // Wildcards. EXI makes ##any and ##other one SE(*) (§8.5.4.1.6): a strict stream
    // does not validate, so any element fits. A list of URIs is SE(uri:*) for each, and
    // AT(uri:*) for attributes.
    assert!(fits(&set, &t(r#"other NS xmlns:x="urn:x" x:a="1"><x:e/></other>"#)));
    assert!(fits(&set, &t(r#"other NS><e/></other>"#)));
    assert!(!fits(&set, &t(r#"other NS xmlns:y="urn:y" y:a="1"><y:e/></other>"#)));
    assert!(fits(&set, &t(r#"local NS><e xmlns=""/></local>"#)));
    assert!(fits(&set, &t(r#"local NS><e/></local>"#)));
    assert!(!fits(&set, &t(r#"local NS><y:e xmlns:y="urn:y"/></local>"#)));
    // Substitution groups are transitive: leaf stands in for mid, which stands in for head.
    assert!(fits(&set, &t(r#"holder NS><head>a</head><mid>b</mid><leaf>c</leaf></holder>"#)));
}

/// The XML form `exi exig` prints, written as text: a quick look at a compiled schema.
#[test]
fn compiled_schemas_have_an_xml_form() {
    let set = GrammarSet::from_xsd(&schema(
        r#"targetNamespace="urn:t""#,
        r#"<xs:element name="e" type="xs:int"/>"#,
    ))
    .unwrap();
    let text = exig_text(&set);
    assert!(text.starts_with("<grammars "), "{text}");
    assert!(text.contains(r#"<uri value="urn:t"><local>e</local></uri>"#), "{text}");
}

#[test]
fn schema_errors() {
    let compile = |body: &str| GrammarSet::from_xsd(&schema("", body));
    let not_schema = br#"<xs:element xmlns:xs="http://www.w3.org/2001/XMLSchema" name="a"/>"#;
    assert_eq!(GrammarSet::from_xsd(not_schema).unwrap_err().kind(), ErrorKind::Schema);
    assert_eq!(GrammarSet::from_xsd(b"<xs:schema").unwrap_err().kind(), ErrorKind::Xml);
    for body in [
        r#"<xs:element type="xs:int"/>"#,           // a global without a name
        r#"<xs:complexType/>"#,
        r#"<xs:element name="a" type="nope:T"/>"#, // an undeclared prefix
        r#"<xs:element name="a" type="Missing"/>"#,
        r#"<xs:element name="a"><xs:complexType><xs:sequence><xs:element ref="b"/></xs:sequence></xs:complexType></xs:element>"#,
        // A complex type where a simple type is needed.
        r#"<xs:complexType name="C"/><xs:element name="e"><xs:complexType><xs:attribute name="a" type="C"/></xs:complexType></xs:element>"#,
        r#"<xs:complexType name="C"/><xs:simpleType name="S"><xs:restriction base="C"/></xs:simpleType>"#,
    ] {
        let e = compile(body).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Schema, "{body}: {e}");
    }
}

/// `xsi:type` selects a type derived from the declared one.
#[test]
fn xsi_type_names_a_derived_type() {
    let set = GrammarSet::from_xsd(&schema(
        "",
        r#"<xs:complexType name="Base"><xs:sequence><xs:element name="a"/></xs:sequence></xs:complexType>
           <xs:complexType name="Derived"><xs:complexContent><xs:extension base="Base">
             <xs:sequence><xs:element name="b"/></xs:sequence>
           </xs:extension></xs:complexContent></xs:complexType>
           <xs:element name="e" type="Base"/>"#,
    ))
    .unwrap();
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    assert!(fits(&set, &format!(r#"<e {xsi} xsi:type="Derived"><a/><b/></e>"#)));
    assert!(!fits(&set, "<e><a/><b/></e>"));
}

/// A member of a substitution group without a type takes the head's.
#[test]
fn substitution_members_inherit_the_head_type() {
    let set = GrammarSet::from_xsd(&schema(
        "",
        r#"<xs:complexType name="T"><xs:attribute name="v" use="required"/></xs:complexType>
           <xs:element name="head" type="T"/>
           <xs:element name="mid" substitutionGroup="head"/>
           <xs:element name="leaf" substitutionGroup="mid"/>
           <xs:element name="r"><xs:complexType><xs:sequence>
             <xs:element ref="head" maxOccurs="unbounded"/>
           </xs:sequence></xs:complexType></xs:element>"#,
    ))
    .unwrap();
    assert!(fits(&set, r#"<r><head v="1"/><leaf v="3"/></r>"#));
    assert!(!fits(&set, "<r><leaf/></r>")); // T needs v
}

#[test]
fn element_and_attribute_forms() {
    let set = GrammarSet::from_xsd(&schema(
        r#"targetNamespace="urn:f""#,
        r#"<xs:element name="r"><xs:complexType><xs:sequence>
             <xs:element name="u"/><xs:element name="q" form="qualified"/>
           </xs:sequence>
           <xs:attribute name="a"/><xs:attribute name="b" form="qualified"/>
           </xs:complexType></xs:element>"#,
    ))
    .unwrap();
    assert!(set.name_id("", "u").is_some() && set.name_id("urn:f", "u").is_none());
    assert!(set.name_id("urn:f", "q").is_some());
    assert!(set.name_id("", "a").is_some() && set.name_id("urn:f", "b").is_some());
    assert!(fits(&set, r#"<f:r xmlns:f="urn:f" a="1" f:b="2"><u/><f:q/></f:r>"#));
    assert!(!fits(&set, r#"<f:r xmlns:f="urn:f"><f:u/><f:q/></f:r>"#));

    let qualified = GrammarSet::from_xsd(&schema(
        r#"targetNamespace="urn:f" elementFormDefault="qualified" attributeFormDefault="qualified""#,
        r#"<xs:element name="r"><xs:complexType><xs:sequence><xs:element name="u"/></xs:sequence>
           <xs:attribute name="a"/></xs:complexType></xs:element>"#,
    ))
    .unwrap();
    assert!(qualified.name_id("urn:f", "u").is_some());
    assert!(qualified.name_id("urn:f", "a").is_some());
}

#[test]
fn enumerations_on_special_bases() {
    let set = GrammarSet::from_xsd(&schema(
        r#"targetNamespace="urn:t" xmlns:t="urn:t""#,
        r#"<xs:simpleType name="U"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType>
           <xs:simpleType name="L"><xs:list itemType="xs:int"/></xs:simpleType>
           <xs:simpleType name="onInt"><xs:restriction base="xs:int">
             <xs:enumeration value="1"/><xs:enumeration value="2"/></xs:restriction></xs:simpleType>
           <xs:simpleType name="onUnion"><xs:restriction base="t:U">
             <xs:enumeration value="1"/><xs:enumeration value="2"/></xs:restriction></xs:simpleType>
           <xs:simpleType name="onQName"><xs:restriction base="xs:QName">
             <xs:enumeration value="t:a"/></xs:restriction></xs:simpleType>
           <xs:simpleType name="onList"><xs:restriction base="t:L">
             <xs:enumeration value="1 2"/></xs:restriction></xs:simpleType>"#,
    ))
    .unwrap();
    for (name, want) in [
        ("onInt", "enumeration"),
        ("onUnion", "string"),
        ("onQName", "string"),
        ("onList", "list"),
    ] {
        assert_eq!(rep(&set, name), want, "{name}");
    }
}

/// An enumeration restricted again has only its own values.
#[test]
fn enumeration_restricted_again() {
    let set = GrammarSet::from_xsd(&schema(
        r#"targetNamespace="urn:t" xmlns:t="urn:t""#,
        r#"<xs:simpleType name="A"><xs:restriction base="xs:string">
             <xs:enumeration value="x"/><xs:enumeration value="y"/><xs:enumeration value="z"/>
           </xs:restriction></xs:simpleType>
           <xs:simpleType name="B"><xs:restriction base="t:A">
             <xs:enumeration value="x"/><xs:enumeration value="z"/>
           </xs:restriction></xs:simpleType>
           <xs:element name="b" type="t:B"/>"#,
    ))
    .unwrap();
    let text = exig_text(&set);
    assert!(text.contains("<value>x</value><value>z</value></enumeration>"), "{text}");
    assert!(fits(&set, r#"<t:b xmlns:t="urn:t">z</t:b>"#));
    assert!(!fits(&set, r#"<t:b xmlns:t="urn:t">y</t:b>"#));
}

/// A pattern that allows few characters gives the string a restricted character set.
#[test]
fn patterns_give_restricted_character_sets() {
    let with = |pattern: &str| {
        exig_text(
            &GrammarSet::from_xsd(&schema(
                r#"targetNamespace="urn:t""#,
                &format!(
                    r#"<xs:simpleType name="S"><xs:restriction base="xs:string">
                         <xs:pattern value="{pattern}"/></xs:restriction></xs:simpleType>"#
                ),
            ))
            .unwrap(),
        )
    };
    assert!(with("[0-9]{3}").contains("0123456789"));
    assert!(!with(".*").contains("0123456789"));
}

#[test]
fn cyclic_definitions_return_schema_error() {
    let compile = |body: &str| GrammarSet::from_xsd(&schema("", body));
    for body in [
        // Cyclic model group (direct and mutual).
        r#"<xs:group name="G"><xs:sequence><xs:group ref="G"/></xs:sequence></xs:group>
           <xs:element name="r"><xs:complexType><xs:group ref="G"/></xs:complexType></xs:element>"#,
        r#"<xs:group name="A"><xs:sequence><xs:group ref="B"/></xs:sequence></xs:group>
           <xs:group name="B"><xs:sequence><xs:group ref="A"/></xs:sequence></xs:group>
           <xs:element name="r"><xs:complexType><xs:group ref="A"/></xs:complexType></xs:element>"#,
        // Cyclic attribute group (direct and mutual).
        r#"<xs:attributeGroup name="AG"><xs:attributeGroup ref="AG"/></xs:attributeGroup>
           <xs:element name="r"><xs:complexType><xs:attributeGroup ref="AG"/></xs:complexType></xs:element>"#,
        r#"<xs:attributeGroup name="A"><xs:attributeGroup ref="B"/></xs:attributeGroup>
           <xs:attributeGroup name="B"><xs:attributeGroup ref="A"/></xs:attributeGroup>
           <xs:element name="r"><xs:complexType><xs:attributeGroup ref="A"/></xs:complexType></xs:element>"#,
        // Cyclic complexType derivation.
        r#"<xs:complexType name="A"><xs:complexContent><xs:extension base="B"/></xs:complexContent></xs:complexType>
           <xs:complexType name="B"><xs:complexContent><xs:extension base="A"/></xs:complexContent></xs:complexType>"#,
        r#"<xs:complexType name="A"><xs:simpleContent><xs:extension base="A"/></xs:simpleContent></xs:complexType>"#,
        // Cyclic simpleType restriction and list.
        r#"<xs:simpleType name="S"><xs:restriction base="S"/></xs:simpleType>"#,
        r#"<xs:simpleType name="L"><xs:list itemType="L"/></xs:simpleType>"#,
        // Cyclic substitutionGroup.
        r#"<xs:element name="a" substitutionGroup="b"/>
           <xs:element name="b" substitutionGroup="a"/>"#,
    ] {
        let e = compile(body).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Schema, "{body}: {e}");
    }

    // Non-cyclic diamond/repeated group references still succeed.
    let dag = compile(
        r#"<xs:group name="Leaf"><xs:sequence><xs:element name="x" type="xs:int"/></xs:sequence></xs:group>
           <xs:attributeGroup name="BaseAttr"><xs:attribute name="id" type="xs:int"/></xs:attributeGroup>
           <xs:attributeGroup name="A1"><xs:attributeGroup ref="BaseAttr"/></xs:attributeGroup>
           <xs:attributeGroup name="A2"><xs:attributeGroup ref="BaseAttr"/></xs:attributeGroup>
           <xs:element name="r"><xs:complexType>
             <xs:sequence><xs:group ref="Leaf"/><xs:group ref="Leaf"/></xs:sequence>
             <xs:attributeGroup ref="A1"/><xs:attributeGroup ref="A2"/>
           </xs:complexType></xs:element>"#,
    )
    .unwrap();
    assert!(fits(&dag, r#"<r id="1"><x>1</x><x>2</x></r>"#));
}

#[test]
fn all_with_group_and_wildcard() {
    let set = GrammarSet::from_xsd(&schema(
        "",
        r#"<xs:group name="Pair"><xs:sequence>
             <xs:element name="a" type="xs:int"/>
             <xs:element name="b" type="xs:int"/>
           </xs:sequence></xs:group>
           <xs:element name="r"><xs:complexType><xs:all>
             <xs:group ref="Pair"/>
             <xs:element name="c" type="xs:int"/>
           </xs:all></xs:complexType></xs:element>"#,
    ))
    .unwrap();
    assert!(fits(&set, "<r><c>3</c><a>1</a><b>2</b></r>"));
    assert!(fits(&set, "<r><b>2</b><c>3</c></r>"));
}
