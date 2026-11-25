// src/grammar/exig_tables.rs
//
// Generated EXIG schema grammar tables
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Generated from `schemas/exig-1.xsd` by
//! `cargo test --features xsd exig_tables -- --ignored` (EXIG_REGEN=1 writes this file).
//! Do not edit.

use alloc::vec;

use super::exig::{Model, TypeModel};
use super::informed::{ElementId, IntegerRep, Prod, Representation, SimpleTypeId};
use crate::OwnedName;

fn n(uri: &str, local: &str) -> OwnedName {
    OwnedName::new(uri, local)
}

#[allow(unused_imports)]
pub(crate) fn model() -> Model {
    Model {
        schema_id: None,
        strings: vec![
            ("".into(), vec!["base".into(), "castable".into(), "element".into(), "event".into(), "itemType".into(), "local".into(), "maxInclusive".into(), "minInclusive".into(), "next".into(), "nillable".into(), "representation".into(), "schemaId".into(), "type".into(), "uri".into(), "value".into()]),
            ("urn:exig:draft-1".into(), vec!["Element".into(), "Event".into(), "Production".into(), "Representation".into(), "SimpleType".into(), "Strings".into(), "Type".into(), "cast".into(), "charset".into(), "element".into(), "enumeration".into(), "global".into(), "grammars".into(), "local".into(), "production".into(), "simpleType".into(), "state".into(), "strings".into(), "type".into(), "uri".into()]),
        ],
        simple: vec![
            Representation::String { charset: None },
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Signed),
            Representation::Enumeration { values: vec!["string".into(), "boolean".into(), "booleanPattern".into(), "integer".into(), "unsignedInteger".into(), "boundedInteger".into(), "decimal".into(), "float".into(), "hexBinary".into(), "base64Binary".into(), "gYear".into(), "gYearMonth".into(), "date".into(), "dateTime".into(), "gMonth".into(), "gMonthDay".into(), "gDay".into(), "time".into(), "list".into(), "enumeration".into()].into(), base: SimpleTypeId(0) },
            Representation::Boolean { pattern: false },
            Representation::Enumeration { values: vec!["AT".into(), "ATuri".into(), "ATany".into(), "SE".into(), "SEuri".into(), "SEany".into(), "EE".into(), "CH".into()].into(), base: SimpleTypeId(0) },
            Representation::List(SimpleTypeId(7)),
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::List(SimpleTypeId(9)),
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::List(SimpleTypeId(12)),
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::Binary(crate::BinaryKind::Base64),
            Representation::Integer(IntegerRep::Bounded { min: -128, max: 127 }),
            Representation::DateTime(crate::DateTimeKind::Date),
            Representation::DateTime(crate::DateTimeKind::DateTime),
            Representation::Decimal,
            Representation::Float,
            Representation::String { charset: None },
            Representation::Float,
            Representation::DateTime(crate::DateTimeKind::GDay),
            Representation::DateTime(crate::DateTimeKind::GMonth),
            Representation::DateTime(crate::DateTimeKind::GMonthDay),
            Representation::DateTime(crate::DateTimeKind::GYear),
            Representation::DateTime(crate::DateTimeKind::GYearMonth),
            Representation::Binary(crate::BinaryKind::Hex),
            Representation::Integer(IntegerRep::Signed),
            Representation::Integer(IntegerRep::Signed),
            Representation::String { charset: None },
            Representation::Integer(IntegerRep::Signed),
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Signed),
            Representation::String { charset: None },
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Signed),
            Representation::DateTime(crate::DateTimeKind::Time),
            Representation::String { charset: None },
            Representation::Integer(IntegerRep::Bounded { min: 0, max: 255 }),
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Unsigned),
        ],
        types: vec![
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "ENTITIES")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(6), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "ENTITY")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(7), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "ID")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(8), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "IDREF")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(9), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "IDREFS")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(10), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NCName")),
                castable: true,
                casts: vec![1, 2, 3],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(11), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NMTOKEN")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(12), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NMTOKENS")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(13), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NOTATION")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(14), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "Name")),
                castable: true,
                casts: vec![1, 2, 3, 5],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(15), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "QName")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(16), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "anySimpleType")),
                castable: true,
                casts: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 47, 49],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(17), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "anyType")),
                castable: true,
                casts: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52],
                states: vec![
                    vec![Prod::AtAny { next: 0 }, Prod::SeAny { next: 1 }, Prod::EE, Prod::Ch { ty: SimpleTypeId(18), next: 1 }],
                    vec![Prod::SeAny { next: 1 }, Prod::EE, Prod::Ch { ty: SimpleTypeId(18), next: 1 }],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "anyURI")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(19), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "base64Binary")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(20), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "boolean")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(4), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "byte")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(21), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "date")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(22), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "dateTime")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(23), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "decimal")),
                castable: true,
                casts: vec![16, 29, 30, 32, 33, 34, 35, 37, 38, 42, 43, 44, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(24), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "double")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(25), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "duration")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(26), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "float")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(27), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gDay")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(28), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gMonth")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(29), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gMonthDay")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(30), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gYear")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(31), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gYearMonth")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(32), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "hexBinary")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(33), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "int")),
                castable: true,
                casts: vec![16, 38],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(34), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "integer")),
                castable: true,
                casts: vec![16, 29, 32, 33, 34, 35, 37, 38, 42, 43, 44, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(35), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "language")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(36), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "long")),
                castable: true,
                casts: vec![16, 29, 38],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(2), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "negativeInteger")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(37), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "nonNegativeInteger")),
                castable: true,
                casts: vec![37, 42, 43, 44, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(38), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "nonPositiveInteger")),
                castable: true,
                casts: vec![33],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(39), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "normalizedString")),
                castable: true,
                casts: vec![1, 2, 3, 5, 6, 9, 31, 41],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(40), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "positiveInteger")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(41), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "short")),
                castable: true,
                casts: vec![16],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(42), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "string")),
                castable: true,
                casts: vec![1, 2, 3, 5, 6, 9, 31, 36, 41, 47, 49],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(0), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "time")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "token")),
                castable: true,
                casts: vec![1, 2, 3, 5, 6, 9, 31],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(44), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedByte")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(45), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedInt")),
                castable: true,
                casts: vec![42, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(1), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedLong")),
                castable: true,
                casts: vec![42, 43, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(46), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedShort")),
                castable: true,
                casts: vec![42],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(47), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Element")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(0), next: 1 }],
                    vec![Prod::At { name: n("", "nillable"), ty: SimpleTypeId(4), next: 2 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(1), next: 3 }],
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(1), next: 3 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 4 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Event")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(5), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Production")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "element"), ty: SimpleTypeId(1), next: 1 }, Prod::At { name: n("", "event"), ty: SimpleTypeId(5), next: 2 }],
                    vec![Prod::At { name: n("", "event"), ty: SimpleTypeId(5), next: 2 }],
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(0), next: 3 }, Prod::At { name: n("", "next"), ty: SimpleTypeId(1), next: 4 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(1), next: 5 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 6 }, Prod::EE],
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(1), next: 4 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(1), next: 5 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 6 }, Prod::EE],
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(1), next: 5 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 6 }, Prod::EE],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 6 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Representation")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(3), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "SimpleType")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "base"), ty: SimpleTypeId(1), next: 1 }, Prod::At { name: n("", "itemType"), ty: SimpleTypeId(1), next: 2 }, Prod::At { name: n("", "maxInclusive"), ty: SimpleTypeId(2), next: 3 }, Prod::At { name: n("", "minInclusive"), ty: SimpleTypeId(2), next: 4 }, Prod::At { name: n("", "representation"), ty: SimpleTypeId(3), next: 5 }],
                    vec![Prod::At { name: n("", "itemType"), ty: SimpleTypeId(1), next: 2 }, Prod::At { name: n("", "maxInclusive"), ty: SimpleTypeId(2), next: 3 }, Prod::At { name: n("", "minInclusive"), ty: SimpleTypeId(2), next: 4 }, Prod::At { name: n("", "representation"), ty: SimpleTypeId(3), next: 5 }],
                    vec![Prod::At { name: n("", "maxInclusive"), ty: SimpleTypeId(2), next: 3 }, Prod::At { name: n("", "minInclusive"), ty: SimpleTypeId(2), next: 4 }, Prod::At { name: n("", "representation"), ty: SimpleTypeId(3), next: 5 }],
                    vec![Prod::At { name: n("", "minInclusive"), ty: SimpleTypeId(2), next: 4 }, Prod::At { name: n("", "representation"), ty: SimpleTypeId(3), next: 5 }],
                    vec![Prod::At { name: n("", "representation"), ty: SimpleTypeId(3), next: 5 }],
                    vec![Prod::Se { element: ElementId(5), next: 5 }, Prod::Se { element: ElementId(6), next: 6 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Strings")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(2), next: 0 }, Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Type")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "castable"), ty: SimpleTypeId(4), next: 1 }, Prod::At { name: n("", "local"), ty: SimpleTypeId(0), next: 2 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 3 }, Prod::Se { element: ElementId(8), next: 4 }],
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(0), next: 2 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 3 }, Prod::Se { element: ElementId(8), next: 4 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(0), next: 3 }, Prod::Se { element: ElementId(8), next: 4 }],
                    vec![Prod::Se { element: ElementId(8), next: 4 }],
                    vec![Prod::Se { element: ElementId(8), next: 4 }, Prod::Se { element: ElementId(10), next: 5 }, Prod::EE],
                    vec![Prod::Se { element: ElementId(10), next: 5 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "schemaId"), ty: SimpleTypeId(0), next: 1 }, Prod::Se { element: ElementId(1), next: 2 }],
                    vec![Prod::Se { element: ElementId(1), next: 2 }],
                    vec![Prod::Se { element: ElementId(4), next: 2 }, Prod::Se { element: ElementId(7), next: 3 }],
                    vec![Prod::Se { element: ElementId(7), next: 3 }, Prod::Se { element: ElementId(11), next: 4 }, Prod::Se { element: ElementId(12), next: 5 }, Prod::EE],
                    vec![Prod::Se { element: ElementId(11), next: 4 }, Prod::Se { element: ElementId(12), next: 5 }, Prod::EE],
                    vec![Prod::Se { element: ElementId(12), next: 5 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "value"), ty: SimpleTypeId(0), next: 1 }],
                    vec![Prod::Se { element: ElementId(3), next: 1 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(9), next: 1 }],
                    vec![Prod::Se { element: ElementId(9), next: 1 }, Prod::EE],
                ],
            },
        ],
        elements: vec![
            (n("urn:exig:draft-1", "grammars"), 53, false),
            (n("urn:exig:draft-1", "strings"), 51, false),
            (n("urn:exig:draft-1", "uri"), 54, false),
            (n("urn:exig:draft-1", "local"), 39, false),
            (n("urn:exig:draft-1", "simpleType"), 50, false),
            (n("urn:exig:draft-1", "enumeration"), 39, false),
            (n("urn:exig:draft-1", "charset"), 39, false),
            (n("urn:exig:draft-1", "type"), 52, false),
            (n("urn:exig:draft-1", "state"), 55, false),
            (n("urn:exig:draft-1", "production"), 48, false),
            (n("urn:exig:draft-1", "cast"), 43, false),
            (n("urn:exig:draft-1", "element"), 46, false),
            (n("urn:exig:draft-1", "global"), 43, false),
        ],
        globals: vec![0],
    }
}
