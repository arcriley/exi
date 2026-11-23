//! Limits for hostile input. A stand-in for the core `limits.rs`, holding only the two
//! limits the string tables check. The other limits arrive with the reader.

/// Limits for one session. Builder methods, because the set will grow.
///
/// ```
/// let l = exi::Limits::new();
/// assert_eq!(l.get_max_names(), 65_536);
/// assert_eq!(l.get_max_string_table_bytes(), 64 * 1024 * 1024);
/// let l = l.max_names(10).max_string_table_bytes(1024);
/// assert_eq!((l.get_max_names(), l.get_max_string_table_bytes()), (10, 1024));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    max_names: u32,
    max_string_table_bytes: u64,
}

impl Limits {
    /// Defaults: 65 536 names, 64 MiB of strings. Placeholders until benchmarks tune them.
    /// Stand-in: implemented, not part of the red set.
    pub const fn new() -> Self {
        Limits { max_names: 65_536, max_string_table_bytes: 64 * 1024 * 1024 }
    }
    /// Most URIs, prefixes and local names one session may add, all three kinds together.
    /// Initial entries do not count.
    pub const fn max_names(mut self, n: u32) -> Self {
        self.max_names = n;
        self
    }
    /// Most bytes of strings one session may reference: the UTF-8 length of every name it
    /// added plus every value in its value partitions, saved partitions included. Initial
    /// entries do not count.
    pub const fn max_string_table_bytes(mut self, n: u64) -> Self {
        self.max_string_table_bytes = n;
        self
    }
    pub const fn get_max_names(&self) -> u32 {
        self.max_names
    }
    pub const fn get_max_string_table_bytes(&self) -> u64 {
        self.max_string_table_bytes
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}
