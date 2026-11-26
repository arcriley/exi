// src/coding/byte_aligned.rs
//
// Byte-aligned coding
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! An n-bit value takes `ceil(n / 8)` octets, least significant octet
//! first, and nothing when `n = 0`. Octets are written as they are.
//! Every event therefore ends on an octet boundary, so a writer can
//! hand over each message the moment it ends (frames), and the
//! reader can copy runs of ASCII characters straight out of its input.

use crate::coding::{Channel, Decoder, Encoder, Sink};
use crate::{Error, ErrorKind, Result};

/// Writes byte-aligned values to a sink. Octets go to the sink as soon
/// as they are written; there is nothing to flush.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Encoder, byte_aligned::ByteEncoder};
/// let mut e = ByteEncoder::new(Vec::new());
/// e.nbit(S, 3, 2)?;              // 2 bits: one octet
/// e.nbit(S, 0, 0)?;              // 0 bits: nothing
/// e.nbit(S, 0x1FF, 9)?;          // 9 bits: two octets, low first
/// e.nbit(S, u32::MAX, 32)?;      // 32 bits: four octets
/// e.align()?;                    // no-op
/// assert_eq!(e.flush()?, 0);
/// assert_eq!(e.bit_position(), 7 * 8);
/// assert_eq!(e.into_sink(), [0x03, 0xFF, 0x01, 0xFF, 0xFF, 0xFF, 0xFF]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// A full fixed sink stops the encoder; what was written before stays.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Encoder, SliceSink, byte_aligned::ByteEncoder};
/// let mut buf = [0u8; 2];
/// let mut e = ByteEncoder::new(SliceSink::new(&mut buf));
/// e.nbit(S, 1, 8)?;
/// assert_eq!(e.nbit(S, 0x1FF, 9).unwrap_err().kind(), exi::ErrorKind::BufferFull);
/// assert_eq!(e.into_sink().written(), [0x01]);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct ByteEncoder<S: Sink> {
    sink: S,
    total: u64,
}

impl<S: Sink> ByteEncoder<S> {
    ///
    /// ```
    /// use exi::coding::byte_aligned::ByteEncoder;
    /// assert!(ByteEncoder::new(Vec::new()).into_sink().is_empty());
    /// ```
    pub fn new(sink: S) -> Self {
        ByteEncoder { sink, total: 0 }
    }
    ///
    /// ```
    /// use exi::coding::byte_aligned::ByteEncoder;
    /// let encoder = ByteEncoder::new(vec![0xAA]);
    /// assert_eq!(encoder.sink(), &[0xAA]);
    /// ```
    pub fn sink(&self) -> &S {
        &self.sink
    }
    ///
    /// ```
    /// use exi::coding::byte_aligned::ByteEncoder;
    /// let mut encoder = ByteEncoder::new(vec![0xAA]);
    /// encoder.sink_mut().push(0xBB);
    /// assert_eq!(encoder.into_sink(), [0xAA, 0xBB]);
    /// ```
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
    ///
    /// ```
    /// use exi::coding::{Channel, Encoder, byte_aligned::ByteEncoder};
    /// let mut encoder = ByteEncoder::new(Vec::new());
    /// encoder.nbit(Channel::Structure, 3, 2)?;
    /// assert_eq!(encoder.into_sink(), [3]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn into_sink(self) -> S {
        self.sink
    }
}

impl<S: Sink> Encoder for ByteEncoder<S> {
    fn nbit(&mut self, _ch: Channel, value: u32, width: u8) -> Result<()> {
        super::check_nbit(value, width)?;
        let n = width.div_ceil(8) as usize;
        if n == 0 {
            return Ok(());
        }
        let octets = value.to_le_bytes();
        self.sink.write(&octets[..n])?;
        self.total += 8 * n as u64;
        Ok(())
    }
    fn octets(&mut self, _ch: Channel, data: &[u8]) -> Result<()> {
        self.sink.write(data)?;
        self.total += 8 * data.len() as u64;
        Ok(())
    }
    fn align(&mut self) -> Result<()> {
        Ok(())
    }
    fn bit_position(&self) -> u64 {
        self.total
    }
    fn flush(&mut self) -> Result<u8> {
        Ok(0)
    }
}

/// Reads byte-aligned values from input given as two slices, read as if
/// joined: the two halves of a ring buffer's filled region, or one
/// slice and an empty one.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Decoder, byte_aligned::ByteDecoder};
/// let mut d = ByteDecoder::new(&[0x03, 0xFF], &[0x01, 0x07]);
/// assert_eq!(d.nbit(S, 2)?, 3);
/// assert_eq!(d.nbit(S, 9)?, 0x1FF);             // octets from both halves
/// assert_eq!(d.nbit(S, 0)?, 0);
/// let mut out = [0u8; 2];
/// assert_eq!(d.octets(S, &mut out).unwrap_err().kind(), exi::ErrorKind::Truncated);
/// assert_eq!(d.bit_position(), 24);             // nothing consumed by the failure
/// assert_eq!(d.nbit(S, 3)?, 7);
/// assert_eq!(d.nbit(S, 1).unwrap_err().kind(), exi::ErrorKind::Truncated);
/// // A 2-bit value whose octet holds more than 2 bits of value is malformed:
/// let mut bad = ByteDecoder::new(&[0x04], &[]);
/// assert_eq!(bad.nbit(S, 2).unwrap_err().kind(), exi::ErrorKind::Grammar);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct ByteDecoder<'a> {
    first: &'a [u8],
    second: &'a [u8],
    pos: usize,
}

impl<'a> ByteDecoder<'a> {
    ///
    /// ```
    /// use exi::coding::{Channel, Decoder, byte_aligned::ByteDecoder};
    /// let mut d = ByteDecoder::new(&[0x34], &[0x12]);
    /// assert_eq!(d.nbit(Channel::Structure, 16)?, 0x1234);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(first: &'a [u8], second: &'a [u8]) -> Self {
        ByteDecoder { first, second, pos: 0 }
    }
    /// Octets not yet read.
    ///
    /// ```
    /// use exi::coding::byte_aligned::ByteDecoder;
    /// let mut d = ByteDecoder::new(b"ab", b"c");
    /// d.advance(1);
    /// assert_eq!(d.remaining(), 2);
    /// ```
    pub fn remaining(&self) -> usize {
        self.first.len() + self.second.len() - self.pos
    }
    /// The next up to `n` unread octets, without consuming them, as the
    /// run that is contiguous in memory. The string decoder uses it to
    /// find runs of ASCII.
    ///
    /// ```
    /// use exi::coding::byte_aligned::ByteDecoder;
    /// let mut d = ByteDecoder::new(b"ab", b"c");
    /// assert_eq!(d.peek_contiguous(3), b"ab");
    /// d.advance(2);
    /// assert_eq!(d.peek_contiguous(3), b"c");
    /// ```
    pub fn peek_contiguous(&self, n: usize) -> &'a [u8] {
        if self.pos < self.first.len() {
            let end = self.first.len().min(self.pos + n);
            &self.first[self.pos..end]
        } else {
            let start = self.pos - self.first.len();
            let end = self.second.len().min(start + n);
            &self.second[start..end]
        }
    }
    /// Consumes `n` octets already seen through `peek_contiguous`.
    ///
    /// ```
    /// use exi::coding::{Channel, Decoder, byte_aligned::ByteDecoder};
    /// let mut d = ByteDecoder::new(b"ab", b"c");
    /// d.advance(2);
    /// assert_eq!(d.nbit(Channel::Structure, 8)?, u32::from(b'c'));
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn advance(&mut self, n: usize) {
        assert!(n <= self.remaining(), "advance past the end of input");
        self.pos += n;
    }
}

/// Reading an n-bit value checks the unused high bits of its last
/// octet: they must be 0, otherwise `ErrorKind::Grammar`, since the
/// value is then out of range.
impl Decoder for ByteDecoder<'_> {
    fn nbit(&mut self, _ch: Channel, width: u8) -> Result<u32> {
        super::check_width(width)?;
        let n = width.div_ceil(8) as usize;
        if n > self.remaining() {
            return Err(Error::new(
                ErrorKind::Truncated,
                "input ends inside an n-bit value",
            ));
        }
        let mut v: u64 = 0;
        for i in 0..n {
            v |= (self.byte_at(self.pos + i) as u64) << (8 * i);
        }
        if (v >> width) != 0 {
            return Err(Error::new(
                ErrorKind::Grammar,
                "n-bit value out of range for its width",
            ));
        }
        self.pos += n;
        Ok(v as u32)
    }
    fn octets(&mut self, _ch: Channel, out: &mut [u8]) -> Result<()> {
        if out.len() > self.remaining() {
            return Err(Error::new(
                ErrorKind::Truncated,
                "input ends inside binary content",
            ));
        }
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.byte_at(self.pos + i);
        }
        self.pos += out.len();
        Ok(())
    }
    fn align(&mut self) {
        // Always aligned.
    }
    fn bit_position(&self) -> u64 {
        8 * self.pos as u64
    }
    fn take_ascii(
        &mut self,
        _ch: Channel,
        max: usize,
        out: &mut alloc::string::String,
    ) -> usize {
        let mut taken = 0;
        while taken < max {
            let run = self.peek_contiguous(max - taken);
            let n = run.iter().take_while(|&&b| b < 0x80).count();
            if n == 0 {
                break;
            }
            // Octets below 0x80 are ASCII, which is valid UTF-8.
            out.push_str(core::str::from_utf8(&run[..n]).expect("ASCII"));
            self.pos += n;
            taken += n;
            if n < run.len() {
                break;
            }
        }
        taken
    }
}

impl ByteDecoder<'_> {
    fn byte_at(&self, i: usize) -> u8 {
        if i < self.first.len() {
            self.first[i]
        } else {
            self.second[i - self.first.len()]
        }
    }
}
