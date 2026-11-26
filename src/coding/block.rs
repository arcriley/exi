// src/coding/block.rs
//
// Compression blocks
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Layout of a block
//!
//! A block is the fewest events that hold `blockSize` values; the last
//! block holds fewer. Its events are split into channels:
//!
//! | Channel | Holds |
//! |---|---|
//! | structure | every event code and every content item that is not a value; also the values of `xsi:type`, and of `xsi:nil` when it is a typed Boolean, because they choose the grammar |
//! | one per name | the values of the attributes of that name, or the character data of elements of that name, in document order |
//!
//! Value channels are ordered by where their first value occurs. The
//! block is laid out as streams:
//!
//! | Values in the block | Streams |
//! |---|---|
//! | at most 100 | one: the structure, then every value channel |
//! | more than 100 | the structure; then all channels with at most 100 values; then each channel with more than 100, alone |
//!
//! With compression each stream is a raw DEFLATE stream (RFC 1951);
//! with pre-compression the streams follow one another as they are.
//! Every part is byte-aligned.
//!
//! # Empty streams: EXIficient, not EXI §9.3
//!
//! EXI §9.3 says that a stream whose channels have no octets (values
//! that take no bits, such as an enumeration of one value) is not
//! written. EXIficient 1.0.7 writes such streams in a block of more
//! than 100 values, and cannot read a block without them. This
//! crate writes what EXIficient writes, so the two can read each other:
//!
//! | Stream | EXI §9.3 | EXIficient and this crate |
//! |---|---|---|
//! | a block of at most 100 values, with no octets (a last block of zero-bit EE and ED) | none | none |
//! | the structure of a larger block, with no octets | none | none (EXIficient not seen: no schema gave such a structure) |
//! | the channels of at most 100 values, with no octets | none | an empty DEFLATE stream, when there are such channels |
//! | a channel of more than 100 values, with no octets | none | an empty DEFLATE stream |
//!
//! The reader takes both: for channels whose values take no bits, it
//! takes the next stream when that stream is empty, and leaves it for
//! the next part when it is not.
//!
//! # String tables follow the channels
//!
//! Values are written, and the string tables updated, in the order of
//! the streams above, not document order. So the writer holds a block's
//! values until the block ends (`Deferred`), and the reader decodes a
//! block's structure before its values.

use alloc::vec::Vec;

use crate::coding::byte_aligned::ByteEncoder;
use crate::coding::{Channel, Encoder, Sink};
use crate::engine::{self, Prepared};
use crate::tables::SessionTables;
use crate::{Error, ErrorKind, NameId, Result};

/// The most values a channel may hold and still share a stream with
/// others.
pub(crate) const SMALL: usize = 100;

/// A value whose writing waits for the end of its block.
struct Deferred {
    name: NameId,
    value: Prepared,
    prefixes: bool,
}

/// The order of a block's value channels: indexes into the block's
/// values, grouped by channel, channels in stream order (small ones
/// first, each group by first value). `split` is the number of small
/// channels; with at most [`SMALL`] values in all, every channel counts
/// as small.
pub(crate) struct ChannelOrder {
    pub channels: Vec<(NameId, Vec<usize>)>,
    pub split: usize,
}

impl ChannelOrder {
    /// The order for values named `names`, in document order.
    pub fn new(names: impl Iterator<Item = NameId>) -> Self {
        let mut channels: Vec<(NameId, Vec<usize>)> = Vec::new();
        let mut index: hashbrown::HashMap<
            NameId,
            usize,
            rustc_hash::FxBuildHasher,
        > = Default::default();
        let mut total = 0;
        for (i, n) in names.enumerate() {
            let c = *index.entry(n).or_insert_with(|| {
                channels.push((n, Vec::new()));
                channels.len() - 1
            });
            channels[c].1.push(i);
            total += 1;
        }
        if total <= SMALL {
            let split = channels.len();
            return ChannelOrder { channels, split };
        }
        // A stable partition keeps first-value order within each group.
        let (small, large): (Vec<_>, Vec<_>) =
            channels.into_iter().partition(|(_, v)| v.len() <= SMALL);
        let split = small.len();
        let mut channels = small;
        channels.extend(large);
        ChannelOrder { channels, split }
    }
    /// The value indexes in the order they are coded.
    pub fn coded(&self) -> impl Iterator<Item = usize> + '_ {
        self.channels.iter().flat_map(|(_, v)| v.iter().copied())
    }
}

/// Writes a pre-compressed or compressed body. Structure goes to a
/// buffer as the events come; values wait in the block until it is full
/// or the document ends.
pub(crate) struct BlockEncoder<S: Sink> {
    sink: S,
    /// The DEFLATE level (0–9) with compression; `None` with
    /// pre-compression.
    level: Option<u32>,
    block_size: u32,
    structure: ByteEncoder<Vec<u8>>,
    values: Vec<Deferred>,
    /// Octets written to the sink so far.
    written: u64,
}

impl<S: Sink> BlockEncoder<S> {
    pub fn new(sink: S, block_size: u32, level: Option<u32>) -> Self {
        BlockEncoder {
            sink,
            level,
            block_size,
            structure: ByteEncoder::new(Vec::new()),
            values: Vec::new(),
            written: 0,
        }
    }
    pub fn sink(&self) -> &S {
        &self.sink
    }
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
    pub fn into_sink(self) -> S {
        self.sink
    }
    /// Holds a value for the end of the block; true when the block is
    /// now full and must be closed (after the event that carries the
    /// value is complete).
    pub fn defer(
        &mut self,
        name: NameId,
        value: Prepared,
        prefixes: bool,
    ) -> bool {
        self.values.push(Deferred { name, value, prefixes });
        self.values.len() as u64 >= self.block_size as u64
    }
    /// Writes the block: its values in channel order, updating `t` in
    /// that order, then its streams. The next block starts empty.
    pub fn close(&mut self, t: &mut SessionTables<'_>) -> Result<()> {
        let order = ChannelOrder::new(self.values.iter().map(|d| d.name));
        let mut bufs: Vec<Vec<u8>> = Vec::with_capacity(order.channels.len());
        for (_, members) in &order.channels {
            let mut e = ByteEncoder::new(Vec::new());
            for &i in members {
                let d = &self.values[i];
                engine::emit_value(&mut e, t, d.name, &d.value, d.prefixes)?;
            }
            bufs.push(e.into_sink());
        }
        let structure = core::mem::take(self.structure.sink_mut());
        // A fresh encoder for the next block, as large as this one's
        // structure.
        self.structure = ByteEncoder::new(Vec::with_capacity(structure.len()));
        let total = self.values.len();
        self.values.clear();
        match self.level {
            None => {
                self.put(&structure)?;
                for b in &bufs {
                    self.put(b)?;
                }
            }
            Some(level) if total <= SMALL => {
                let mut all = structure;
                for b in &bufs {
                    all.extend_from_slice(b);
                }
                self.put_deflated(&all, level)?;
            }
            Some(level) => {
                self.put_deflated(&structure, level)?;
                // EXIficient's layout: a stream for the small channels
                // whenever there are any, and for each large channel,
                // even with no octets (see the module docs).
                if order.split > 0 {
                    let small: Vec<u8> = bufs[..order.split].concat();
                    self.put(&deflate(&small, level)?)?;
                }
                for b in &bufs[order.split..] {
                    self.put(&deflate(b, level)?)?;
                }
            }
        }
        Ok(())
    }
    fn put(&mut self, octets: &[u8]) -> Result<()> {
        self.sink.write(octets)?;
        self.written += octets.len() as u64;
        Ok(())
    }
    /// One DEFLATE stream of `octets`; nothing when they are empty (EXI
    /// §9.3).
    fn put_deflated(&mut self, octets: &[u8], level: u32) -> Result<()> {
        if octets.is_empty() {
            return Ok(());
        }
        let z = deflate(octets, level)?;
        self.put(&z)
    }
}

/// Structure: every call lands in the structure buffer, byte-aligned.
/// Values reach the encoder only through [`BlockEncoder::defer`]; the
/// values that belong to the structure (`xsi:type`, a typed `xsi:nil`)
/// are written directly and land here too.
impl<S: Sink> Encoder for BlockEncoder<S> {
    fn nbit(&mut self, _ch: Channel, value: u32, width: u8) -> Result<()> {
        self.structure.nbit(Channel::Structure, value, width)
    }
    fn octets(&mut self, _ch: Channel, data: &[u8]) -> Result<()> {
        self.structure.octets(Channel::Structure, data)
    }
    fn align(&mut self) -> Result<()> {
        Ok(())
    }
    fn bit_position(&self) -> u64 {
        8 * (self.written + self.structure.sink().len() as u64)
    }
    /// A block reaches the sink only when it closes.
    fn flush(&mut self) -> Result<u8> {
        Ok(0)
    }
}

/// `octets` as one raw DEFLATE stream.
pub(crate) fn deflate(octets: &[u8], level: u32) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut z = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(octets.len() / 2 + 64),
        flate2::Compression::new(level),
    );
    z.write_all(octets)
        .and_then(|()| z.finish())
        .map_err(|_| Error::new(ErrorKind::Internal, "DEFLATE failed"))
}

// ----- Reading -----

/// One value of the block, waiting for its channel: where its event is
/// in the reader's queue, its channel's name, and how it is coded.
pub(crate) struct PendingValue {
    pub qi: usize,
    pub name: NameId,
    pub slot: engine::Slot,
}

/// What the reader knows of the block it is reading.
pub(crate) struct BlockRead {
    /// Compression: values come from inflated streams. Pre-compression:
    /// from the input.
    pub deflate: bool,
    pub block_size: u32,
    /// The structure has ended: values are being read.
    pub values_phase: bool,
    pub pending: Vec<PendingValue>,
    /// Indexes into `pending`, in the order the values are coded.
    pub order: Vec<usize>,
    /// Ranges of `order`, one per stream (compression).
    pub groups: Vec<core::ops::Range<usize>>,
    pub group: usize,
    pub next: usize,
    /// The current stream, inflated.
    pub z: Inflater,
    /// `z` holds the whole of the current stream.
    pub ready: bool,
    /// A stream has been started in `z` and is not complete yet.
    pub inflating: bool,
    /// The stream in `z` belongs to a part already read. A part with no
    /// octets has no stream (EXI §9.3), so a stream is claimed by the
    /// first part that has octets.
    pub claimed: bool,
    /// The block's values are in streams of their own (compression,
    /// more than 100 values).
    pub separate: bool,
    /// The group whose stream has been found.
    pub acquired: Option<usize>,
    /// The value of the event just decoded waits: its name and slot.
    pub last_deferred: Option<(NameId, engine::Slot)>,
}

impl BlockRead {
    pub fn new(deflate: bool, block_size: u32) -> Self {
        BlockRead {
            deflate,
            block_size,
            values_phase: false,
            pending: Vec::new(),
            order: Vec::new(),
            groups: Vec::new(),
            group: 0,
            next: 0,
            z: Inflater::new(),
            ready: false,
            inflating: false,
            claimed: false,
            separate: false,
            acquired: None,
            last_deferred: None,
        }
    }
    /// The block holds all its values, or the stream has ended: read
    /// the values next.
    pub fn end_structure(&mut self) {
        let order = ChannelOrder::new(self.pending.iter().map(|p| p.name));
        self.order = order.coded().collect();
        self.groups.clear();
        let total = self.order.len();
        // The structure claimed the stream it was read from if it took
        // octets of it.
        self.claimed = self.z.pos > 0;
        self.acquired = None;
        self.separate = self.deflate && total > SMALL;
        if !self.separate {
            // One run of values: after the structure in the same stream
            // (compression), or in the input (pre-compression).
            self.groups.push(0..total);
        } else {
            let mut at = 0;
            let small: usize = order.channels[..order.split]
                .iter()
                .map(|(_, v)| v.len())
                .sum();
            self.groups.push(0..small);
            at += small;
            for (_, v) in &order.channels[order.split..] {
                self.groups.push(at..at + v.len());
                at += v.len();
            }
        }
        self.group = 0;
        self.next = 0;
        self.values_phase = true;
    }
    /// After the block's values: the next block starts with its
    /// structure.
    pub fn next_block(&mut self) {
        self.values_phase = false;
        self.pending.clear();
        self.order.clear();
        self.groups.clear();
        self.group = 0;
        self.next = 0;
        self.ready = false;
        self.inflating = false;
        self.claimed = false;
        self.separate = false;
        self.acquired = None;
        self.last_deferred = None;
    }
}

/// Inflates one raw DEFLATE stream at a time, as input arrives.
pub(crate) struct Inflater {
    z: flate2::Decompress,
    /// The stream's octets so far; `pos` of them have been decoded.
    pub out: Vec<u8>,
    pub pos: usize,
}

impl Inflater {
    pub fn new() -> Self {
        Inflater { z: flate2::Decompress::new(false), out: Vec::new(), pos: 0 }
    }
    /// Starts a new stream, or an empty one.
    pub fn start(&mut self) {
        self.z.reset(false);
        self.out.clear();
        self.pos = 0;
    }
    /// Inflates from `input`; returns how many octets of it the stream
    /// used and whether the stream has ended.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Grammar` for input that is not DEFLATE;
    /// `ErrorKind::Limit` when the stream inflates past `max_out`
    /// octets.
    pub fn feed(
        &mut self,
        input: &[u8],
        max_out: u64,
    ) -> Result<(usize, bool)> {
        let limit = || {
            Error::new(
                ErrorKind::Limit,
                "a DEFLATE stream inflates past max_block_bytes",
            )
        };
        // Room for one octet past the limit, so that reaching it is
        // seen.
        let cap = max_out.saturating_add(1).min(usize::MAX as u64) as usize;
        let mut used = 0;
        loop {
            if self.out.capacity() - self.out.len() < 4096 {
                let want = self
                    .out
                    .len()
                    .clamp(4096, 1 << 20)
                    .min(cap.saturating_sub(self.out.len()));
                if want == 0 {
                    return Err(limit());
                }
                self.out.reserve_exact(want);
            }
            let (in0, out0) = (self.z.total_in(), self.out.len());
            let status = self
                .z
                .decompress_vec(
                    &input[used..],
                    &mut self.out,
                    flate2::FlushDecompress::None,
                )
                .map_err(|_| {
                    Error::new(
                        ErrorKind::Grammar,
                        "a compressed stream is not valid DEFLATE",
                    )
                })?;
            used += (self.z.total_in() - in0) as usize;
            if self.out.len() as u64 > max_out {
                return Err(limit());
            }
            if status == flate2::Status::StreamEnd {
                return Ok((used, true));
            }
            let progress = self.z.total_in() != in0 || self.out.len() != out0;
            if used == input.len()
                && (!progress || self.out.len() < self.out.capacity())
            {
                // All input taken and the output not full: the stream
                // needs more input.
                return Ok((used, false));
            }
        }
    }
}
