// tests/doctest/c_api.rs
//
// C API doctest fixtures
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//

pub(crate) use core::ptr::{null, null_mut};
use exi::capi::*;

pub(crate) fn s(text: &'static str) -> exi_str {
    exi_str { ptr: text.as_ptr().cast(), len: text.len() }
}

pub(crate) fn byte() -> exi_options {
    let mut options = unsafe { core::mem::zeroed() };
    unsafe { exi_options_init(&mut options) };
    options.coding = EXI_CODING_BYTE_ALIGNED;
    options
}

pub(crate) fn writer_config(options: &exi_options) -> exi_writer_config {
    let mut cfg: exi_writer_config = unsafe { core::mem::zeroed() };
    cfg.size = core::mem::size_of::<exi_writer_config>() as u32;
    cfg.options = options;
    cfg.header = EXI_HDR_NO_OPTIONS;
    cfg
}

pub(crate) fn reader_config(options: &exi_options) -> exi_reader_config {
    let mut cfg: exi_reader_config = unsafe { core::mem::zeroed() };
    cfg.size = core::mem::size_of::<exi_reader_config>() as u32;
    cfg.options = options;
    cfg
}
