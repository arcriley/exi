// src/coding/bits.rs
//
// Bit-packed coding with exact bit widths
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Always compiled: the header and its options document are bit-packed
//! in every stream (D18). Bit-packed *bodies* need the feature
//! `bit-packed`.
//!
//! Bits are packed most significant first: the first bit written is bit
//! 7 of the first octet. An n-bit value is written from its most
//! significant bit. A partial last octet is held until it fills,
//! [`Encoder::align`] pads it with zeros, or the encoder is finished.

use crate::coding::{Channel, Decoder, Encoder, Sink};
use crate::{Error, ErrorKind, Result};

/// Writes bits into a sink.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Encoder, bits::BitEncoder};
/// let mut e = BitEncoder::new(Vec::new());
/// e.nbit(S, 0b10, 2)?;          // distinguishing bits
/// e.nbit(S, 1, 1)?;             // options present
/// e.nbit(S, 0, 5)?;             // final version 1
/// assert_eq!(e.bit_position(), 8);
/// e.nbit(S, 0b101, 3)?;
/// assert_eq!(e.flush()?, 3);    // three bits held back
/// e.align()?;
/// assert_eq!(e.into_sink(), [0xA0, 0b1010_0000]);
/// # Ok::<(), exi::Error>(())
/// ```
///
/// Widths at their limits: 0 writes nothing, 32 writes four octets'
/// worth of bits.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Encoder, bits::BitEncoder};
/// use exi::ErrorKind;
/// let mut e = BitEncoder::new(Vec::new());
/// e.nbit(S, 0, 0)?;
/// assert_eq!(e.bit_position(), 0);
/// e.nbit(S, u32::MAX, 32)?;
/// assert_eq!(e.nbit(S, 2, 1).unwrap_err().kind(), ErrorKind::Internal);   // 2 needs 2 bits
/// assert_eq!(e.nbit(S, 0, 33).unwrap_err().kind(), ErrorKind::Internal);
/// assert_eq!(e.bit_position(), 32);                                      // errors wrote nothing
/// e.octets(S, &[0x12])?;
/// assert_eq!(e.into_sink(), [0xFF, 0xFF, 0xFF, 0xFF, 0x12]);
/// # Ok::<(), exi::Error>(())
/// ```
pub struct BitEncoder<S: Sink> {
    sink: S,
    /// Bits not yet forming a full octet, in the low `held` bits.
    acc: u8,
    held: u8,
    total: u64,
}

impl<S: Sink> BitEncoder<S> {
    ///
    /// ```
    /// use exi::coding::bits::BitEncoder;
    /// assert!(BitEncoder::new(Vec::new()).into_sink().is_empty());
    /// ```
    pub fn new(sink: S) -> Self {
        BitEncoder { sink, acc: 0, held: 0, total: 0 }
    }
    ///
    /// ```
    /// use exi::coding::bits::BitEncoder;
    /// let encoder = BitEncoder::new(vec![0xAA]);
    /// assert_eq!(encoder.sink(), &[0xAA]);
    /// ```
    pub fn sink(&self) -> &S {
        &self.sink
    }
    /// The sink. Octets written to it directly go after the complete
    /// octets written so far, before any bits still held.
    ///
    /// ```
    /// use exi::coding::bits::BitEncoder;
    /// let mut encoder = BitEncoder::new(vec![0xAA]);
    /// encoder.sink_mut().push(0xBB);
    /// assert_eq!(encoder.into_sink(), [0xAA, 0xBB]);
    /// ```
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
    /// Pads to an octet with zeros, flushes, and returns the sink.
    ///
    /// ```
    /// use exi::coding::{Channel, Encoder, bits::BitEncoder};
    /// let mut encoder = BitEncoder::new(Vec::new());
    /// encoder.nbit(Channel::Structure, 3, 2)?;
    /// assert_eq!(encoder.into_sink(), [192]);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn into_sink(self) -> S {
        let mut me = self;
        let _ = me.align();
        me.sink
    }
}

impl<S: Sink> Encoder for BitEncoder<S> {
    fn nbit(&mut self, _ch: Channel, value: u32, width: u8) -> Result<()> {
        super::check_nbit(value, width)?;
        // The held bits, then the value's: at most 7 + 32 bits, so at
        // most 4 full octets.
        let bits = ((self.acc as u64) << width) | value as u64;
        let n = self.held + width;
        let full = (n / 8) as usize;
        let mut out = [0u8; 4];
        for (i, o) in out[..full].iter_mut().enumerate() {
            *o = (bits >> (n as usize - 8 * (i + 1))) as u8;
        }
        if full > 0 {
            self.sink.write(&out[..full])?;
        }
        self.held = n % 8;
        self.acc = (bits & ((1u64 << self.held) - 1)) as u8;
        self.total += width as u64;
        Ok(())
    }
    fn octets(&mut self, _ch: Channel, data: &[u8]) -> Result<()> {
        if self.held == 0 {
            return self
                .sink
                .write(data)
                .map(|()| self.total += 8 * data.len() as u64);
        }
        // Each octet is split across two: the held bits shift through
        // the data.
        let h = self.held;
        let mut out = alloc::vec::Vec::with_capacity(data.len());
        let mut acc = self.acc;
        for &b in data {
            out.push((acc << (8 - h)) | (b >> h));
            acc = b & ((1u8 << h) - 1);
        }
        self.sink.write(&out)?;
        self.acc = acc;
        self.total += 8 * data.len() as u64;
        Ok(())
    }
    fn align(&mut self) -> Result<()> {
        if self.held != 0 {
            let octet = self.acc << (8 - self.held);
            self.sink.write(&[octet])?;
            self.total += (8 - self.held) as u64;
            self.acc = 0;
            self.held = 0;
        }
        Ok(())
    }
    fn bit_position(&self) -> u64 {
        self.total
    }
    fn flush(&mut self) -> Result<u8> {
        Ok(self.held)
    }
}

/// Reads bits from a slice, starting at a bit offset.
///
/// ```
/// use exi::coding::{Channel::Structure as S, Decoder, bits::BitDecoder};
/// let mut d = BitDecoder::new(&[0xA0, 0b1010_0000], 0);
/// assert_eq!((d.nbit(S, 2)?, d.nbit(S, 1)?, d.nbit(S, 5)?), (0b10, 1, 0));
/// assert_eq!(d.nbit(S, 3)?, 0b101);
/// assert_eq!(d.bit_position(), 11);
/// d.align();
/// assert_eq!(d.bit_position(), 16);
/// assert_eq!(d.nbit(S, 1).unwrap_err().kind(), exi::ErrorKind::Truncated);
/// assert_eq!(d.nbit(S, 0)?, 0);                      // 0 bits: always available
/// let mut d = BitDecoder::new(&[0b0000_0001, 0x80], 7);
/// assert_eq!(d.nbit(S, 2)?, 0b11);                    // across an octet boundary
/// # Ok::<(), exi::Error>(())
/// ```
pub struct BitDecoder<'a> {
    first: &'a [u8],
    second: &'a [u8],
    bit: u64,
}

impl<'a> BitDecoder<'a> {
    /// Starts reading `input` at bit `start_bit` (0 = the most
    /// significant bit of the first octet).
    ///
    /// ```
    /// use exi::coding::{Channel, Decoder, bits::BitDecoder};
    /// let mut d = BitDecoder::new(&[0xA0], 2);
    /// assert_eq!(d.nbit(Channel::Structure, 2)?, 2);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn new(input: &'a [u8], start_bit: u64) -> Self {
        BitDecoder { first: input, second: &[], bit: start_bit }
    }
    /// Reads `first` then `second` as one input (the two parts of a
    /// ring buffer).
    ///
    /// ```
    /// use exi::coding::{Channel::Structure as S, Decoder, bits::BitDecoder};
    /// let mut d = BitDecoder::split(&[0b0000_0011], &[0b1100_0000], 6);
    /// assert_eq!(d.nbit(S, 4)?, 0b1111);
    /// # Ok::<(), exi::Error>(())
    /// ```
    pub fn split(first: &'a [u8], second: &'a [u8], start_bit: u64) -> Self {
        BitDecoder { first, second, bit: start_bit }
    }
    fn len_bits(&self) -> u64 {
        8 * (self.first.len() + self.second.len()) as u64
    }
    fn octet(&self, i: usize) -> u8 {
        if i < self.first.len() {
            self.first[i]
        } else {
            self.second[i - self.first.len()]
        }
    }
}

impl Decoder for BitDecoder<'_> {
    fn nbit(&mut self, _ch: Channel, width: u8) -> Result<u32> {
        super::check_width(width)?;
        if self.bit + width as u64 > self.len_bits() {
            return Err(Error::new(
                ErrorKind::Truncated,
                "input ends inside an n-bit value",
            ));
        }
        let mut v: u64 = 0;
        let mut need = width as u32;
        while need > 0 {
            let off = (self.bit % 8) as u32;
            let take = (8 - off).min(need);
            let octet = self.octet((self.bit / 8) as usize) as u32;
            let part = (octet >> (8 - off - take)) & ((1 << take) - 1);
            v = (v << take) | part as u64;
            need -= take;
            self.bit += take as u64;
        }
        Ok(v as u32)
    }
    fn octets(&mut self, ch: Channel, out: &mut [u8]) -> Result<()> {
        if self.bit + 8 * out.len() as u64 > self.len_bits() {
            return Err(Error::new(
                ErrorKind::Truncated,
                "input ends inside binary content",
            ));
        }
        for o in out.iter_mut() {
            *o = self.nbit(ch, 8)? as u8;
        }
        Ok(())
    }
    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }
    fn bit_position(&self) -> u64 {
        self.bit
    }
}
