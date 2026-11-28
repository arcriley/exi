// tests/wasm/src/lib.rs
//
// Browser integration test harness
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Browser test entry points for the same sensor codec used on the
//! XIAO.

use std::cell::{Cell, RefCell};

use exi::grammar::{GrammarSet, Grammars};
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{Coding, Options, Result};
use exi_esp32c6::{MAX_DOCUMENT, decode, encode, grammar, reading_options};

/// The coding of this build: the browser suite builds both, as the
/// firmware can.
const CODING: Coding = if cfg!(feature = "bit-packed") {
    Coding::BitPacked
} else {
    Coding::ByteAligned
};

thread_local! {
    // One string pool and grammar for every call, as on the device.
    static CODEC: Result<(GlobalStrings, GrammarSet, Options)> = Ok((
        GlobalStrings::new(GlobalConfig::device())?,
        grammar()?,
        reading_options(CODING)?,
    ));
    static BYTES: RefCell<[u8; MAX_DOCUMENT]> = const { RefCell::new([0; MAX_DOCUMENT]) };
    static READING: Cell<i64> = const { Cell::new(0) };
}

fn with_codec<T>(
    f: impl FnOnce(&Grammars<'_>, &Options) -> Result<T>,
) -> Result<T> {
    CODEC.with(|codec| {
        let (strings, set, opts) = codec.as_ref().map_err(Clone::clone)?;
        f(&set.attach(strings)?, opts)
    })
}

/// Scratch space of `capacity()` bytes. The browser accesses it between
/// calls, when the guest has no outstanding borrow of the buffer.
#[unsafe(no_mangle)]
pub extern "C" fn buffer() -> *mut u8 {
    BYTES.with(|bytes| bytes.borrow_mut().as_mut_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn capacity() -> usize {
    MAX_DOCUMENT
}

/// Writes one EXI document into the scratch buffer and returns its
/// length.
#[unsafe(no_mangle)]
pub extern "C" fn encode_reading(value: i64) -> i32 {
    with_codec(|g, opts| {
        BYTES.with(|bytes| encode(g, opts, value, &mut *bytes.borrow_mut()))
    })
    .map_or(-1, |len| len as i32)
}

/// Reads an EXI document from the scratch buffer. A zero status
/// publishes the value through `reading()`.
#[unsafe(no_mangle)]
pub extern "C" fn decode_reading(len: usize) -> i32 {
    if len > MAX_DOCUMENT {
        return -1;
    }
    let result = with_codec(|g, opts| {
        BYTES.with(|bytes| decode(g, opts, &bytes.borrow()[..len]))
    });
    match result {
        Ok(value) => {
            READING.set(value);
            0
        }
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn reading() -> i64 {
    READING.get()
}

#[cfg(feature = "exig")]
mod exig_harness {
    use core::slice;

    use exi::grammar::{GrammarSet, Grammars};
    use exi::strings::{GlobalConfig, GlobalStrings};
    use exi::{Options, Reader, ReaderConfig, Result, Step};

    use super::CODING;

    fn count_events(
        grammars: &Grammars<'_>,
        opts: Options,
        bytes: &[u8],
    ) -> Result<i32> {
        let mut reader =
            Reader::<256>::new(ReaderConfig::new(grammars).options(opts))?;
        let mut off = 0;
        let mut ended = false;
        let mut events: i32 = 0;

        loop {
            if !ended {
                if off < bytes.len() {
                    let n = reader.feed(&bytes[off..]);
                    off += n;
                }
                if off == bytes.len() {
                    reader.end_input();
                    ended = true;
                }
            }

            loop {
                match reader.next()? {
                    Step::Event(_, _) => {
                        events = events.saturating_add(1);
                    }
                    Step::NeedInput => break,
                    Step::End => return Ok(events),
                }
            }
        }
    }

    unsafe fn input_slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
        if len == 0 || ptr.is_null() {
            &[]
        } else {
            unsafe { slice::from_raw_parts(ptr, len) }
        }
    }

    /// Allocates `len` bytes in linear memory for host-supplied EXI/EXIG inputs.
    #[unsafe(no_mangle)]
    pub extern "C" fn alloc_buf(len: usize) -> *mut u8 {
        if len == 0 {
            return core::ptr::NonNull::<u8>::dangling().as_ptr();
        }
        let mut buf = Vec::<u8>::with_capacity(len);
        let ptr = buf.as_mut_ptr();
        core::mem::forget(buf);
        ptr
    }

    /// Frees a buffer previously returned by `alloc_buf`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn free_buf(ptr: *mut u8, len: usize) {
        if len == 0 || ptr.is_null() {
            return;
        }
        unsafe {
            drop(Vec::from_raw_parts(ptr, 0, len));
        }
    }

    /// Parses a schema-less EXI stream, returning event count (`>= 0`) or `-1` on error.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wasm_parse_exi_schemaless(
        ptr: *const u8,
        len: usize,
    ) -> i32 {
        let bytes = unsafe { input_slice(ptr, len) };
        let res = (|| -> Result<i32> {
            let strings = GlobalStrings::new(GlobalConfig::new())?;
            let set = GrammarSet::schemaless();
            let grammars = set.attach(&strings)?;
            let opts = Options::builder().coding(CODING).build()?;
            count_events(&grammars, opts, bytes)
        })();
        res.unwrap_or(-1)
    }

    /// Parses an `.exig` grammar file and verifies `to_exig()` and `digest()` round-trip.
    /// Returns `0` on success or `-1` on error.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wasm_parse_exig(
        ptr: *const u8,
        len: usize,
    ) -> i32 {
        let bytes = unsafe { input_slice(ptr, len) };
        let res = (|| -> Result<i32> {
            let loaded = GrammarSet::from_exig(bytes)?;
            let roundtrip = loaded.to_exig()?;
            if roundtrip != bytes {
                return Ok(-2);
            }
            let d1 = loaded.digest()?;
            let d2 = GrammarSet::from_exig(&roundtrip)?.digest()?;
            if d1 != d2 {
                return Ok(-3);
            }
            Ok(0)
        })();
        res.unwrap_or(-1)
    }

    /// Loads an `.exig` grammar and decodes a schema-informed EXI stream with it.
    /// Returns event count (`>= 0`) on success or `-1` on error.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wasm_parse_exi_with_exig(
        exig_ptr: *const u8,
        exig_len: usize,
        exi_ptr: *const u8,
        exi_len: usize,
        strict: i32,
    ) -> i32 {
        let exig_bytes = unsafe { input_slice(exig_ptr, exig_len) };
        let exi_bytes = unsafe { input_slice(exi_ptr, exi_len) };
        let res = (|| -> Result<i32> {
            let strings = GlobalStrings::new(GlobalConfig::new())?;
            let set = GrammarSet::from_exig(exig_bytes)?;
            let grammars = set.attach(&strings)?;
            let opts = Options::builder()
                .coding(CODING)
                .strict(strict != 0)
                .build()?;
            count_events(&grammars, opts, exi_bytes)
        })();
        res.unwrap_or(-1)
    }
}
