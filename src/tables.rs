//! The string tables of one EXI stream, built on the global tables.
//!
//! This module is written so the tables can be implemented from it alone. It states every
//! rule EXI sets for string tables. The EXI section each rule comes from is named in
//! brackets for reviewers; the implementation does not need it.
//!
//! # What a session's tables are
//!
//! An EXI encoder and decoder each keep string tables for one stream. Both add the same
//! strings in the same order, so a number means the same string on both sides. Every rule
//! below exists for that: if this code adds, skips or removes one entry differently from
//! another implementation, every later number differs and the stream decodes as wrong data.
//!
//! [`SessionTables`] are those tables. Each entry is the id of a string in the
//! [`GlobalStrings`] (ST2), so a string is stored once however many sessions use it. The
//! numbers are exactly EXI's; sharing changes nothing a stream writes.
//!
//! The tables do not read or write EXI. The reader and writer call them in the order the
//! stream needs.
//!
//! # Partitions (R1) [EXI §7.3.1]
//!
//! | Partition | How many | Holds |
//! |---|---|---|
//! | URI | one | namespace URIs |
//! | Prefix | one per URI | prefixes declared for that URI |
//! | Local name | one per URI | local names of elements and attributes in that namespace |
//! | Global value | one | every value kept, in one numbering |
//! | Local value | one per name ([`NameId`]) | the values of that name, in a second numbering |
//!
//! Every partition numbers its entries from 0. The local value partition of a name holds
//! the values of attributes with that name and the character content of elements with that
//! name: an element and an attribute with the same URI and local name share one partition.
//! (To be confirmed against a fixture from a second implementation before this module is
//! marked done.)
//!
//! # Initial entries (R2) [EXI App. D]
//!
//! Every stream starts with these entries, in this order. Value partitions start empty.
//!
//! | URI number | URI | Prefixes | Local names |
//! |---|---|---|---|
//! | 0 | `""` (no namespace) | `""` | none |
//! | 1 | `http://www.w3.org/XML/1998/namespace` | `xml` | `base`, `id`, `lang`, `space` |
//! | 2 | `http://www.w3.org/2001/XMLSchema-instance` | `xsi` | `nil`, `type` |
//! | 3, only when a schema is used | `http://www.w3.org/2001/XMLSchema` | none | the 46 built-in type names, sorted: `ENTITIES`, `ENTITY`, `ID`, `IDREF`, `IDREFS`, `NCName`, `NMTOKEN`, `NMTOKENS`, `NOTATION`, `Name`, `QName`, `anySimpleType`, `anyType`, `anyURI`, `base64Binary`, `boolean`, `byte`, `date`, `dateTime`, `decimal`, `double`, `duration`, `float`, `gDay`, `gMonth`, `gMonthDay`, `gYear`, `gYearMonth`, `hexBinary`, `int`, `integer`, `language`, `long`, `negativeInteger`, `nonNegativeInteger`, `nonPositiveInteger`, `normalizedString`, `positiveInteger`, `short`, `string`, `time`, `token`, `unsignedByte`, `unsignedInt`, `unsignedLong`, `unsignedShort` |
//!
//! With a schema, its target namespaces that are not already listed follow URI 3, sorted.
//! Each URI's local-name partition then holds its entries above plus every name the schema
//! declares in that namespace, the whole partition sorted, duplicates removed. A schema adds
//! no prefixes. "Sorted" means by Unicode code point, which for UTF-8 is byte order: Rust's
//! `str` ordering. So `"Z"` sorts before `"a"`, and `"é"` after `"z"`.
//!
//! # Names (R3) [EXI §7.3.2]
//!
//! A URI, prefix or local name sent as a literal is added after it, with the next number in
//! its partition. Names are never removed during a stream.
//!
//! # Values (R4, R5, R6, R11) [EXI §7.3.3, Profile §4]
//!
//! **R4: which values are added.** A value sent as a literal is added only when all three
//! hold:
//!
//! 1. `valuePartitionCapacity` is not 0;
//! 2. the value has at least one character, so the empty value is never added;
//! 3. the value has at most `valueMaxLength` characters, when that option is set.
//!
//! Characters are Unicode code points, what `str::chars` yields, not bytes.
//!
//! **R5: where.** An added value goes into the global value partition and into the local
//! value partition of its name, each with that partition's next number.
//!
//! **R6: a full global partition.** When `valuePartitionCapacity` is set and the global
//! partition holds that many values, the next value added takes the slot of the oldest. A
//! slot counter goes 0, 1, …, capacity − 1, then back to 0, and the slot it points at is the
//! global number of the value added. The value that was in that slot is removed from the
//! global partition and from its local partition. Its local number is permanently
//! unassigned: the local partition keeps counting from where it was, and the number is never
//! given to another value. Names are never removed this way; only values.
//!
//! **R11: no local partitions.** With the EXI Profile parameter `localValuePartitions` set
//! to 0, values go only into the global partition, and only global numbers exist.
//!
//! # Self-contained elements (R7) [EXI §8.4.3]
//!
//! Inside a self-contained element the tables are saved, reset to the initial entries
//! (R2), and restored when the element ends. Self-contained elements can nest.
//! [`SessionTables::begin_self_contained`] and [`SessionTables::end_self_contained`] do
//! this, keeping the saved partitions on a stack inside the session.
//!
//! # One stream (R8) [EXI §7.3]
//!
//! Tables last for one stream. The next stream starts again from R2 with a new
//! `SessionTables`.
//!
//! # Order of additions (R9) [EXI §9.3]
//!
//! The tables add entries in the order they are called. With compression or
//! pre-compression the stream holds values in channel order, not document order, and the
//! reader and writer must call the tables in channel order.
//!
//! # Finding values (R10) [C14N §4.5.6]
//!
//! A value may be sent by its local number or its global number. [`SessionTables::find_value`]
//! returns the local number when there is one, then the global number. Canonical EXI
//! requires that order, and other writers lose nothing by it.
//!
//! # Duplicates
//!
//! The reader adds what the stream says. An encoder may send a literal for a string the
//! table already holds, and then the table holds it twice, under two numbers, as the encoder
//! does. Lookups find the most recent number. A libexi writer always looks first, so it never
//! adds a duplicate.
//!
//! # Writer policy (ST3)
//!
//! libexi writers leave `valuePartitionCapacity` unset, so their value partitions only grow.
//! When a session reaches `Limits::max_string_table_bytes` it returns `ErrorKind::Limit`, and
//! the application starts a new stream. The tables do not decide this: they apply the
//! [`ValueRules`] they are given. R6 exists for streams from other encoders.
//!
//! # References
//!
//! Every entry a session adds holds one reference to its global string (one per value
//! addition, for its global and local entry together). Initial entries are held by
//! [`Initial`], once for all its sessions. Dropping a `SessionTables`, or ending a
//! self-contained element, releases the references of what it drops; strings no one
//! references are freed.
//!
//! # Counts, not widths
//!
//! The number of bits a number takes on the wire depends on how many entries its partition
//! has. The tables report the counts ([`SessionTables::uri_count`] and the others) and the
//! codec computes the widths. [`SessionTables::local_value_count`] counts every number ever
//! assigned in that partition, permanently unassigned ones included.
//!
//! # Representation
//!
//! | Part | Representation |
//! |---|---|
//! | Initial entries | Held by [`Initial`]: id arrays per partition and their reverse maps, built once. A session refers to them and stores only what it adds, so creating a session copies nothing |
//! | URIs, prefixes, local names | Per partition: the initial part from `Initial`, then a `Vec` of added ids, and a reverse map (`hashbrown::HashMap`, `FxBuildHasher`) from id to its most recent number |
//! | Global values | `Vec<Slot>` with `Slot { id: ValueId, name: NameId }`, grown up to the capacity, then reused by the slot counter; a reverse map from id to its most recent slot |
//! | Local values | `HashMap<NameId, LocalPartition>`, with `LocalPartition { base: u32, live: VecDeque<ValueId>, reverse: HashMap<ValueId, u32> }`. Live numbers are `base .. base + live.len()`; numbers below `base` are permanently unassigned |
//! | Self-contained stack | `Vec` of saved partition sets, innermost last |
//! | Counts | names added, and bytes referenced (see Limits), over the current and saved partitions together |
//!
//! Why a queue is enough for local partitions: values enter the global partition in time
//! order and R6 removes them in the same order, oldest first. Each local partition receives
//! its values in time order too. So the value R6 removes is always the oldest live value of
//! its own local partition: its front. Removing it is `live.pop_front()` and `base += 1`.
//! Memory is bounded by the live values, not by every value ever added.
//!
//! On removal, drop a reverse-map entry only when it points at the number or slot being
//! removed; with duplicates it may point at a newer one.
//!
//! # Limits
//!
//! | Limit | Counts | When exceeded |
//! |---|---|---|
//! | [`Limits::max_names`] | URIs, prefixes and local names this session added, all kinds together, saved partitions included. Initial entries do not count | `ErrorKind::Limit` |
//! | [`Limits::max_string_table_bytes`] | UTF-8 bytes of every name this session added, plus every value now in its global value partition, saved partitions included. Initial entries do not count. A removed value (R6) stops counting. The check is `bytes − removed + added ≤ limit`, so exactly the limit is allowed | `ErrorKind::Limit` |
//! | Numbers per partition | `0 ..= u32::MAX − 1` | `ErrorKind::Limit` |
//! | Global table limits | [`GlobalConfig`](crate::strings::GlobalConfig) | `ErrorKind::Limit` |
//!
//! On any error, nothing is changed, and the session stays usable.

use crate::strings::{GlobalStrings, Held, LocalId, PrefixId, UriId, Value, ValueId};
use crate::{Limits, NameId, Result};

/// The entries a stream starts with (R2), as strings. Attach them to a [`GlobalStrings`]
/// with [`attach`](InitialEntries::attach) to make sessions.
///
/// ```
/// use exi::tables::InitialEntries;
/// let e = InitialEntries::schemaless();
/// assert_eq!(e.uris(), ["", "http://www.w3.org/XML/1998/namespace",
///                       "http://www.w3.org/2001/XMLSchema-instance"]);
/// assert_eq!(e.prefixes(0), [""]);
/// assert_eq!(e.prefixes(1), ["xml"]);
/// assert_eq!(e.prefixes(2), ["xsi"]);
/// assert!(e.locals(0).is_empty());
/// assert_eq!(e.locals(1), ["base", "id", "lang", "space"]);
/// assert_eq!(e.locals(2), ["nil", "type"]);
/// assert!(e.locals(3).is_empty());        // out of range: empty
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitialEntries {
    // Per URI, in number order: the URI, its prefixes, its local names.
    _private: (),
}

impl InitialEntries {
    /// Without a schema: URIs 0 to 2.
    pub fn schemaless() -> InitialEntries {
        todo!()
    }

    /// With the XML Schema built-in types only: URIs 0 to 3.
    ///
    /// ```
    /// use exi::tables::InitialEntries;
    /// let e = InitialEntries::builtin_types();
    /// assert_eq!(e.uris()[3], "http://www.w3.org/2001/XMLSchema");
    /// assert!(e.prefixes(3).is_empty());     // no prefix for the XML Schema namespace
    /// let types = e.locals(3);
    /// assert_eq!(types.len(), 46);
    /// assert_eq!((types[0], types[9], types[10], types[45]),
    ///            ("ENTITIES", "Name", "QName", "unsignedShort"));
    /// assert!(types.windows(2).all(|w| w[0] < w[1]));   // sorted, no duplicates
    /// ```
    pub fn builtin_types() -> InitialEntries {
        todo!()
    }

    /// Starts from [`builtin_types`](InitialEntries::builtin_types) and adds a schema's
    /// names.
    pub fn schema() -> InitialBuilder {
        todo!()
    }

    pub fn uris(&self) -> alloc::vec::Vec<&str> {
        todo!()
    }
    /// The prefixes of URI number `uri`; empty when out of range.
    pub fn prefixes(&self, uri: u32) -> alloc::vec::Vec<&str> {
        todo!()
    }
    /// The local names of URI number `uri`; empty when out of range.
    pub fn locals(&self, uri: u32) -> alloc::vec::Vec<&str> {
        todo!()
    }

    /// Interns every entry into `g` and builds the id arrays sessions share. The returned
    /// [`Initial`] holds one reference to each string until it is dropped.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when `g` cannot take the strings. Nothing is kept.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::InitialEntries;
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let init = InitialEntries::schemaless().attach(&g)?;
    /// assert_eq!(g.uris().stats().strings, 3);
    /// assert_eq!(g.locals().stats().strings, 6);
    /// drop(init);
    /// assert_eq!(g.uris().stats().strings, 0);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn attach<'g>(&self, g: &'g GlobalStrings) -> Result<Initial<'g>> {
        todo!()
    }
}

/// Builds the initial entries of a schema (R2). Names given more than once, or already
/// present, are kept once.
///
/// ```
/// use exi::tables::InitialEntries;
/// const XML: &str = "http://www.w3.org/XML/1998/namespace";
/// let e = InitialEntries::schema()
///     .uri("urn:b")
///     .local("urn:a", "zeta")       // adds urn:a too
///     .local("urn:a", "Zeta")
///     .local("urn:a", "été")
///     .local("urn:a", "zeta")       // duplicate
///     .local("", "note")            // no namespace: URI 0 already exists
///     .local(XML, "aaa")            // joins the XML namespace's names, sorted
///     .build()?;
/// assert_eq!(&e.uris()[4..], ["urn:a", "urn:b"]);   // after URI 3, sorted
/// assert_eq!(e.locals(4), ["Zeta", "zeta", "été"]); // code-point order
/// assert!(e.locals(5).is_empty());
/// assert_eq!(e.locals(0), ["note"]);
/// assert_eq!(e.locals(1), ["aaa", "base", "id", "lang", "space"]); // numbers shift
/// assert!(e.prefixes(4).is_empty());                 // a schema adds no prefixes
/// # Ok::<(), exi::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct InitialBuilder {
    _private: (),
}

impl InitialBuilder {
    /// Adds a target namespace.
    pub fn uri(self, uri: &str) -> Self {
        todo!()
    }
    /// Adds a declared name, and its namespace if new.
    pub fn local(self, uri: &str, local: &str) -> Self {
        todo!()
    }
    /// Sorts and numbers the entries as R2 says.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` when a partition would have more than `u32::MAX − 1` entries.
    pub fn build(self) -> Result<InitialEntries> {
        todo!()
    }
}

/// Initial entries attached to one [`GlobalStrings`]: what every session of that grammar
/// set starts from. Built once, shared by any number of sessions.
pub struct Initial<'g> {
    global: &'g GlobalStrings,
}

impl<'g> Initial<'g> {
    pub fn global(&self) -> &'g GlobalStrings {
        todo!()
    }
}

impl core::fmt::Debug for Initial<'_> {
    /// Shows the partition counts.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl Drop for Initial<'_> {
    fn drop(&mut self) {
        todo!()
    }
}

/// The stream options that govern value partitions (R4, R6, R11).
///
/// [`unbounded`](ValueRules::unbounded) is EXI's default: no capacity, no maximum length,
/// local partitions on. A later `from_options(&Options)` fills these from a stream's
/// options, once `Options` exists.
///
/// ```
/// use exi::tables::ValueRules;
/// let r = ValueRules::unbounded();
/// assert_eq!((r.get_capacity(), r.get_max_length(), r.get_local_partitions()), (None, None, true));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ValueRules {
    capacity: Option<u32>,
    max_length: Option<u32>,
    local_partitions: bool,
}

impl ValueRules {
    pub const fn unbounded() -> Self {
        todo!()
    }
    /// `valuePartitionCapacity`: `None` for no limit.
    pub const fn capacity(self, n: Option<u32>) -> Self {
        todo!()
    }
    /// `valueMaxLength`, in characters: `None` for no limit.
    pub const fn max_length(self, n: Option<u32>) -> Self {
        todo!()
    }
    /// The Profile's `localValuePartitions`: `false` for 0.
    pub const fn local_partitions(self, on: bool) -> Self {
        todo!()
    }
    pub const fn get_capacity(&self) -> Option<u32> {
        todo!()
    }
    pub const fn get_max_length(&self) -> Option<u32> {
        todo!()
    }
    pub const fn get_local_partitions(&self) -> bool {
        todo!()
    }

    /// R4: is a literal of `chars` characters added to the tables?
    ///
    /// ```
    /// use exi::tables::ValueRules;
    /// let r = ValueRules::unbounded();
    /// assert!(!r.keeps(0));                                // the empty value: never
    /// assert!(r.keeps(1));
    /// assert!(r.keeps(u32::MAX));
    /// let r = ValueRules::unbounded().max_length(Some(8));
    /// assert!(r.keeps(8));                                 // exactly the maximum
    /// assert!(!r.keeps(9));
    /// assert!(!ValueRules::unbounded().max_length(Some(0)).keeps(1));  // nothing kept
    /// assert!(!ValueRules::unbounded().capacity(Some(0)).keeps(5));    // nothing kept
    /// assert!(ValueRules::unbounded().capacity(Some(1)).keeps(5));
    /// ```
    pub const fn keeps(&self, chars: u32) -> bool {
        todo!()
    }
}

/// Where a value was found (R10).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ValueHit {
    /// Its number in the name's local value partition.
    Local(u32),
    /// Its number in the global value partition.
    Global(u32),
    /// Not in this session's tables: send it as a literal.
    Miss,
}

/// EXI's string tables for one stream (R1), as arrays of global ids (ST2).
///
/// Setup used by the examples:
///
/// ```
/// use exi::strings::{GlobalStrings, GlobalConfig};
/// use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// let g = GlobalStrings::new(GlobalConfig::new())?;
/// let init = InitialEntries::schemaless().attach(&g)?;
/// let t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// assert_eq!(t.uri_count(), 3);
/// assert_eq!(t.uri_str(0), Some(""));           // no namespace is URI 0
/// assert_eq!(t.prefix_count(0), 1);
/// assert_eq!(t.local_count(1), 4);
/// assert_eq!((t.global_value_count(), t.names_added(), t.referenced_bytes()), (0, 0, 0));
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Dropping a session releases its references.
///
/// ```
/// # use exi::strings::{GlobalStrings, GlobalConfig};
/// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
/// let g = GlobalStrings::new(GlobalConfig::new())?;
/// let init = InitialEntries::schemaless().attach(&g)?;
/// let mut a = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let mut b = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
/// let (na, nb) = (a.add_name("", "n")?, b.add_name("", "n")?);
/// let v = a.add_value(na, "shared")?.unwrap();
/// b.add_value(nb, "shared")?;
/// assert_eq!(g.values().refs(v), 2);                 // stored once, referenced twice
/// drop(a);
/// assert_eq!(g.values().refs(v), 1);
/// drop(b);
/// assert_eq!(g.values().get("shared"), None);
/// assert_eq!(g.locals().get("n"), None);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct SessionTables<'a> {
    initial: &'a Initial<'a>,
}

impl<'a> SessionTables<'a> {
    /// Tables holding only the initial entries.
    pub fn new(initial: &'a Initial<'a>, rules: ValueRules, limits: &Limits) -> Self {
        todo!()
    }
    pub fn rules(&self) -> ValueRules {
        todo!()
    }

    // ----- URIs, prefixes, local names (R2, R3) -----

    pub fn uri_count(&self) -> u32 {
        todo!()
    }
    pub fn uri(&self, n: u32) -> Option<UriId> {
        todo!()
    }
    pub fn uri_str(&self, n: u32) -> Option<&str> {
        todo!()
    }
    /// The most recent number of the URI with global id `id`.
    pub fn find_uri(&self, id: UriId) -> Option<u32> {
        todo!()
    }
    pub fn find_uri_str(&self, s: &str) -> Option<u32> {
        todo!()
    }

    /// Adds a URI sent as a literal, with the next number, even when the URI is already
    /// present (see "Duplicates").
    ///
    /// # Errors
    ///
    /// `ErrorKind::Limit` for [`Limits::max_names`], [`Limits::max_string_table_bytes`],
    /// a full partition or a full global table. Nothing is changed.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// assert_eq!(t.add_uri("urn:example")?, 3);
    /// assert_eq!(t.add_uri("urn:example")?, 4);          // a duplicate gets its own number
    /// assert_eq!(t.find_uri_str("urn:example"), Some(4)); // the most recent
    /// assert_eq!(t.uri_str(3), Some("urn:example"));
    /// assert_eq!(t.add_uri("")?, 5);                     // even "" may be sent again
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// [`Limits::max_names`] counts the names this session added; initial entries are free.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # use exi::ErrorKind;
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new().max_names(2));
    /// t.add_uri("urn:a")?;
    /// t.add_local(3, "x")?;                                      // 2: exactly the limit
    /// assert_eq!(t.add_prefix(3, "p").unwrap_err().kind(), ErrorKind::Limit);
    /// assert_eq!((t.prefix_count(3), t.names_added()), (0, 2)); // nothing changed
    /// let mut u = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new().max_names(0));
    /// assert_eq!(u.add_uri("urn:a").unwrap_err().kind(), ErrorKind::Limit);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn add_uri(&mut self, s: &str) -> Result<u32> {
        todo!()
    }

    /// Prefixes in the partition of URI number `uri`; 0 when out of range.
    pub fn prefix_count(&self, uri: u32) -> u32 {
        todo!()
    }
    pub fn prefix(&self, uri: u32, n: u32) -> Option<PrefixId> {
        todo!()
    }
    pub fn prefix_str(&self, uri: u32, n: u32) -> Option<&str> {
        todo!()
    }
    pub fn find_prefix(&self, uri: u32, id: PrefixId) -> Option<u32> {
        todo!()
    }
    /// Adds a prefix to the partition of URI number `uri`, as [`add_uri`](Self::add_uri).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when `uri` is not a number this session has; otherwise as `add_uri`.
    pub fn add_prefix(&mut self, uri: u32, s: &str) -> Result<u32> {
        todo!()
    }

    /// Local names in the partition of URI number `uri`; 0 when out of range.
    pub fn local_count(&self, uri: u32) -> u32 {
        todo!()
    }
    pub fn local(&self, uri: u32, n: u32) -> Option<LocalId> {
        todo!()
    }
    pub fn local_str(&self, uri: u32, n: u32) -> Option<&str> {
        todo!()
    }
    pub fn find_local(&self, uri: u32, id: LocalId) -> Option<u32> {
        todo!()
    }
    /// Adds a local name to the partition of URI number `uri`, as [`add_uri`](Self::add_uri).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Arg` when `uri` is not a number this session has; otherwise as `add_uri`.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # use exi::ErrorKind;
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// assert_eq!(t.add_local(1, "extra")?, 4);              // after base, id, lang, space
    /// assert_eq!(t.add_local(3, "x").unwrap_err().kind(), ErrorKind::Arg);  // no URI 3
    /// assert_eq!(t.names_added(), 1);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn add_local(&mut self, uri: u32, s: &str) -> Result<u32> {
        todo!()
    }

    /// The numbers of a name, if this session has its URI and local name.
    pub fn find_name(&self, uri: &str, local: &str) -> Option<NameId> {
        todo!()
    }
    /// The numbers of a name, adding its URI and local name when absent: what a writer
    /// does. A reader adds with `add_uri` and `add_local`, as the stream says.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let lang = t.add_name("http://www.w3.org/XML/1998/namespace", "lang")?;
    /// assert_eq!(lang, exi::NameId { uri: 1, local: 2 });  // initial: nothing added
    /// let to = t.add_name("", "to")?;
    /// assert_eq!(to, exi::NameId { uri: 0, local: 0 });
    /// assert_eq!(t.add_name("", "to")?, to);                // found, not added again
    /// assert_eq!(t.names_added(), 1);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn add_name(&mut self, uri: &str, local: &str) -> Result<NameId> {
        todo!()
    }

    // ----- Values (R4, R5, R6, R10, R11) -----

    /// Values now in the global value partition: at most the capacity.
    pub fn global_value_count(&self) -> u32 {
        todo!()
    }
    /// Numbers ever assigned in the local value partition of `name`, permanently unassigned
    /// ones included (R6). 0 without local partitions (R11).
    pub fn local_value_count(&self, name: NameId) -> u32 {
        todo!()
    }
    pub fn global_value(&self, n: u32) -> Option<ValueId> {
        todo!()
    }
    pub fn global_value_str(&self, n: u32) -> Option<&str> {
        todo!()
    }
    /// `None` for a number never assigned or permanently unassigned.
    pub fn local_value(&self, name: NameId, n: u32) -> Option<ValueId> {
        todo!()
    }
    pub fn local_value_str(&self, name: NameId, n: u32) -> Option<&str> {
        todo!()
    }
    /// The value's local number if it has one, else its global number (R10).
    ///
    /// A router looks a JID up once, keeps it, and finds its number on each connection with
    /// one integer lookup:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut c1 = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let mut c2 = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let to1 = c1.add_name("", "to")?;
    /// let to2 = c2.add_name("", "to")?;
    /// c1.add_value(to1, "juliet@example.com")?;            // connection 1 has seen it
    ///
    /// let jid = g.values().intern("juliet@example.com")?;  // the router keeps this handle
    /// assert_eq!(c1.find_value(to1, jid.id()), ValueHit::Local(0));
    /// assert_eq!(c2.find_value(to2, jid.id()), ValueHit::Miss); // connection 2: send a literal
    /// drop(c1);
    /// assert_eq!(jid.as_str(), "juliet@example.com");      // still valid: the router holds it
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn find_value(&self, name: NameId, id: ValueId) -> ValueHit {
        todo!()
    }
    pub fn find_value_str(&self, name: NameId, s: &str) -> ValueHit {
        todo!()
    }

    /// Adds a value sent as a literal for `name`. Returns its global id, or `None` when R4
    /// does not keep it, in which case nothing changes. When the global partition is full,
    /// the oldest value is removed first (R6).
    ///
    /// # Errors
    ///
    /// - `ErrorKind::Arg` when `name` is not a name this session has.
    /// - `ErrorKind::Limit` for [`Limits::max_string_table_bytes`], a full partition or a
    ///   full global table.
    ///
    /// Nothing is changed on error.
    ///
    /// R4 and R5: the empty value is skipped; a kept value has both numbers.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let class = t.add_name("", "class")?;
    /// let style = t.add_name("", "style")?;
    /// assert_eq!(t.add_value(class, "")?, None);
    /// assert_eq!(t.global_value_count(), 0);
    /// let big = t.add_value(class, "big")?.unwrap();
    /// assert_eq!(t.find_value(class, big), ValueHit::Local(0));   // local first
    /// assert_eq!(t.find_value(style, big), ValueHit::Global(0));  // other names: global
    /// assert_eq!(t.find_value_str(style, "small"), ValueHit::Miss);
    /// assert_eq!(t.local_value_str(class, 0), Some("big"));
    /// assert_eq!(t.local_value_count(style), 0);
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// `valueMaxLength` counts characters, not bytes.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let rules = ValueRules::unbounded().max_length(Some(3));
    /// let mut t = SessionTables::new(&init, rules, &exi::Limits::new());
    /// let n = t.add_name("", "n")?;
    /// assert!(t.add_value(n, "ééé")?.is_some());      // 3 characters, 6 bytes
    /// assert!(t.add_value(n, "😀😀😀")?.is_some());   // 3 characters, 12 bytes
    /// assert!(t.add_value(n, "abcd")?.is_none());     // 4 characters
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// R6 at its smallest: with capacity 1, every value removes the one before it.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded().capacity(Some(1)), &exi::Limits::new());
    /// let n = t.add_name("", "n")?;
    /// let a = t.add_value(n, "a")?.unwrap();
    /// let b = t.add_value(n, "b")?.unwrap();
    /// assert_eq!(t.global_value_count(), 1);
    /// assert_eq!(t.global_value(0), Some(b));
    /// assert_eq!(t.find_value(n, a), ValueHit::Miss);
    /// assert_eq!(t.find_value(n, b), ValueHit::Local(1));  // number 0 is unassigned for good
    /// assert_eq!(t.local_value(n, 0), None);
    /// assert_eq!(t.local_value_count(n), 2);
    /// assert_eq!(g.values().get("a"), None);               // no one references "a" now
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// R6 wrapping many times: ten values through capacity 3. The slot counter is at
    /// 10 mod 3 = 1, so the oldest value still present is in slot 1.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded().capacity(Some(3)), &exi::Limits::new());
    /// let n = t.add_name("", "n")?;
    /// for i in 0..10 { t.add_value(n, &format!("v{i}"))?; }
    /// assert_eq!(t.global_value_count(), 3);
    /// let slots: Vec<_> = (0..3).map(|i| t.global_value_str(i).unwrap()).collect();
    /// assert_eq!(slots, ["v9", "v7", "v8"]);
    /// assert_eq!(t.local_value_count(n), 10);
    /// assert_eq!(t.local_value_str(n, 6), None);           // removed
    /// assert_eq!(t.local_value_str(n, 7), Some("v7"));
    /// assert_eq!(t.local_value_str(n, 9), Some("v9"));
    /// assert_eq!(t.local_value_str(n, 10), None);          // not yet assigned
    /// assert_eq!(t.find_value_str(n, "v8"), ValueHit::Local(8));
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// R6 removes a value from its own local partition only; other names keep theirs.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded().capacity(Some(2)), &exi::Limits::new());
    /// let (x, y) = (t.add_name("", "x")?, t.add_name("", "y")?);
    /// t.add_value(x, "x0")?;
    /// t.add_value(y, "y0")?;
    /// t.add_value(y, "y1")?;                                // removes "x0", the oldest
    /// assert_eq!((t.local_value_count(x), t.local_value_str(x, 0)), (1, None));
    /// assert_eq!(t.local_value_str(y, 0), Some("y0"));
    /// assert_eq!(t.find_value_str(y, "y1"), ValueHit::Local(1));
    /// let x1 = t.add_value(x, "x1")?.unwrap();              // removes "y0"
    /// assert_eq!(t.find_value(x, x1), ValueHit::Local(1));  // x's number 0 stays unassigned
    /// assert_eq!(t.local_value_str(y, 0), None);
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// Capacity 0 keeps nothing; without local partitions only global numbers exist (R11).
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded().capacity(Some(0)), &exi::Limits::new());
    /// let n = t.add_name("", "n")?;
    /// assert_eq!(t.add_value(n, "anything")?, None);
    /// assert_eq!((t.global_value_count(), t.local_value_count(n)), (0, 0));
    ///
    /// let mut u = SessionTables::new(&init, ValueRules::unbounded().local_partitions(false), &exi::Limits::new());
    /// let m = u.add_name("", "m")?;
    /// let v = u.add_value(m, "v")?.unwrap();
    /// assert_eq!(u.find_value(m, v), ValueHit::Global(0));
    /// assert_eq!((u.local_value_count(m), u.local_value(m, 0)), (0, None));
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// Duplicates: lookups find the most recent; removing the older copy leaves the newer.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded().capacity(Some(2)), &exi::Limits::new());
    /// let n = t.add_name("", "n")?;
    /// let x = t.add_value(n, "x")?.unwrap();
    /// assert_eq!(t.add_value(n, "x")?, Some(x));            // same global string
    /// assert_eq!(g.values().refs(x), 2);                    // one reference per addition
    /// assert_eq!(t.find_value(n, x), ValueHit::Local(1));
    /// t.add_value(n, "y")?;                                 // removes the first "x"
    /// assert_eq!(t.find_value(n, x), ValueHit::Local(1));   // the second is still there
    /// assert_eq!(t.find_value_str(n, "y"), ValueHit::Local(2));
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// The byte limit: names and values count, exactly the limit is allowed, and a removed
    /// value makes room (R6).
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # use exi::ErrorKind;
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let limits = exi::Limits::new().max_string_table_bytes(10);
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded().capacity(Some(1)), &limits);
    /// let n = t.add_name("", "n")?;                         // 1 byte
    /// t.add_value(n, "abcdefghi")?;                         // 10 bytes: exactly the limit
    /// t.add_value(n, "123456789")?;                         // removes 9, adds 9: still 10
    /// assert_eq!(t.referenced_bytes(), 10);
    /// assert_eq!(t.add_value(n, "0123456789").unwrap_err().kind(), ErrorKind::Limit);
    /// assert_eq!(t.global_value_str(0), Some("123456789"));  // nothing changed
    /// assert_eq!(t.add_uri("u").unwrap_err().kind(), ErrorKind::Limit);
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// A name this session does not have is an argument error:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # use exi::ErrorKind;
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let bad = exi::NameId { uri: 0, local: 0 };            // URI 0 has no local names yet
    /// assert_eq!(t.add_value(bad, "v").unwrap_err().kind(), ErrorKind::Arg);
    /// let bad = exi::NameId { uri: 9, local: 0 };
    /// assert_eq!(t.add_value(bad, "v").unwrap_err().kind(), ErrorKind::Arg);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn add_value(&mut self, name: NameId, s: &str) -> Result<Option<ValueId>> {
        todo!()
    }

    /// As [`add_value`](Self::add_value), for a literal the reader decoded straight into
    /// the global value table ([`GlobalTable::begin`](crate::strings::GlobalTable::begin)).
    /// The handle's reference passes to the session; when R4 does not keep the value, the
    /// handle is dropped.
    ///
    /// # Errors
    ///
    /// As `add_value`, and `ErrorKind::Arg` when `v` belongs to another `GlobalStrings`.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules, ValueHit};
    /// # use exi::ErrorKind;
    /// let other = GlobalStrings::new(GlobalConfig::new())?;
    /// let g = GlobalStrings::new(GlobalConfig::new())?;
    /// let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let n = t.add_name("", "n")?;
    /// let mut p = g.values().begin(20)?;
    /// p.push_str("decoded in place")?;
    /// let id = t.add_value_held(n, p.finish()?)?.unwrap();
    /// assert_eq!(g.values().refs(id), 1);                   // the session's reference
    /// assert_eq!(t.find_value(n, id), ValueHit::Local(0));
    ///
    /// let foreign = other.values().intern("x")?;
    /// assert_eq!(t.add_value_held(n, foreign).unwrap_err().kind(), ErrorKind::Arg);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn add_value_held(&mut self, name: NameId, v: Held<'a, Value>) -> Result<Option<ValueId>> {
        todo!()
    }

    // ----- Self-contained elements (R7) -----

    /// Enters a self-contained element: moves the current partitions onto an internal
    /// stack and starts again from the initial entries, with empty value partitions and the
    /// slot counter at 0 (R7). The limits keep counting what was moved aside.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// let n = t.add_name("urn:outer", "n")?;
    /// t.add_value(n, "outer value")?;
    /// t.begin_self_contained();
    /// assert_eq!((t.depth(), t.uri_count(), t.global_value_count()), (1, 3, 0));
    /// assert_eq!(t.local_count(1), 4);                       // initial names keep their numbers
    /// t.add_uri("urn:inner")?;
    /// t.begin_self_contained();                              // nested
    /// assert_eq!((t.depth(), t.uri_count()), (2, 3));
    /// t.end_self_contained()?;
    /// assert_eq!(t.uri_str(3), Some("urn:inner"));
    /// t.end_self_contained()?;
    /// assert_eq!((t.depth(), t.uri_str(3), t.global_value_str(0)),
    ///            (0, Some("urn:outer"), Some("outer value")));
    /// # Ok::<(), exi::Error>(())
    /// ```
    ///
    /// What was moved aside still counts toward the limits:
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # use exi::ErrorKind;
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new().max_names(1));
    /// t.add_uri("urn:a")?;
    /// t.begin_self_contained();
    /// assert_eq!(t.add_uri("urn:b").unwrap_err().kind(), ErrorKind::Limit);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn begin_self_contained(&mut self) {
        todo!()
    }

    /// Leaves the innermost self-contained element: drops the partitions used inside it,
    /// releasing their references, and puts back the ones moved aside by the matching
    /// [`begin_self_contained`](Self::begin_self_contained).
    ///
    /// # Errors
    ///
    /// `ErrorKind::State` when no self-contained element is open. Nothing is changed.
    ///
    /// ```
    /// # use exi::strings::{GlobalStrings, GlobalConfig};
    /// # use exi::tables::{InitialEntries, SessionTables, ValueRules};
    /// # use exi::ErrorKind;
    /// # let g = GlobalStrings::new(GlobalConfig::new())?;
    /// # let init = InitialEntries::schemaless().attach(&g)?;
    /// # let mut t = SessionTables::new(&init, ValueRules::unbounded(), &exi::Limits::new());
    /// assert_eq!(t.end_self_contained().unwrap_err().kind(), ErrorKind::State);
    /// t.begin_self_contained();
    /// let n = t.add_name("", "inner")?;
    /// t.add_value(n, "inner value")?;
    /// t.end_self_contained()?;
    /// assert_eq!(g.values().get("inner value"), None);       // released
    /// assert_eq!((t.names_added(), t.referenced_bytes()), (0, 0));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn end_self_contained(&mut self) -> Result<()> {
        todo!()
    }

    /// Self-contained elements now open.
    pub fn depth(&self) -> u32 {
        todo!()
    }

    // ----- Limits -----

    /// Names this session added, partitions moved aside included.
    pub fn names_added(&self) -> u32 {
        todo!()
    }
    /// Bytes this session references, as [`Limits::max_string_table_bytes`] counts them.
    pub fn referenced_bytes(&self) -> u64 {
        todo!()
    }
}

impl core::fmt::Debug for SessionTables<'_> {
    /// Shows the partition counts, the depth and the limit counters, not the strings.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl Drop for SessionTables<'_> {
    fn drop(&mut self) {
        todo!()
    }
}

