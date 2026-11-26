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

use exi::strings::{GlobalConfig, GlobalStrings};
use exi_esp32c6::{MAX_DOCUMENT, decode, encode, grammar, options};

thread_local! {
    static BYTES: RefCell<[u8; MAX_DOCUMENT]> = const { RefCell::new([0; MAX_DOCUMENT]) };
    static READING: Cell<i64> = const { Cell::new(0) };
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
    let result = || -> exi::Result<usize> {
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let grammar = grammar()?;
        let attached = grammar.attach(&strings)?;
        BYTES.with(|bytes| {
            encode(&attached, &options()?, value, &mut *bytes.borrow_mut())
        })
    };
    result().map_or(-1, |len| len as i32)
}

/// Reads an EXI document from the scratch buffer. A zero status
/// publishes the value through `reading()`.
#[unsafe(no_mangle)]
pub extern "C" fn decode_reading(len: usize) -> i32 {
    if len > MAX_DOCUMENT {
        return -1;
    }
    let result = || -> exi::Result<i64> {
        let strings = GlobalStrings::new(GlobalConfig::device())?;
        let grammar = grammar()?;
        let attached = grammar.attach(&strings)?;
        BYTES.with(|bytes| {
            decode(&attached, &options()?, &bytes.borrow()[..len])
        })
    };
    match result() {
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
