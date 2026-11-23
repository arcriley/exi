//! Names as numbers. A stand-in for the core `name.rs`: only [`NameId`].

/// An element or attribute name as the numbers one session gives it: `uri` is the name's
/// number in the session's URI partition, `local` its number in that URI's local-name
/// partition. Both come from [`SessionTables`](crate::tables::SessionTables).
///
/// A `NameId` is meaningful only in the session that produced it. Two sessions can give
/// different numbers to the same name, and the same numbers to different names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NameId {
    pub uri: u32,
    pub local: u32,
}
