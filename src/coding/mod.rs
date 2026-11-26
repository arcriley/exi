// src/coding/mod.rs
//
// Coding interfaces and octet sinks
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! # Layers
//!
//! | Layer | Type | Job |
//! |---|---|---|
//! | Sink | [`Sink`] | where finished octets go: a `Vec<u8>`, a fixed slice, a socket |
//! | Coder | [`Encoder`] / [`Decoder`] | how n-bit values and octets are laid out: bit-packed or byte-aligned |
//! | Datatypes | [`datatypes`] | how each EXI datatype is written with a coder |
//! | Names and strings | [`names`] | how names and string values are written, using the session's string tables |
//!
//! The reader and writer use only the [`Encoder`] and [`Decoder`]
//! traits, so a coding is replaced without changing them. The coders:
//!
//! | Coder | Used for |
//! |---|---|
//! | [`bits::BitEncoder`], [`bits::BitDecoder`] | the header and its options document, which are always bit-packed; bit-packed bodies (feature `bit-packed`) |
//! | [`byte_aligned::ByteEncoder`], [`byte_aligned::ByteDecoder`] | byte-aligned bodies; each channel of pre-compression and compression (feature `compression`) |
//!
//! # Channels
//!
//! Every coder call names a [`Channel`]. Pre-compression sends the
//! structure of a stream (event codes, names, namespace declarations,
//! comments, and the values of `xsi:type` and a typed `xsi:nil`) and
//! the values of each element or attribute name in separate
//! channels, so that similar values sit together (EXI §9.2). Attribute
//! values and character content go to `Channel::Value(name)`, where
//! `name` is the attribute's name or the enclosing element's name;
//! everything else goes to `Channel::Structure`. The bit-packed and
//! byte-aligned coders ignore the channel. With pre-compression and
//! compression the writer holds each block's values and writes them
//! channel by channel when the block ends, and the reader reads a
//! block's structure before its values.
//!
//! # n-bit unsigned integers
//!
//! An n-bit unsigned integer (`0 ≤ value < 2^n`, `n ≤ 32`) is the
//! primitive everything else is built from:
//!
//! | Coding | Layout of an n-bit value |
//! |---|---|
//! | bit-packed | exactly `n` bits, most significant first, packed into octets from their most significant bit |
//! | byte-aligned, pre-compression, compression | `ceil(n / 8)` octets, least significant octet first; `n = 0` takes no octets |
//!
//! # Input ends inside a value
//!
//! Decoders return `ErrorKind::Truncated` when their input ends before
//! a value does. The reader rewinds to the start of the event and
//! reports `Step::NeedInput`, unless input has been ended with
//! `end_input`, when the error stands.

use alloc::vec::Vec;

use crate::{Error, ErrorKind, NameId, Result};

pub mod bits;
#[cfg(feature = "compression")]
pub(crate) mod block;
pub(crate) mod body;
pub mod byte_aligned;
pub mod datatypes;
pub mod names;

fn check_width(width: u8) -> Result<()> {
    if width > 32 {
        return Err(Error::new(ErrorKind::Internal, "n-bit width above 32"));
    }
    Ok(())
}

fn check_nbit(value: u32, width: u8) -> Result<()> {
    check_width(width)?;
    if width < 32 && (value >> width) != 0 {
        return Err(Error::new(
            ErrorKind::Internal,
            "n-bit value does not fit its width",
        ));
    }
    Ok(())
}

/// Where encoded octets go.
///
/// Implemented for `Vec<u8>` (grows without limit), [`SliceSink`] (a
/// fixed buffer) and `&mut S` for any sink `S`.
pub trait Sink {
    /// Takes all of `octets`, or none of them.
    ///
    /// # Errors
    ///
    /// `ErrorKind::BufferFull` when a fixed buffer has too little room;
    /// `ErrorKind::Io` for an I/O failure.
    fn write(&mut self, octets: &[u8]) -> Result<()>;
}

impl Sink for Vec<u8> {
    fn write(&mut self, octets: &[u8]) -> Result<()> {
        self.extend_from_slice(octets);
        Ok(())
    }
}

impl<S: Sink + ?Sized> Sink for &mut S {
    fn write(&mut self, octets: &[u8]) -> Result<()> {
        (**self).write(octets)
    }
}

/// A sink over a fixed buffer, for devices.
///
/// ```
/// use exi::coding::{Sink, SliceSink};
/// let mut buf = [0u8; 3];
/// let mut s = SliceSink::new(&mut buf);
/// s.write(&[1, 2])?;
/// assert_eq!(s.write(&[3, 4]).unwrap_err().kind(), exi::ErrorKind::BufferFull);
/// assert_eq!(s.written(), [1, 2]);                 // all or nothing
/// s.write(&[3])?;                                   // exactly full
/// s.write(&[])?;                                    // nothing always fits
/// assert_eq!((s.written(), s.remaining()), (&[1, 2, 3][..], 0));
/// # Ok::<(), exi::Error>(())
/// ```
pub struct SliceSink<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> SliceSink<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        SliceSink { buf, len: 0 }
    }
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.len
    }
    /// Forgets what was written, keeping the buffer.
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Sink for SliceSink<'_> {
    fn write(&mut self, octets: &[u8]) -> Result<()> {
        if octets.len() > self.remaining() {
            return Err(Error::new(
                ErrorKind::BufferFull,
                "slice sink is full",
            ));
        }
        self.buf[self.len..self.len + octets.len()].copy_from_slice(octets);
        self.len += octets.len();
        Ok(())
    }
}

/// A sink over any `std::io::Write`. Errors become `ErrorKind::Io`.
#[cfg(feature = "std")]
pub struct IoSink<W: std::io::Write>(pub W);

#[cfg(feature = "std")]
impl<W: std::io::Write> Sink for IoSink<W> {
    fn write(&mut self, octets: &[u8]) -> Result<()> {
        self.0.write_all(octets).map_err(|_| {
            Error::new(ErrorKind::Io, "write to std::io::Write failed")
        })
    }
}

/// Which part of the stream a coder call belongs to (module
/// documentation).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Structure,
    /// The values of one element or attribute name.
    Value(NameId),
}

/// Writes n-bit values and octets. Implemented by each coding.
pub trait Encoder {
    /// Writes `value` as an n-bit unsigned integer of `width` bits
    /// (module documentation).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Internal` when `width > 32` or `value ≥ 2^width`: a
    /// bug in the caller. Sink errors pass through.
    fn nbit(&mut self, ch: Channel, value: u32, width: u8) -> Result<()>;

    /// Writes raw octets: the content of a binary value. In bit-packed
    /// coding each octet is 8 bits at the current bit position; in
    /// byte-aligned coding the octets as they are.
    fn octets(&mut self, ch: Channel, data: &[u8]) -> Result<()>;

    /// Pads with zero bits to the next octet boundary. A no-op when
    /// already aligned, which byte-aligned coding always is.
    fn align(&mut self) -> Result<()>;

    /// Bits written so far, padding included.
    fn bit_position(&self) -> u64;

    /// Pushes every complete octet to the sink and returns how many
    /// bits are still held because they do not fill an octet (0–7;
    /// always 0 for byte-aligned coding).
    fn flush(&mut self) -> Result<u8>;
}

/// Reads n-bit values and octets. Implemented by each coding.
pub trait Decoder {
    /// Reads an n-bit unsigned integer of `width` bits (`width ≤ 32`).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Truncated` when the input ends first;
    /// `ErrorKind::Internal` when `width > 32`.
    fn nbit(&mut self, ch: Channel, width: u8) -> Result<u32>;

    /// Fills `out` with octets (binary content).
    ///
    /// # Errors
    ///
    /// `ErrorKind::Truncated` when the input ends first; nothing is
    /// consumed then.
    fn octets(&mut self, ch: Channel, out: &mut [u8]) -> Result<()>;

    /// Skips to the next octet boundary.
    fn align(&mut self);

    /// Bits read so far.
    fn bit_position(&self) -> u64;

    /// The ASCII fast path for strings: consumes up to `max` octets
    /// below 128 that come next, appends them to `out` as characters,
    /// and returns how many. Only byte-aligned coding lays characters
    /// out this way; the default does nothing and returns 0.
    fn take_ascii(
        &mut self,
        ch: Channel,
        max: usize,
        out: &mut alloc::string::String,
    ) -> usize {
        let _ = (ch, max, out);
        0
    }
}

/// The number of bits an n-bit value needs to tell `count` alternatives
/// apart: `ceil(log2 count)`, and 0 when `count` is 0 or 1.
///
/// ```
/// use exi::coding::width;
/// assert_eq!([width(0), width(1), width(2), width(3), width(4), width(5)], [0, 0, 1, 2, 2, 3]);
/// assert_eq!((width(256), width(257)), (8, 9));
/// assert_eq!((width(1 << 31), width((1 << 31) + 1), width(u32::MAX)), (31, 32, 32));
/// ```
pub const fn width(count: u32) -> u8 {
    if count <= 1 { 0 } else { (32 - (count - 1).leading_zeros()) as u8 }
}
