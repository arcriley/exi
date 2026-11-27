// src/xsd/regex.rs
//
// Restricted character sets from XML Schema patterns (EXI §7.1.10.1)
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! A string type whose pattern can only ever produce characters from a
//! small set codes its characters as indexes into that set. This module
//! walks the pattern and collects every character any part of it can
//! match. It answers `None` when the set is not small: when the pattern
//! uses `.`, a negated class, a multi-character escape other than `\s`,
//! a Unicode category, or when the set reaches 256 characters.
//! Quantifiers and grouping do not change the set, so they are only
//! skipped.
//!
//! ```
//! # #[cfg(feature = "xsd")] {
//! use exi::xsd::regex::char_set;
//! assert_eq!(char_set("[0-9]{3}-[0-9]{4}"), Some("-0123456789".chars().collect()));
//! assert_eq!(char_set("(yes|no)"), Some("enosy".chars().collect()));
//! assert_eq!(char_set(r"\s*[A-C]"), Some("\t\n\r ABC".chars().collect()));
//! assert_eq!(char_set("[a-z-[aeiou]]+").map(|s| s.len()), Some(21));
//! assert_eq!(char_set(".*"), None);                 // any character
//! assert_eq!(char_set("[^0-9]"), None);             // a complement is large
//! assert_eq!(char_set(r"\d+"), None);               // Unicode digits: more than 255
//! assert_eq!(char_set(r"\p{Lu}"), None);
//! assert_eq!(char_set("[\u{100}-\u{1FF}]"), None);  // 256 characters
//! assert_eq!(char_set("[a-"), None);                // not a valid pattern: no set
//! # }
//! ```

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// At most this many characters make a restricted set (EXI §7.1.10.1).
const MAX: usize = 255;

/// The sorted set of characters `pattern` can match, or `None` (see the
/// module docs).
pub fn char_set(pattern: &str) -> Option<Vec<char>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut p = P { s: &chars, i: 0, set: BTreeSet::new() };
    p.regexp()?;
    if p.i != chars.len() || p.set.is_empty() || p.set.len() > MAX {
        return None;
    }
    Some(p.set.into_iter().collect())
}

struct P<'a> {
    s: &'a [char],
    i: usize,
    set: BTreeSet<char>,
}

impl P<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn add(&mut self, from: char, to: char) -> Option<()> {
        if (to as u32).saturating_sub(from as u32) as usize >= MAX {
            return None;
        }
        for c in from..=to {
            self.set.insert(c);
        }
        if self.set.len() > MAX { None } else { Some(()) }
    }

    /// regExp ::= branch ( '|' branch )*
    fn regexp(&mut self) -> Option<()> {
        loop {
            self.branch()?;
            if !self.eat('|') {
                return Some(());
            }
        }
    }

    /// branch ::= piece*; piece ::= atom quantifier?
    fn branch(&mut self) -> Option<()> {
        while let Some(c) = self.peek() {
            match c {
                '|' | ')' => return Some(()),
                _ => {
                    self.atom()?;
                    self.quantifier()?;
                }
            }
        }
        Some(())
    }

    fn quantifier(&mut self) -> Option<()> {
        match self.peek() {
            Some('?' | '*' | '+') => self.i += 1,
            Some('{') => {
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == '}' {
                        return Some(());
                    }
                    if !(c.is_ascii_digit() || c == ',' || c == '{') {
                        return None;
                    }
                }
                return None;
            }
            _ => {}
        }
        Some(())
    }

    fn atom(&mut self) -> Option<()> {
        let c = self.peek()?;
        self.i += 1;
        match c {
            '(' => {
                self.regexp()?;
                if !self.eat(')') {
                    return None;
                }
                Some(())
            }
            '[' => {
                let set = self.class()?;
                for c in set {
                    self.add(c, c)?;
                }
                Some(())
            }
            '\\' => {
                let set = self.escape()?;
                for c in set {
                    self.add(c, c)?;
                }
                Some(())
            }
            '.' | '?' | '*' | '+' | '{' | '}' | ']' => None,
            c => self.add(c, c),
        }
    }

    /// After `\`: a single-character escape, or `\s`. Anything else is
    /// a large set.
    fn escape(&mut self) -> Option<Vec<char>> {
        let c = self.peek()?;
        self.i += 1;
        Some(match c {
            'n' => alloc::vec!['\n'],
            'r' => alloc::vec!['\r'],
            't' => alloc::vec!['\t'],
            's' => alloc::vec!['\t', '\n', '\r', ' '],
            '\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}'
            | '(' | ')' | '[' | ']' => alloc::vec![c],
            _ => return None, // \S \i \I \c \C \d \D \w \W \p{..} \P{..}
        })
    }

    /// After `[`: a class, with ranges and an optional
    /// `-[subtraction]`.
    fn class(&mut self) -> Option<BTreeSet<char>> {
        if self.peek() == Some('^') {
            return None;
        }
        let mut set = BTreeSet::new();
        let mut first = true;
        loop {
            let c = self.peek()?;
            match c {
                ']' if !first => {
                    self.i += 1;
                    return Some(set);
                }
                '-' if self.s.get(self.i + 1) == Some(&'[') => {
                    self.i += 2;
                    let minus = self.class()?;
                    if !self.eat(']') {
                        return None;
                    }
                    return Some(set.difference(&minus).copied().collect());
                }
                _ => {
                    let lo = self.class_char()?;
                    if lo.len() != 1 {
                        set.extend(lo); // \s
                    } else if self.peek() == Some('-')
                        && self
                            .s
                            .get(self.i + 1)
                            .is_some_and(|c| *c != ']' && *c != '[')
                    {
                        self.i += 1;
                        let hi = self.class_char()?;
                        let (from, to) = (lo[0], *hi.first()?);
                        if hi.len() != 1
                            || to < from
                            || (to as u32 - from as u32) as usize >= MAX
                        {
                            return None;
                        }
                        set.extend(from..=to);
                    } else {
                        set.insert(lo[0]);
                    }
                    if set.len() > MAX {
                        return None;
                    }
                }
            }
            first = false;
        }
    }

    /// One character of a class, or the members of `\s`.
    fn class_char(&mut self) -> Option<Vec<char>> {
        let c = self.peek()?;
        self.i += 1;
        if c != '\\' {
            return Some(alloc::vec![c]);
        }
        self.escape()
    }
}

#[cfg(test)]
mod tests {
    use super::char_set;
    use alloc::string::String;

    fn set(pattern: &str) -> Option<String> {
        char_set(pattern).map(|s| s.into_iter().collect())
    }

    #[test]
    fn classes() {
        assert_eq!(set(r"[\-\]\\]").as_deref(), Some("-\\]"));
        assert_eq!(set(r"[\sa-c]").as_deref(), Some("\t\n\r abc"));
        assert_eq!(set("[a-f-[c-d]]").as_deref(), Some("abef"));
        assert_eq!(set("[ab-]").as_deref(), Some("-ab")); // a final - is itself
        assert_eq!(set("[z-a]"), None); // a range backwards
        assert_eq!(set(r"[a-\s]"), None); // a range to a set
    }

    #[test]
    fn patterns() {
        assert_eq!(set("(ab|c)+d{2,3}").as_deref(), Some("abcd"));
        assert_eq!(set("a{x}"), None);
        assert_eq!(set("(ab"), None);
        // The empty pattern matches only the empty string: no characters to code.
        assert_eq!(set(""), None);
        assert_eq!(set("()"), None);
    }

    /// 255 characters are a set; 256 are not, in one range or several.
    #[test]
    fn the_cutoff() {
        assert_eq!(char_set("[\u{100}-\u{1FE}]").map(|s| s.len()), Some(255));
        assert_eq!(char_set("[\u{100}-\u{1FF}]"), None);
        assert_eq!(char_set("[\u{100}-\u{17F}][\u{180}-\u{1FF}]"), None);
        assert_eq!(char_set("[\u{100}-\u{17F}]|[\u{180}-\u{1FE}]").map(|s| s.len()), Some(255));
        // Overlaps count once.
        assert_eq!(char_set("[\u{100}-\u{1FE}][\u{100}-\u{110}]").map(|s| s.len()), Some(255));
    }
}
