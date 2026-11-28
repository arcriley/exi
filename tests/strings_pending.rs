// tests/strings_pending.rs
//
// Pending strings while session tables are released
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! A `PendingStr` reservation stays valid while a session releases its
//! strings, even the last ones in the table.

use exi::grammar::GrammarSet;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{Coding, EventSink, Options, Writer, WriterConfig};

/// A writer whose session has interned one value.
fn writer_with_a_value<'g>(
    g: &'g exi::grammar::Grammars<'g>,
) -> exi::Result<Writer<'g, Vec<u8>>> {
    let opts = Options::builder().coding(Coding::ByteAligned).build()?;
    let mut w = Writer::new(Vec::new(), WriterConfig::new(opts, g))?;
    w.start_element("a")?;
    w.text("v")?;
    w.end_element()?;
    w.end_document()?;
    Ok(w)
}

/// Releasing the table's last strings must not hand out the bytes of a
/// reservation in the chunk being appended to.
#[test]
fn reservation_in_the_current_chunk_survives() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new())?;
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings)?;
    let w = writer_with_a_value(&g)?;
    assert_eq!(strings.values().stats().strings, 1);
    let mut p = strings.values().begin(16)?;
    p.push_str("pending")?;
    drop(w); // the session's value is the table's last string
    let other = strings.values().intern("zzzzzzzzzzzzzz")?;
    assert_eq!(other.as_str(), "zzzzzzzzzzzzzz");
    let kept = p.finish()?;
    assert_eq!(kept.as_str(), "pending");
    Ok(())
}

/// A reservation larger than a chunk has a chunk of its own, which
/// must outlive the table's last strings.
#[test]
fn oversized_reservation_keeps_its_chunk() -> exi::Result<()> {
    let strings = GlobalStrings::new(GlobalConfig::new().chunk_bytes(64))?;
    let set = GrammarSet::schemaless();
    let g = set.attach(&strings)?;
    let w = writer_with_a_value(&g)?;
    let p = strings.values().begin(200)?;
    let chunks = strings.values().stats().chunks;
    drop(w); // the current chunk stays, empty; the reservation's stays
    assert_eq!(strings.values().stats().chunks, chunks);
    drop(p); // now its chunk goes
    assert_eq!(strings.values().stats().chunks, chunks - 1);
    Ok(())
}
