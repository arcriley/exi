// src/reader/blocks.rs
//
// EXI compressed block reading
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Block structure, channel values, and DEFLATE streams (EXI §9).

use alloc::string::String;

use super::{Out, Reader, input_view, owned_value};
use crate::coding::{Decoder, body::BodyDecoder};
use crate::engine::{self, Probe, err};
use crate::grammar::Terminal;
use crate::{Coding, ErrorKind, Result};

impl<'g, const N: usize> Reader<'g, N> {
    pub(super) fn blocks(&self) -> bool {
        self.block.is_some()
    }
    pub(super) fn inflated(&self) -> bool {
        self.block.as_ref().is_some_and(|b| b.deflate)
    }
    pub(super) fn in_values(&self) -> bool {
        self.block.as_ref().is_some_and(|b| b.values_phase)
    }
    /// Events at the front of the queue may be returned.
    pub(super) fn can_release(&self) -> bool {
        self.block.is_none() || self.released > 0
    }
    pub(super) fn popped(&mut self) {
        if self.block.is_some() {
            self.released -= 1;
        }
    }
    /// Notes the value the event just decoded left in its channel: its
    /// name and slot.
    pub(super) fn note_deferred(&mut self) {
        if !self.dec.deferred {
            return;
        }
        let name = match self.dec.term {
            Some(Terminal::CH) => self
                .sess
                .as_ref()
                .and_then(|s| s.st.stack.last())
                .map(|f| f.name),
            _ => self.dec.name.map(|(id, _)| id),
        };
        if let (Some(b), Some(name), Some(slot)) =
            (self.block.as_mut(), name, self.dec.slot)
        {
            b.last_deferred = Some((name, slot));
        }
    }
    /// After an event is queued: record its waiting value, and end the
    /// structure when the block holds `blockSize` values or the stream
    /// has ended.
    pub(super) fn after_enqueue(&mut self, is_ed: bool) -> Result<()> {
        let Some(b) = self.block.as_mut() else { return Ok(()) };
        if self.queue.len() as u64 > self.cfg.limits.get_max_block_events() {
            return Err(err(
                ErrorKind::Limit,
                "a block holds more events than max_block_events",
            ));
        }
        if let Some((name, slot)) = b.last_deferred.take() {
            b.pending.push(crate::coding::block::PendingValue {
                qi: self.queue.len() - 1,
                name,
                slot,
            });
        }
        if b.pending.len() as u64 >= b.block_size as u64 || is_ed {
            b.end_structure();
        }
        Ok(())
    }
    /// Makes the current DEFLATE stream whole in the block's buffer;
    /// `false` when more input is needed. No input at all after the end
    /// of input is an empty stream: EXI writes none for a part with no
    /// octets.
    pub(super) fn inflate_stream(&mut self) -> Result<bool> {
        let max = self.cfg.limits.get_max_block_bytes();
        loop {
            let available = self.available();
            let ended = self.ended;
            let Reader { ring, spill, spilling, block, .. } = &mut *self;
            let b = block.as_mut().expect("inflated implies a block");
            if b.ready {
                return Ok(true);
            }
            if !b.inflating {
                if available == 0 {
                    if !ended {
                        return Ok(false);
                    }
                    b.z.start();
                    b.ready = true;
                    return Ok(true);
                }
                b.z.start();
                b.inflating = true;
            }
            let (x, y) = if *spilling {
                (spill.data(), &[][..])
            } else {
                ring.filled()
            };
            let part = if x.is_empty() { y } else { x };
            if part.is_empty() {
                if ended {
                    return Err(err(
                        ErrorKind::Truncated,
                        "input ends inside a DEFLATE stream",
                    ));
                }
                return Ok(false);
            }
            let (used, done) = b.z.feed(part, max)?;
            if done {
                b.inflating = false;
                b.ready = true;
            }
            self.consume(used);
            if !done && used == 0 {
                return Ok(false);
            }
        }
    }
    /// True when every value of the current group takes no octets (a
    /// stream with no octets is not written).
    fn group_is_implicit(&self) -> bool {
        let (Some(b), Some(sess)) = (self.block.as_ref(), self.sess.as_ref())
        else {
            return false;
        };
        let r = b.groups[b.group].clone();
        let mut text = String::new();
        b.order[r].iter().all(|&i| {
            let p = &b.pending[i];
            let mut d = BodyDecoder::new(Coding::ByteAligned, &[], &[], 0);
            let mut probe = Probe::new(&sess.tables);
            text.clear();
            engine::decode_value(
                &sess.st, &mut d, &mut probe, p.slot, p.name, &mut text,
            )
            .is_ok()
        })
    }
    /// Reads the block's values in channel order and fills in their
    /// events; `Ok(true)` when the block is complete and its events may
    /// be returned.
    pub(super) fn read_values(&mut self) -> Result<bool> {
        loop {
            let b = self.block.as_mut().expect("values imply a block");
            if b.group >= b.groups.len() {
                b.next_block();
                self.released = self.queue.len();
                return Ok(true);
            }
            let r = b.groups[b.group].clone();
            b.next = b.next.max(r.start);
            if b.next >= r.end {
                b.group += 1;
                continue;
            }
            if b.separate
                && b.acquired != Some(b.group)
                && !self.acquire_group()?
            {
                return Ok(false);
            }
            if !self.read_value()? {
                return Ok(false);
            }
        }
    }
    /// Selects the current value group's DEFLATE stream (EXI §9.3).
    /// EXIficient also writes empty streams for groups with no octets.
    fn acquire_group(&mut self) -> Result<bool> {
        let implicit = self.group_is_implicit();
        loop {
            let available = self.available();
            let b = self.block.as_mut().expect("values imply a block");
            if !b.inflating && (!b.ready || b.claimed) {
                b.ready = false;
                if implicit && available == 0 {
                    if !self.ended {
                        return Ok(false);
                    }
                    b.acquired = Some(b.group);
                    b.claimed = true;
                    return Ok(true);
                }
            }
            if !self.inflate_stream()? {
                return Ok(false);
            }

            let available = self.available();
            let b = self.block.as_mut().expect("values imply a block");
            let empty = b.z.out.is_empty();
            if implicit {
                // An implicit group takes an empty stream if present,
                // leaving a nonempty stream for the next group.
                b.claimed = empty;
            } else {
                if empty {
                    if available > 0 {
                        b.claimed = true;
                        continue;
                    }
                    if !self.ended {
                        return Ok(false);
                    }
                    // Reading the group's values reports truncation.
                }
                b.claimed = true;
            }
            b.acquired = Some(b.group);
            return Ok(true);
        }
    }

    /// Reads the next value of the block; `false` when more input is
    /// needed.
    fn read_value(&mut self) -> Result<bool> {
        let inflated = self.inflated();
        let (n, final_input) = if inflated {
            (0, true)
        } else {
            let Some(l) = self.limit() else { return Ok(false) };
            l
        };
        let (qi, name, slot) = {
            let b = self.block.as_ref().expect("values imply a block");
            let p = &b.pending[b.order[b.next]];
            (p.qi, p.name, p.slot)
        };
        let mut text = core::mem::take(&mut self.dec.text);
        text.clear();
        let Some((value, used)) = ('decode: {
            let Reader { ring, spill, spilling, sess, block, .. } = self;
            let z = block
                .as_ref()
                .filter(|_| inflated)
                .map(|b| &b.z.out[b.z.pos..]);
            let (a, b) = input_view(ring, spill.data(), *spilling, z, n);
            let sess = sess.as_mut().expect("a block has a session");
            if !final_input {
                let mut d = BodyDecoder::new(Coding::ByteAligned, a, b, 0);
                let mut probe = Probe::new(&sess.tables);
                match engine::decode_value(
                    &sess.st, &mut d, &mut probe, slot, name, &mut text,
                ) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::Truncated => {
                        break 'decode None;
                    }
                    Err(e) => return Err(e),
                }
                text.clear();
            }
            let mut d = BodyDecoder::new(Coding::ByteAligned, a, b, 0);
            let v = engine::decode_value(
                &sess.st,
                &mut d,
                &mut sess.tables,
                slot,
                name,
                &mut text,
            )?;
            Some((owned_value(&sess.tables, v, &text), d.bit_position()))
        }) else {
            self.dec.text = text;
            self.spill_if_full();
            return Ok(false);
        };
        self.dec.text = text;
        self.advance_input(used, inflated);
        match self.queue.get_mut(qi) {
            Some((Out::AT(_, _, v) | Out::CH(v), _)) => *v = value,
            _ => {
                return Err(err(
                    ErrorKind::Internal,
                    "a block value without its event",
                ));
            }
        }
        self.block.as_mut().expect("values imply a block").next += 1;
        Ok(true)
    }
}
