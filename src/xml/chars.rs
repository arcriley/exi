// src/xml/chars.rs
//
// XML 1.0 character classes
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! XML 1.0 character classes (Fifth Edition).

/// `Char`: `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] |
/// [#x10000-#x10FFFF]`.
pub(crate) fn is_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// `NameStartChar`.
pub(crate) fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// `NameChar`.
pub(crate) fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// `Name`.
pub(crate) fn is_name(s: &str) -> bool {
    let mut it = s.chars();
    it.next().is_some_and(is_name_start) && it.all(is_name_char)
}

/// `NCName`: a `Name` without `:`.
pub(crate) fn is_ncname(s: &str) -> bool {
    let Some((&first, rest)) = s.as_bytes().split_first() else {
        return false;
    };
    if (first.is_ascii_alphabetic() || first == b'_')
        && rest.iter().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
        })
    {
        return true;
    }
    is_name(s) && !s.contains(':')
}

/// `PubidChar`: `#x20 | #xD | #xA | [a-zA-Z0-9] |
/// [-'()+,./:=?;!*#@$_%]`.
pub(crate) fn is_pubid_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, ' ' | '\r' | '\n')
        || "-'()+,./:=?;!*#@$_%".contains(c)
}

/// `S`.
pub(crate) fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}
