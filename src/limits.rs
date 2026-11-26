// src/limits.rs
//
// Limits for untrusted XML
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Every limit is checked **before** the memory it guards is allocated,
//! and reaching one returns `ErrorKind::Limit`. A reader or writer is
//! then poisoned; the application drops it and, on a
//! connection, reconnects with a new stream.
//!
//! | Limit | Default | `device()` | Counts | Checked by |
//! |---|---|---|---|---|
//! | `max_depth` | 256 | 32 | open elements | reader, writer, XML reader |
//! | `max_attributes` | 1 024 | 32 | attributes and namespace declarations on one element | reader, writer, XML reader |
//! | `max_value_chars` | 1 048 576 | 4 096 | characters in one string value, name, comment, PI or literal | reader, XML reader |
//! | `max_binary_bytes` | 16 777 216 | 4 096 | octets in one binary value | reader |
//! | `max_list_items` | 65 536 | 256 | items in one list value | reader |
//! | `max_integer_octets` | 64 | 16 | octets of one EXI unsigned integer (7 value bits each) | reader, header |
//! | `max_names` | 65 536 | 512 | URIs, prefixes and local names one session adds | session tables |
//! | `max_string_table_bytes` | 67 108 864 | 16 384 | bytes of strings one session references | session tables |
//! | `max_learned_productions` | 1 048 576 | 1 024 | productions added to built-in grammars in one stream, plus built-in element grammars created | reader, writer |
//! | `max_options_bytes` | 4 096 | 256 | octets of the header's options document | header |
//! | `max_expanded_bytes` | 1 048 576 | 4 096 | bytes of replacement text expanded in one XML document, counted at every level of nesting | XML reader |
//! | `max_entity_depth` | 8 | 2 | entity references being expanded inside each other | XML reader |
//! | `max_block_events` | 4 194 304 | 4 096 | events of one pre-compression or compression block, which the reader holds until the block's values are read | reader |
//! | `max_block_bytes` | 67 108 864 | 65 536 | octets one DEFLATE stream inflates to | reader |
//!
//! The defaults are placeholders until the benchmark suite tunes them
//! against real documents. A limit of 0 is allowed and means
//! "none allowed": `max_depth(0)` rejects the first element.

/// Limits for one reader, writer or XML reader. Builder methods,
/// because the set will grow.
///
/// ```
/// use exi::Limits;
/// let l = Limits::new();
/// assert_eq!((l.get_max_depth(), l.get_max_attributes(), l.get_max_integer_octets()), (256, 1024, 64));
/// assert_eq!(l.get_max_string_table_bytes(), 64 * 1024 * 1024);
/// let d = Limits::device();
/// assert_eq!((d.get_max_depth(), d.get_max_value_chars()), (32, 4096));
/// let custom = Limits::new().max_depth(0).max_value_chars(u64::MAX);
/// assert_eq!((custom.get_max_depth(), custom.get_max_value_chars()), (0, u64::MAX));
/// assert_eq!(Limits::default(), Limits::new());
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    max_depth: u32,
    max_attributes: u32,
    max_value_chars: u64,
    max_binary_bytes: u64,
    max_list_items: u32,
    max_integer_octets: u32,
    max_names: u32,
    max_string_table_bytes: u64,
    max_learned_productions: u64,
    max_options_bytes: u32,
    max_expanded_bytes: u64,
    max_entity_depth: u32,
    max_block_events: u64,
    max_block_bytes: u64,
}

impl Limits {
    /// Defaults for hosts and servers (table above).
    pub const fn new() -> Self {
        Limits {
            max_depth: 256,
            max_attributes: 1024,
            max_value_chars: 1048576,
            max_binary_bytes: 16777216,
            max_list_items: 65536,
            max_integer_octets: 64,
            max_names: 65536,
            max_string_table_bytes: 67108864,
            max_learned_productions: 1048576,
            max_options_bytes: 4096,
            max_expanded_bytes: 1048576,
            max_entity_depth: 8,
            max_block_events: 4194304,
            max_block_bytes: 67108864,
        }
    }
    /// Small limits for microcontrollers with a heap (table above).
    pub const fn device() -> Self {
        Limits {
            max_depth: 32,
            max_attributes: 32,
            max_value_chars: 4096,
            max_binary_bytes: 4096,
            max_list_items: 256,
            max_integer_octets: 16,
            max_names: 512,
            max_string_table_bytes: 16384,
            max_learned_productions: 1024,
            max_options_bytes: 256,
            max_expanded_bytes: 4096,
            max_entity_depth: 2,
            max_block_events: 4096,
            max_block_bytes: 65536,
        }
    }
    pub const fn max_depth(self, n: u32) -> Self {
        Limits { max_depth: n, ..self }
    }
    pub const fn max_attributes(self, n: u32) -> Self {
        Limits { max_attributes: n, ..self }
    }
    pub const fn max_value_chars(self, n: u64) -> Self {
        Limits { max_value_chars: n, ..self }
    }
    pub const fn max_binary_bytes(self, n: u64) -> Self {
        Limits { max_binary_bytes: n, ..self }
    }
    pub const fn max_list_items(self, n: u32) -> Self {
        Limits { max_list_items: n, ..self }
    }
    /// At least 5 is needed to read every `u32`, and 10 for every
    /// `u64`. Values below 5 are accepted; the header then fails on
    /// large option values.
    pub const fn max_integer_octets(self, n: u32) -> Self {
        Limits { max_integer_octets: n, ..self }
    }
    /// The number of name-table entries added during a session.
    pub const fn max_names(self, n: u32) -> Self {
        Limits { max_names: n, ..self }
    }
    /// Bytes retained by the session for names and values it adds.
    pub const fn max_string_table_bytes(self, n: u64) -> Self {
        Limits { max_string_table_bytes: n, ..self }
    }
    pub const fn max_learned_productions(self, n: u64) -> Self {
        Limits { max_learned_productions: n, ..self }
    }
    pub const fn max_options_bytes(self, n: u32) -> Self {
        Limits { max_options_bytes: n, ..self }
    }
    pub const fn max_expanded_bytes(self, n: u64) -> Self {
        Limits { max_expanded_bytes: n, ..self }
    }
    pub const fn max_entity_depth(self, n: u32) -> Self {
        Limits { max_entity_depth: n, ..self }
    }
    pub const fn max_block_events(self, n: u64) -> Self {
        Limits { max_block_events: n, ..self }
    }
    pub const fn max_block_bytes(self, n: u64) -> Self {
        Limits { max_block_bytes: n, ..self }
    }

    pub const fn get_max_depth(&self) -> u32 {
        self.max_depth
    }
    pub const fn get_max_attributes(&self) -> u32 {
        self.max_attributes
    }
    pub const fn get_max_value_chars(&self) -> u64 {
        self.max_value_chars
    }
    pub const fn get_max_binary_bytes(&self) -> u64 {
        self.max_binary_bytes
    }
    pub const fn get_max_list_items(&self) -> u32 {
        self.max_list_items
    }
    pub const fn get_max_integer_octets(&self) -> u32 {
        self.max_integer_octets
    }
    pub const fn get_max_names(&self) -> u32 {
        self.max_names
    }
    pub const fn get_max_string_table_bytes(&self) -> u64 {
        self.max_string_table_bytes
    }
    pub const fn get_max_learned_productions(&self) -> u64 {
        self.max_learned_productions
    }
    pub const fn get_max_options_bytes(&self) -> u32 {
        self.max_options_bytes
    }
    pub const fn get_max_expanded_bytes(&self) -> u64 {
        self.max_expanded_bytes
    }
    pub const fn get_max_entity_depth(&self) -> u32 {
        self.max_entity_depth
    }
    pub const fn get_max_block_events(&self) -> u64 {
        self.max_block_events
    }
    pub const fn get_max_block_bytes(&self) -> u64 {
        self.max_block_bytes
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}
