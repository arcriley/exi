// src/coding/body.rs
//
// EXI body coding dispatch
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! The reader and writer hold one of these instead of a type parameter
//! per coding: the coding is known only once the header or the options
//! are read. Each call is one match.
//!
//! | `Coding` | Encoder | Decoder |
//! |---|---|---|
//! | `BitPacked` | [`BitEncoder`] | [`BitDecoder`] |
//! | `ByteAligned` | [`ByteEncoder`] | [`ByteDecoder`] |
//! | `PreCompression`, `Compression` | byte-aligned, into channel buffers (`crate::coding::block`) | [`ByteDecoder`] over one channel's octets |

use alloc::string::String;

#[cfg(feature = "compression")]
use super::block::BlockEncoder;
use crate::coding::bits::{BitDecoder, BitEncoder};
use crate::coding::byte_aligned::{ByteDecoder, ByteEncoder};
use crate::coding::{Channel, Decoder, Encoder, Sink};
use crate::{Coding, Options, Result};

/// Writes a body bit-packed or byte-aligned.
pub(crate) enum BodyEncoder<S: Sink> {
    Byte(ByteEncoder<S>),
    Bit(BitEncoder<S>),
    #[cfg(feature = "compression")]
    Block(BlockEncoder<S>),
}

impl<S: Sink> BodyEncoder<S> {
    /// The encoder for the options' coding; `level` is the DEFLATE
    /// level for compression. Without the feature `compression` the
    /// options are checked before this (an unsupported coding never
    /// reaches it), so byte alignment is the fallback.
    pub fn new(options: &Options, level: u32, sink: S) -> Self {
        let _ = level;
        match options.coding() {
            Coding::BitPacked => BodyEncoder::Bit(BitEncoder::new(sink)),
            #[cfg(feature = "compression")]
            Coding::PreCompression => BodyEncoder::Block(BlockEncoder::new(
                sink,
                options.block_size(),
                None,
            )),
            #[cfg(feature = "compression")]
            Coding::Compression => BodyEncoder::Block(BlockEncoder::new(
                sink,
                options.block_size(),
                Some(level),
            )),
            _ => BodyEncoder::Byte(ByteEncoder::new(sink)),
        }
    }
    pub fn sink(&self) -> &S {
        match self {
            BodyEncoder::Byte(e) => e.sink(),
            BodyEncoder::Bit(e) => e.sink(),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.sink(),
        }
    }
    pub fn sink_mut(&mut self) -> &mut S {
        match self {
            BodyEncoder::Byte(e) => e.sink_mut(),
            BodyEncoder::Bit(e) => e.sink_mut(),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.sink_mut(),
        }
    }
    /// Pads a bit-packed body to an octet, and returns the sink.
    pub fn into_sink(self) -> S {
        match self {
            BodyEncoder::Byte(e) => e.into_sink(),
            BodyEncoder::Bit(e) => e.into_sink(),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.into_sink(),
        }
    }
}

impl<S: Sink> Encoder for BodyEncoder<S> {
    fn nbit(&mut self, ch: Channel, value: u32, width: u8) -> Result<()> {
        match self {
            BodyEncoder::Byte(e) => e.nbit(ch, value, width),
            BodyEncoder::Bit(e) => e.nbit(ch, value, width),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.nbit(ch, value, width),
        }
    }
    fn octets(&mut self, ch: Channel, data: &[u8]) -> Result<()> {
        match self {
            BodyEncoder::Byte(e) => e.octets(ch, data),
            BodyEncoder::Bit(e) => e.octets(ch, data),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.octets(ch, data),
        }
    }
    fn align(&mut self) -> Result<()> {
        match self {
            BodyEncoder::Byte(e) => e.align(),
            BodyEncoder::Bit(e) => e.align(),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.align(),
        }
    }
    fn bit_position(&self) -> u64 {
        match self {
            BodyEncoder::Byte(e) => e.bit_position(),
            BodyEncoder::Bit(e) => e.bit_position(),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.bit_position(),
        }
    }
    fn flush(&mut self) -> Result<u8> {
        match self {
            BodyEncoder::Byte(e) => e.flush(),
            BodyEncoder::Bit(e) => e.flush(),
            #[cfg(feature = "compression")]
            BodyEncoder::Block(e) => e.flush(),
        }
    }
}

/// Reads a body bit-packed or byte-aligned, over the two parts of the
/// reader's input.
pub(crate) enum BodyDecoder<'a> {
    Byte(ByteDecoder<'a>),
    Bit(BitDecoder<'a>),
}

impl<'a> BodyDecoder<'a> {
    /// Reads `first` then `second`, starting `bit` bits (0–7) into
    /// `first`; `bit` is 0 unless the coding is bit-packed.
    pub fn new(
        coding: Coding,
        first: &'a [u8],
        second: &'a [u8],
        bit: u8,
    ) -> Self {
        match coding {
            Coding::BitPacked => {
                BodyDecoder::Bit(BitDecoder::split(first, second, bit as u64))
            }
            _ => BodyDecoder::Byte(ByteDecoder::new(first, second)),
        }
    }
}

impl Decoder for BodyDecoder<'_> {
    fn nbit(&mut self, ch: Channel, width: u8) -> Result<u32> {
        match self {
            BodyDecoder::Byte(d) => d.nbit(ch, width),
            BodyDecoder::Bit(d) => d.nbit(ch, width),
        }
    }
    fn octets(&mut self, ch: Channel, out: &mut [u8]) -> Result<()> {
        match self {
            BodyDecoder::Byte(d) => d.octets(ch, out),
            BodyDecoder::Bit(d) => d.octets(ch, out),
        }
    }
    fn align(&mut self) {
        match self {
            BodyDecoder::Byte(d) => d.align(),
            BodyDecoder::Bit(d) => d.align(),
        }
    }
    /// For a bit-packed body, counted from the start of `first`,
    /// including the starting offset.
    fn bit_position(&self) -> u64 {
        match self {
            BodyDecoder::Byte(d) => d.bit_position(),
            BodyDecoder::Bit(d) => d.bit_position(),
        }
    }
    fn take_ascii(
        &mut self,
        ch: Channel,
        max: usize,
        out: &mut String,
    ) -> usize {
        match self {
            BodyDecoder::Byte(d) => d.take_ascii(ch, max, out),
            BodyDecoder::Bit(_) => 0,
        }
    }
}
