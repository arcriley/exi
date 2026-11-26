// src/grammar/builtin.rs
//
// Built-in grammars that learn in schema-less streams
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Document grammar (option `fragment` off)
//!
//! | State | Production | Code |
//! |---|---|---|
//! | Document | SD → DocContent | 0 |
//! | DocContent | SE(\*) → DocEnd | 0 |
//! | | DT → DocContent | 1.0 |
//! | | CM → DocContent | 1.1.0 |
//! | | PI → DocContent | 1.1.1 |
//! | DocEnd | ED | 0 |
//! | | CM → DocEnd | 1.0 |
//! | | PI → DocEnd | 1.1 |
//!
//! # Fragment grammar (option `fragment` on)
//!
//! | State | Production | Code |
//! |---|---|---|
//! | Fragment | SD → FragmentContent | 0 |
//! | FragmentContent | SE(\*) → FragmentContent | 0 |
//! | | ED | 1 |
//! | | CM → FragmentContent | 2.0 |
//! | | PI → FragmentContent | 2.1 |
//!
//! FragmentContent learns: when SE(\*) matches a name `q`, a production
//! SE(q) → FragmentContent is added with code 0, and the first part of
//! every other code grows by one. DocContent does not need to learn: it
//! is left after one element.
//!
//! # Element grammars
//!
//! Each element name has one built-in element grammar for the whole
//! stream, created when the name is first met as SE(\*), and shared by
//! every element with that name, wherever it occurs. It has two states.
//! Before pruning:
//!
//! | State | Production | Code |
//! |---|---|---|
//! | StartTagContent | EE | 0.0 |
//! | | AT(\*) → StartTagContent | 0.1 |
//! | | NS → StartTagContent | 0.2 |
//! | | SC → Fragment | 0.3 |
//! | | SE(\*) → ElementContent | 0.4 |
//! | | CH → ElementContent | 0.5 |
//! | | ER → ElementContent | 0.6 |
//! | | CM → ElementContent | 0.7.0 |
//! | | PI → ElementContent | 0.7.1 |
//! | ElementContent | EE | 0 |
//! | | SE(\*) → ElementContent | 1.0 |
//! | | CH → ElementContent | 1.1 |
//! | | ER → ElementContent | 1.2 |
//! | | CM → ElementContent | 1.3.0 |
//! | | PI → ElementContent | 1.3.1 |
//!
//! With default options (nothing preserved, not self-contained) this
//! prunes to: StartTagContent `EE 0.0, AT(*) 0.1, SE(*) 0.2, CH 0.3`
//! and ElementContent `EE 0, SE(*) 1.0, CH 1.1`. A fresh
//! StartTagContent therefore has one first-level value (0 bits) and
//! four second-level values (2 bits).
//!
//! An SE production enters the child's element grammar at
//! StartTagContent; when the child's EE is read, the parent continues
//! in the state the SE production names.
//!
//! # Learning
//!
//! Learned productions are added with code 0 at the first level, and
//! the first part of every other code in that state grows by one. They
//! are learned only in the state where their event happened:
//!
//! | Event matched | In state | Learned production | Condition |
//! |---|---|---|---|
//! | SE(\*) with name `q` | StartTagContent | SE(q) → ElementContent | always |
//! | SE(\*) with name `q` | ElementContent | SE(q) → ElementContent | always |
//! | AT(\*) with name `q` | StartTagContent | AT(q) → StartTagContent | unless `q` is `xsi:type` and an AT(xsi:type) production with a one-part code exists |
//! | CH | StartTagContent | CH → ElementContent | unless a CH production with a one-part code exists |
//! | CH | ElementContent | CH → ElementContent | unless a CH production with a one-part code exists |
//! | EE | StartTagContent | EE | unless an EE production with a one-part code exists |
//!
//! NS, SC, ER, CM and PI never cause learning. "Always" means a reader
//! learns even when an equal production already exists, because the
//! stream used SE(\*); a libexi writer always uses the learned
//! production, so it never causes a duplicate.
//!
//! Learning counts toward `Limits::max_learned_productions`, and
//! creating an element grammar counts as one too.
//!
//! # Values in built-in grammars
//!
//! Attribute values and character content are strings (with the string
//! tables), except the value of `xsi:type`, which is a name (URI, local
//! name, prefix) unless `Preserve.lexicalValues` is on. (`xsi:nil` is a
//! string in schema-less streams; to be confirmed against a fixture
//! before this module is marked done.)
//!
//! # Self-contained elements
//!
//! The SC production follows an SE. Then: the encoder pads to an octet;
//! the string tables and every learned grammar are saved and reset to
//! the stream's initial state; the element is coded as a fragment body
//! `SD, SE(q), its content, EE, ED` with the fragment grammar,
//! where `q` is the element's name; the encoder pads to an octet again;
//! and the saved state is restored. The parent continues as if the
//! element had been an ordinary child.

use alloc::vec::Vec;

use crate::grammar::{EventCode, Terminal};
use crate::{Error, ErrorKind, Options, Preserve, Result};

fn limit() -> Error {
    Error::new(ErrorKind::Limit, "max_learned_productions reached")
}

fn internal(detail: &'static str) -> Error {
    Error::new(ErrorKind::Internal, detail)
}

/// Which pruned productions an options set keeps (module documentation
/// of [`crate::grammar`]).
///
/// ```
/// use exi::grammar::builtin::Keep;
/// use exi::{Options, Preserve};
/// let k = Keep::from_options(&Options::default());
/// assert_eq!(k, Keep::default());
/// assert!(!k.ns && !k.sc && !k.dtd && !k.comments && !k.pis);
/// let all = Options::builder().preserve(Preserve::all()).self_contained(true)
///     .coding(exi::Coding::ByteAligned).build()?;
/// let k = Keep::from_options(&all);
/// assert!(k.ns && k.sc && k.dtd && k.comments && k.pis);
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Keep {
    pub ns: bool,
    pub sc: bool,
    pub dtd: bool,
    pub comments: bool,
    pub pis: bool,
}

impl Keep {
    pub fn from_options(o: &Options) -> Self {
        let p = o.preserve();
        Keep {
            ns: p.contains(Preserve::PREFIXES),
            sc: o.self_contained(),
            dtd: p.contains(Preserve::DTD),
            comments: p.contains(Preserve::COMMENTS),
            pis: p.contains(Preserve::PIS),
        }
    }
}

/// The productions of one state with their codes, after pruning, in
/// code order.
///
/// `count(prefix)` gives how many values the next part takes after the
/// parts in `prefix`: what [`write_event_code`](crate::coding::datatypes::write_event_code) and
/// [`read_code_part`](crate::coding::datatypes::read_code_part) need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeTable {
    entries: Vec<(Terminal, EventCode)>,
}

impl CodeTable {
    pub fn entries(&self) -> &[(Terminal, EventCode)] {
        &self.entries
    }
    /// The code of the first production matching `t`.
    pub fn code(&self, t: Terminal) -> Option<EventCode> {
        self.entries.iter().find(|(x, _)| *x == t).map(|(_, c)| *c)
    }
    /// The number of values the part after `prefix` takes (`prefix`
    /// empty: the first part).
    pub fn count(&self, prefix: &[u32]) -> u32 {
        let k = prefix.len();
        self.entries
            .iter()
            .filter(|(_, c)| c.parts().len() > k && &c.parts()[..k] == prefix)
            .map(|(_, c)| c.parts()[k] + 1)
            .max()
            .unwrap_or(0)
    }
    /// `(value, count)` for each part of `code`, ready for
    /// `write_event_code`.
    pub fn parts_with_counts(&self, code: EventCode) -> Vec<(u32, u32)> {
        let p = code.parts();
        (0..p.len()).map(|i| (p[i], self.count(&p[..i]))).collect()
    }
    /// The production whose code is exactly `parts`, if any. When
    /// `parts` is the prefix of longer codes, returns `None`: read
    /// another part.
    pub fn lookup(&self, parts: &[u32]) -> Option<Terminal> {
        self.entries.iter().find(|(_, c)| c.parts() == parts).map(|(t, _)| *t)
    }
}

/// One element name's built-in grammar.
///
/// ```
/// use exi::grammar::{builtin::{ElementGrammar, Keep}, EventCode, Terminal};
/// let g = ElementGrammar::new(Keep::default());
/// let st = g.start_tag();
/// assert_eq!(st.code(Terminal::EE), Some(EventCode::new(&[0, 0])));
/// assert_eq!(st.code(Terminal::AtAny), Some(EventCode::new(&[0, 1])));
/// assert_eq!(st.code(Terminal::SeAny), Some(EventCode::new(&[0, 2])));
/// assert_eq!(st.code(Terminal::CH), Some(EventCode::new(&[0, 3])));
/// assert_eq!(st.code(Terminal::NS), None);                         // pruned
/// assert_eq!((st.count(&[]), st.count(&[0])), (1, 4));             // 0 bits, then 2 bits
/// let ec = g.content();
/// assert_eq!(ec.code(Terminal::EE), Some(EventCode::new(&[0])));
/// assert_eq!(ec.code(Terminal::CH), Some(EventCode::new(&[1, 1])));
/// assert_eq!((ec.count(&[]), ec.count(&[1])), (2, 2));
/// ```
///
/// Everything kept: nine StartTagContent productions, a third level for
/// CM and PI.
///
/// ```
/// use exi::grammar::{builtin::{ElementGrammar, Keep}, EventCode, Terminal};
/// let all = Keep { ns: true, sc: true, dtd: true, comments: true, pis: true };
/// let st = ElementGrammar::new(all).start_tag().clone();
/// assert_eq!(st.code(Terminal::SC), Some(EventCode::new(&[0, 3])));
/// assert_eq!(st.code(Terminal::PI), Some(EventCode::new(&[0, 7, 1])));
/// assert_eq!((st.count(&[0]), st.count(&[0, 7])), (8, 2));
/// let only_pis = Keep { pis: true, ..Keep::default() };
/// let st = ElementGrammar::new(only_pis).start_tag().clone();
/// assert_eq!(st.code(Terminal::PI), Some(EventCode::new(&[0, 4, 0])));   // renumbered
/// assert_eq!((st.count(&[0]), st.count(&[0, 4])), (5, 1));
/// ```
///
/// Learning inserts at 0 and shifts the rest.
///
/// ```
/// use exi::grammar::{builtin::{ElementGrammar, Keep, Learn}, EventCode, Terminal};
/// use exi::{Limits, NameId};
/// let id = NameId { uri: 0, local: 0 };
/// let mut g = ElementGrammar::new(Keep::default());
/// let mut budget = Limits::new().get_max_learned_productions();
/// g.learn_start_tag(Learn::AT(id), &mut budget)?;
/// g.learn_start_tag(Learn::EE, &mut budget)?;
/// let st = g.start_tag();
/// assert_eq!(st.code(Terminal::EE), Some(EventCode::new(&[0])));       // the learned one
/// assert_eq!(st.code(Terminal::AT(id)), Some(EventCode::new(&[1])));
/// assert_eq!(st.code(Terminal::AtAny), Some(EventCode::new(&[2, 1])));
/// assert_eq!(st.count(&[]), 3);
/// g.learn_start_tag(Learn::EE, &mut budget)?;                          // already one-part
/// assert_eq!(g.start_tag().count(&[]), 3);
/// let x = NameId { uri: 0, local: 1 };
/// g.learn_content(Learn::SE(x), &mut budget)?;
/// assert_eq!(g.content().code(Terminal::SE(x)), Some(EventCode::new(&[0])));
/// assert_eq!(g.content().code(Terminal::EE), Some(EventCode::new(&[1])));
/// # Ok::<(), exi::Error>(())
/// ```
///
/// The learning budget: at 0, learning fails and nothing changes.
///
/// ```
/// use exi::grammar::{builtin::{ElementGrammar, Keep, Learn}, Terminal};
/// use exi::{ErrorKind, NameId};
/// let mut g = ElementGrammar::new(Keep::default());
/// let mut budget = 1;
/// g.learn_start_tag(Learn::CH, &mut budget)?;
/// assert_eq!(budget, 0);
/// let err = g.learn_start_tag(Learn::AT(NameId { uri: 0, local: 0 }), &mut budget).unwrap_err();
/// assert_eq!(err.kind(), ErrorKind::Limit);
/// assert_eq!(g.start_tag().count(&[]), 2);                             // CH and the 0.x group
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct ElementGrammar {
    start_tag: CodeTable,
    content: CodeTable,
}

/// What an event teaches an element grammar (module table "Learning").
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Learn {
    SE(crate::NameId),
    AT(crate::NameId),
    /// `xsi:type` met through AT(\*).
    XsiType(crate::NameId),
    CH,
    EE,
}

impl ElementGrammar {
    pub fn new(keep: Keep) -> Self {
        use Terminal::*;
        let c = EventCode::new;
        let start = [
            (EE, c(&[0, 0])),
            (AtAny, c(&[0, 1])),
            (NS, c(&[0, 2])),
            (SC, c(&[0, 3])),
            (SeAny, c(&[0, 4])),
            (CH, c(&[0, 5])),
            (ER, c(&[0, 6])),
            (CM, c(&[0, 7, 0])),
            (PI, c(&[0, 7, 1])),
        ];
        let content = [
            (EE, c(&[0])),
            (SeAny, c(&[1, 0])),
            (CH, c(&[1, 1])),
            (ER, c(&[1, 2])),
            (CM, c(&[1, 3, 0])),
            (PI, c(&[1, 3, 1])),
        ];
        ElementGrammar {
            start_tag: CodeTable::pruned(&start, keeps(keep)),
            content: CodeTable::pruned(&content, keeps(keep)),
        }
    }
    pub fn start_tag(&self) -> &CodeTable {
        &self.start_tag
    }
    pub fn content(&self) -> &CodeTable {
        &self.content
    }
    /// Learns in StartTagContent, as the module table says. A
    /// production the table says not to learn changes nothing and costs
    /// nothing. `budget` is what remains of
    /// `Limits::max_learned_productions`; each production learned takes
    /// one.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when `budget` is 0; nothing changes.
    /// `ErrorKind::Internal` for `Learn::EE` in ElementContent (EE is
    /// always there).
    pub fn learn_start_tag(
        &mut self,
        l: Learn,
        budget: &mut u64,
    ) -> Result<()> {
        let t = match l {
            Learn::SE(q) => Terminal::SE(q),
            Learn::XsiType(q)
                if self.start_tag.has_one_part(Terminal::AT(q)) =>
            {
                return Ok(());
            }
            Learn::AT(q) | Learn::XsiType(q) => Terminal::AT(q),
            Learn::CH if self.start_tag.has_one_part(Terminal::CH) => {
                return Ok(());
            }
            Learn::CH => Terminal::CH,
            Learn::EE if self.start_tag.has_one_part(Terminal::EE) => {
                return Ok(());
            }
            Learn::EE => Terminal::EE,
        };
        spend(budget)?;
        self.start_tag.insert_front(t);
        Ok(())
    }
    /// Learns in ElementContent: `SE` and `CH` only.
    pub fn learn_content(&mut self, l: Learn, budget: &mut u64) -> Result<()> {
        let t = match l {
            Learn::SE(q) => Terminal::SE(q),
            Learn::CH if self.content.has_one_part(Terminal::CH) => {
                return Ok(());
            }
            Learn::CH => Terminal::CH,
            Learn::EE | Learn::AT(_) | Learn::XsiType(_) => {
                return Err(internal("ElementContent learns only SE and CH"));
            }
        };
        spend(budget)?;
        self.content.insert_front(t);
        Ok(())
    }
}

/// The document and fragment grammars' top-level states after pruning.
///
/// ```
/// use exi::grammar::{builtin::{TopLevel, Keep}, EventCode, Terminal};
/// let doc = TopLevel::document(Keep::default());
/// assert_eq!(doc.content().count(&[]), 1);                  // SE(*) alone: 0 bits
/// assert_eq!(doc.end().count(&[]), 1);                       // ED alone: 0 bits
/// let dt = TopLevel::document(Keep { dtd: true, ..Keep::default() });
/// assert_eq!(dt.content().code(Terminal::DT), Some(EventCode::new(&[1, 0])));
/// let frag = TopLevel::fragment(Keep::default());
/// assert_eq!(frag.content().code(Terminal::ED), Some(EventCode::new(&[1])));
/// assert_eq!(frag.content().count(&[]), 2);
/// ```
#[derive(Clone, Debug)]
pub struct TopLevel {
    content: CodeTable,
    end: Option<CodeTable>,
}

impl TopLevel {
    pub fn document(keep: Keep) -> Self {
        use Terminal::*;
        let c = EventCode::new;
        let content = [
            (SeAny, c(&[0])),
            (DT, c(&[1, 0])),
            (CM, c(&[1, 1, 0])),
            (PI, c(&[1, 1, 1])),
        ];
        let end = [(ED, c(&[0])), (CM, c(&[1, 0])), (PI, c(&[1, 1]))];
        TopLevel {
            content: CodeTable::pruned(&content, keeps(keep)),
            end: Some(CodeTable::pruned(&end, keeps(keep))),
        }
    }
    pub fn fragment(keep: Keep) -> Self {
        use Terminal::*;
        let c = EventCode::new;
        let content = [
            (SeAny, c(&[0])),
            (ED, c(&[1])),
            (CM, c(&[2, 0])),
            (PI, c(&[2, 1])),
        ];
        TopLevel {
            content: CodeTable::pruned(&content, keeps(keep)),
            end: None,
        }
    }
    /// DocContent, or FragmentContent.
    pub fn content(&self) -> &CodeTable {
        &self.content
    }
    /// DocEnd. A fragment has no separate end state: this returns its
    /// FragmentContent.
    pub fn end(&self) -> &CodeTable {
        self.end.as_ref().unwrap_or(&self.content)
    }
    /// FragmentContent learning (module documentation).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when `budget` is 0; `ErrorKind::Internal` for
    /// a document.
    pub fn learn_fragment_se(
        &mut self,
        name: crate::NameId,
        budget: &mut u64,
    ) -> Result<()> {
        if self.end.is_some() {
            return Err(internal("a document's top level does not learn"));
        }
        spend(budget)?;
        self.content.insert_front(Terminal::SE(name));
        Ok(())
    }
}

impl CodeTable {
    /// Keeps the productions whose terminal `keep` accepts, from a full
    /// table in code order, and renumbers each level so its values stay
    /// contiguous.
    pub(crate) fn pruned(
        full: &[(Terminal, EventCode)],
        keep: impl Fn(Terminal) -> bool,
    ) -> CodeTable {
        let kept: Vec<(Terminal, EventCode)> =
            full.iter().copied().filter(|(t, _)| keep(*t)).collect();
        let entries = kept
            .iter()
            .map(|(t, orig)| {
                let mut parts = [0u32; 3];
                let orig_parts = orig.parts();
                for k in 0..orig_parts.len() {
                    // The rank of this part among the distinct values
                    // taken at level k by the productions that share
                    // the earlier parts.
                    let mut count = 0u32;
                    let mut prev = None;
                    for (_, o) in &kept {
                        let op = o.parts();
                        if op.len() > k
                            && op[..k] == orig_parts[..k]
                            && op[k] < orig_parts[k]
                            && prev != Some(op[k])
                        {
                            count += 1;
                            prev = Some(op[k]);
                        }
                    }
                    parts[k] = count;
                }
                (*t, EventCode::new(&parts[..orig_parts.len()]))
            })
            .collect();
        CodeTable { entries }
    }

    /// One-part codes 0, 1, 2, … in the order given.
    pub(crate) fn flat(
        terminals: impl IntoIterator<Item = Terminal>,
    ) -> CodeTable {
        CodeTable {
            entries: terminals
                .into_iter()
                .enumerate()
                .map(|(i, t)| (t, EventCode::new(&[i as u32])))
                .collect(),
        }
    }

    /// Appends `t` with the code `parts`.
    pub(crate) fn push(&mut self, t: Terminal, parts: &[u32]) {
        self.entries.push((t, EventCode::new(parts)));
    }

    fn has_one_part(&self, t: Terminal) -> bool {
        self.entries.iter().any(|(x, c)| *x == t && c.len() == 1)
    }

    /// Adds `t` with code 0 and shifts the first part of every other
    /// code.
    fn insert_front(&mut self, t: Terminal) {
        for (_, c) in &mut self.entries {
            c.parts[0] += 1;
        }
        self.entries.insert(0, (t, EventCode::new(&[0])));
    }
}

/// Spends one unit of the learning budget, or fails without change.
fn spend(budget: &mut u64) -> Result<()> {
    if *budget == 0 {
        return Err(limit());
    }
    *budget -= 1;
    Ok(())
}

pub(crate) fn keeps(keep: Keep) -> impl Fn(Terminal) -> bool {
    move |t| match t {
        Terminal::NS => keep.ns,
        Terminal::SC => keep.sc,
        Terminal::DT | Terminal::ER => keep.dtd,
        Terminal::CM => keep.comments,
        Terminal::PI => keep.pis,
        _ => true,
    }
}
