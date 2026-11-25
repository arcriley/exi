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
            ("".into(), vec!["allGlobal".into(), "base".into(), "builtinTypes".into(), "castable".into(), "element".into(), "itemType".into(), "local".into(), "maxInclusive".into(), "minInclusive".into(), "next".into(), "nillable".into(), "required".into(), "schemaId".into(), "set".into(), "type".into(), "uri".into(), "value".into()]),
            ("urn:exig:draft-1".into(), vec!["Empty".into(), "Numbers".into(), "SimpleTypes".into(), "State".into(), "Strings".into(), "Type".into(), "anyAttribute".into(), "at".into(), "atAny".into(), "atUri".into(), "attribute".into(), "attributeSet".into(), "attributes".into(), "base64Binary".into(), "boolean".into(), "booleanPattern".into(), "boundedInteger".into(), "ch".into(), "charset".into(), "content".into(), "date".into(), "dateTime".into(), "decimal".into(), "ee".into(), "element".into(), "elementSets".into(), "elements".into(), "empty".into(), "enumeration".into(), "float".into(), "gDay".into(), "gMonth".into(), "gMonthDay".into(), "gYear".into(), "gYearMonth".into(), "global".into(), "grammars".into(), "hexBinary".into(), "integer".into(), "list".into(), "local".into(), "scalar".into(), "se".into(), "seAny".into(), "seSet".into(), "seUri".into(), "set".into(), "simpleTypes".into(), "state".into(), "string".into(), "strings".into(), "time".into(), "type".into(), "types".into(), "unsignedInteger".into(), "uri".into(), "value".into()]),
        ],
        simple: vec![
            Representation::List(SimpleTypeId(1)),
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::List(SimpleTypeId(3)),
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::List(SimpleTypeId(6)),
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::String { charset: None },
            Representation::Binary(crate::BinaryKind::Base64),
            Representation::Boolean { pattern: false },
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
            Representation::Integer(IntegerRep::Signed),
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Signed),
            Representation::String { charset: None },
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Signed),
            Representation::String { charset: None },
            Representation::DateTime(crate::DateTimeKind::Time),
            Representation::String { charset: None },
            Representation::Integer(IntegerRep::Bounded { min: 0, max: 255 }),
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Unsigned),
            Representation::Integer(IntegerRep::Unsigned),
            Representation::List(SimpleTypeId(43)),
        ],
        types: vec![
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "ENTITIES")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(0), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "ENTITY")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(1), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "ID")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(2), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "IDREF")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(3), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "IDREFS")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(4), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NCName")),
                castable: true,
                casts: vec![1, 2, 3],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(5), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NMTOKEN")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(6), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NMTOKENS")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(7), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "NOTATION")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(8), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "Name")),
                castable: true,
                casts: vec![1, 2, 3, 5],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(9), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "QName")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(10), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "anySimpleType")),
                castable: true,
                casts: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 47],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(11), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "anyType")),
                castable: true,
                casts: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51],
                states: vec![
                    vec![Prod::AtAny { next: 0 }, Prod::SeAny { next: 1 }, Prod::EE, Prod::Ch { ty: SimpleTypeId(12), next: 1 }],
                    vec![Prod::SeAny { next: 1 }, Prod::EE, Prod::Ch { ty: SimpleTypeId(12), next: 1 }],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "anyURI")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(13), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "base64Binary")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(14), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "boolean")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(15), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "byte")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(16), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "date")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(17), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "dateTime")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(18), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "decimal")),
                castable: true,
                casts: vec![16, 29, 30, 32, 33, 34, 35, 37, 38, 42, 43, 44, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(19), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "double")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(20), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "duration")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(21), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "float")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(22), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gDay")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(23), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gMonth")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(24), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gMonthDay")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(25), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gYear")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(26), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "gYearMonth")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(27), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "hexBinary")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(28), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "int")),
                castable: true,
                casts: vec![16, 38],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(29), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "integer")),
                castable: true,
                casts: vec![16, 29, 32, 33, 34, 35, 37, 38, 42, 43, 44, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(30), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "language")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(31), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "long")),
                castable: true,
                casts: vec![16, 29, 38],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(32), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "negativeInteger")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(33), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "nonNegativeInteger")),
                castable: true,
                casts: vec![37, 42, 43, 44, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(34), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "nonPositiveInteger")),
                castable: true,
                casts: vec![33],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(35), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "normalizedString")),
                castable: true,
                casts: vec![1, 2, 3, 5, 6, 9, 31, 41],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(36), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "positiveInteger")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(37), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "short")),
                castable: true,
                casts: vec![16],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(38), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "string")),
                castable: true,
                casts: vec![1, 2, 3, 5, 6, 9, 31, 36, 41],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(39), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "time")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(40), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "token")),
                castable: true,
                casts: vec![1, 2, 3, 5, 6, 9, 31],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(41), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedByte")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(42), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedInt")),
                castable: true,
                casts: vec![42, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedLong")),
                castable: true,
                casts: vec![42, 43, 45],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(44), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("http://www.w3.org/2001/XMLSchema", "unsignedShort")),
                castable: true,
                casts: vec![42],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(45), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Empty")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "Numbers")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Ch { ty: SimpleTypeId(46), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "SimpleTypes")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(5), next: 0 }, Prod::Se { element: ElementId(7), next: 0 }, Prod::Se { element: ElementId(8), next: 0 }, Prod::Se { element: ElementId(9), next: 0 }, Prod::Se { element: ElementId(10), next: 0 }, Prod::Se { element: ElementId(11), next: 0 }, Prod::Se { element: ElementId(12), next: 0 }, Prod::Se { element: ElementId(13), next: 0 }, Prod::Se { element: ElementId(14), next: 0 }, Prod::Se { element: ElementId(15), next: 0 }, Prod::Se { element: ElementId(16), next: 0 }, Prod::Se { element: ElementId(17), next: 0 }, Prod::Se { element: ElementId(18), next: 0 }, Prod::Se { element: ElementId(19), next: 0 }, Prod::Se { element: ElementId(20), next: 0 }, Prod::Se { element: ElementId(21), next: 0 }, Prod::Se { element: ElementId(22), next: 0 }, Prod::Se { element: ElementId(23), next: 0 }, Prod::Se { element: ElementId(24), next: 0 }, Prod::Se { element: ElementId(25), next: 0 }, Prod::EE],
                ],
            },
            TypeModel {
                name: Some(n("urn:exig:draft-1", "State")),
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(40), next: 1 }, Prod::Se { element: ElementId(41), next: 1 }, Prod::Se { element: ElementId(42), next: 1 }, Prod::Se { element: ElementId(43), next: 1 }, Prod::Se { element: ElementId(44), next: 1 }, Prod::Se { element: ElementId(45), next: 1 }, Prod::Se { element: ElementId(46), next: 1 }, Prod::Se { element: ElementId(47), next: 1 }, Prod::Se { element: ElementId(48), next: 1 }],
                    vec![Prod::Se { element: ElementId(40), next: 1 }, Prod::Se { element: ElementId(41), next: 1 }, Prod::Se { element: ElementId(42), next: 1 }, Prod::Se { element: ElementId(43), next: 1 }, Prod::Se { element: ElementId(44), next: 1 }, Prod::Se { element: ElementId(45), next: 1 }, Prod::Se { element: ElementId(46), next: 1 }, Prod::Se { element: ElementId(47), next: 1 }, Prod::Se { element: ElementId(48), next: 1 }, Prod::EE],
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
                    vec![Prod::At { name: n("", "base"), ty: SimpleTypeId(43), next: 1 }, Prod::At { name: n("", "castable"), ty: SimpleTypeId(15), next: 2 }, Prod::At { name: n("", "local"), ty: SimpleTypeId(43), next: 3 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }, Prod::Se { element: ElementId(34), next: 5 }, Prod::Se { element: ElementId(35), next: 5 }, Prod::Se { element: ElementId(36), next: 6 }, Prod::Se { element: ElementId(37), next: 6 }, Prod::Se { element: ElementId(38), next: 6 }],
                    vec![Prod::At { name: n("", "castable"), ty: SimpleTypeId(15), next: 2 }, Prod::At { name: n("", "local"), ty: SimpleTypeId(43), next: 3 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }, Prod::Se { element: ElementId(34), next: 5 }, Prod::Se { element: ElementId(35), next: 5 }, Prod::Se { element: ElementId(36), next: 6 }, Prod::Se { element: ElementId(37), next: 6 }, Prod::Se { element: ElementId(38), next: 6 }],
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(43), next: 3 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }, Prod::Se { element: ElementId(34), next: 5 }, Prod::Se { element: ElementId(35), next: 5 }, Prod::Se { element: ElementId(36), next: 6 }, Prod::Se { element: ElementId(37), next: 6 }, Prod::Se { element: ElementId(38), next: 6 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }, Prod::Se { element: ElementId(34), next: 5 }, Prod::Se { element: ElementId(35), next: 5 }, Prod::Se { element: ElementId(36), next: 6 }, Prod::Se { element: ElementId(37), next: 6 }, Prod::Se { element: ElementId(38), next: 6 }],
                    vec![Prod::Se { element: ElementId(34), next: 5 }, Prod::Se { element: ElementId(35), next: 5 }, Prod::Se { element: ElementId(36), next: 6 }, Prod::Se { element: ElementId(37), next: 6 }, Prod::Se { element: ElementId(38), next: 6 }],
                    vec![Prod::Se { element: ElementId(35), next: 5 }, Prod::Se { element: ElementId(36), next: 6 }, Prod::Se { element: ElementId(37), next: 6 }, Prod::Se { element: ElementId(38), next: 6 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "allGlobal"), ty: SimpleTypeId(15), next: 1 }, Prod::At { name: n("", "builtinTypes"), ty: SimpleTypeId(15), next: 2 }, Prod::At { name: n("", "schemaId"), ty: SimpleTypeId(39), next: 3 }, Prod::Se { element: ElementId(1), next: 4 }],
                    vec![Prod::At { name: n("", "builtinTypes"), ty: SimpleTypeId(15), next: 2 }, Prod::At { name: n("", "schemaId"), ty: SimpleTypeId(39), next: 3 }, Prod::Se { element: ElementId(1), next: 4 }],
                    vec![Prod::At { name: n("", "schemaId"), ty: SimpleTypeId(39), next: 3 }, Prod::Se { element: ElementId(1), next: 4 }],
                    vec![Prod::Se { element: ElementId(1), next: 4 }],
                    vec![Prod::Se { element: ElementId(4), next: 5 }],
                    vec![Prod::Se { element: ElementId(27), next: 6 }],
                    vec![Prod::Se { element: ElementId(29), next: 7 }],
                    vec![Prod::Se { element: ElementId(31), next: 8 }, Prod::Se { element: ElementId(32), next: 9 }],
                    vec![Prod::Se { element: ElementId(32), next: 9 }],
                    vec![Prod::Se { element: ElementId(49), next: 10 }],
                    vec![Prod::Se { element: ElementId(51), next: 10 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "value"), ty: SimpleTypeId(39), next: 1 }],
                    vec![Prod::Se { element: ElementId(3), next: 1 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(6), next: 1 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "maxInclusive"), ty: SimpleTypeId(32), next: 1 }],
                    vec![Prod::At { name: n("", "minInclusive"), ty: SimpleTypeId(32), next: 2 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "itemType"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "base"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::Se { element: ElementId(26), next: 1 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(28), next: 0 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(30), next: 0 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::At { name: n("", "required"), ty: SimpleTypeId(15), next: 2 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 3 }],
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 3 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(33), next: 0 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "set"), ty: SimpleTypeId(43), next: 1 }, Prod::Ch { ty: SimpleTypeId(46), next: 2 }],
                    vec![Prod::Ch { ty: SimpleTypeId(46), next: 2 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 1 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(39), next: 1 }],
                    vec![Prod::Se { element: ElementId(39), next: 1 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "element"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 2 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 1 }, Prod::At { name: n("", "set"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::At { name: n("", "set"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 1 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 1 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 1 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 2 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 3 }],
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 3 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 1 }, Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 2 }],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "next"), ty: SimpleTypeId(43), next: 1 }, Prod::EE],
                    vec![Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::Se { element: ElementId(50), next: 0 }, Prod::EE],
                ],
            },
            TypeModel {
                name: None,
                castable: false,
                casts: vec![],
                states: vec![
                    vec![Prod::At { name: n("", "local"), ty: SimpleTypeId(43), next: 1 }],
                    vec![Prod::At { name: n("", "nillable"), ty: SimpleTypeId(15), next: 2 }, Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 3 }],
                    vec![Prod::At { name: n("", "type"), ty: SimpleTypeId(43), next: 3 }],
                    vec![Prod::At { name: n("", "uri"), ty: SimpleTypeId(43), next: 4 }],
                    vec![Prod::EE],
                ],
            },
        ],
        elements: vec![
            (n("urn:exig:draft-1", "grammars"), 52, false),
            (n("urn:exig:draft-1", "strings"), 50, false),
            (n("urn:exig:draft-1", "uri"), 53, false),
            (n("urn:exig:draft-1", "local"), 39, false),
            (n("urn:exig:draft-1", "simpleTypes"), 48, false),
            (n("urn:exig:draft-1", "string"), 54, false),
            (n("urn:exig:draft-1", "charset"), 39, false),
            (n("urn:exig:draft-1", "boolean"), 46, false),
            (n("urn:exig:draft-1", "booleanPattern"), 46, false),
            (n("urn:exig:draft-1", "integer"), 46, false),
            (n("urn:exig:draft-1", "unsignedInteger"), 46, false),
            (n("urn:exig:draft-1", "boundedInteger"), 55, false),
            (n("urn:exig:draft-1", "decimal"), 46, false),
            (n("urn:exig:draft-1", "float"), 46, false),
            (n("urn:exig:draft-1", "hexBinary"), 46, false),
            (n("urn:exig:draft-1", "base64Binary"), 46, false),
            (n("urn:exig:draft-1", "gYear"), 46, false),
            (n("urn:exig:draft-1", "gYearMonth"), 46, false),
            (n("urn:exig:draft-1", "date"), 46, false),
            (n("urn:exig:draft-1", "dateTime"), 46, false),
            (n("urn:exig:draft-1", "gMonth"), 46, false),
            (n("urn:exig:draft-1", "gMonthDay"), 46, false),
            (n("urn:exig:draft-1", "gDay"), 46, false),
            (n("urn:exig:draft-1", "time"), 46, false),
            (n("urn:exig:draft-1", "list"), 56, false),
            (n("urn:exig:draft-1", "enumeration"), 57, false),
            (n("urn:exig:draft-1", "value"), 39, false),
            (n("urn:exig:draft-1", "elementSets"), 58, false),
            (n("urn:exig:draft-1", "set"), 47, false),
            (n("urn:exig:draft-1", "attributes"), 59, false),
            (n("urn:exig:draft-1", "attribute"), 60, false),
            (n("urn:exig:draft-1", "attributeSet"), 47, false),
            (n("urn:exig:draft-1", "types"), 61, false),
            (n("urn:exig:draft-1", "type"), 51, false),
            (n("urn:exig:draft-1", "attributes"), 62, false),
            (n("urn:exig:draft-1", "anyAttribute"), 63, false),
            (n("urn:exig:draft-1", "empty"), 46, false),
            (n("urn:exig:draft-1", "scalar"), 64, false),
            (n("urn:exig:draft-1", "content"), 65, false),
            (n("urn:exig:draft-1", "state"), 49, false),
            (n("urn:exig:draft-1", "se"), 66, false),
            (n("urn:exig:draft-1", "seSet"), 67, false),
            (n("urn:exig:draft-1", "seUri"), 68, false),
            (n("urn:exig:draft-1", "seAny"), 69, false),
            (n("urn:exig:draft-1", "ee"), 46, false),
            (n("urn:exig:draft-1", "ch"), 70, false),
            (n("urn:exig:draft-1", "at"), 71, false),
            (n("urn:exig:draft-1", "atUri"), 72, false),
            (n("urn:exig:draft-1", "atAny"), 73, false),
            (n("urn:exig:draft-1", "elements"), 74, false),
            (n("urn:exig:draft-1", "element"), 75, false),
            (n("urn:exig:draft-1", "global"), 43, false),
        ],
        globals: vec![0],
    }
}
