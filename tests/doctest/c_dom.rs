// tests/doctest/c_dom.rs
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
pub(crate) use exi::{capi::dom::*, dom::raw::*};

macro_rules! s {
    ($text:literal) => {
        concat!($text, "\0").as_ptr()
    };
}
pub(crate) use s;

/// Copies a malloc'd string from the API and frees it.
///
/// The pointer must be a non-null string returned by the API.
pub(crate) unsafe fn text(ptr: *mut u8) -> String {
    assert!(!ptr.is_null());
    unsafe {
        let text =
            core::ffi::CStr::from_ptr(ptr.cast()).to_str().unwrap().to_owned();
        exiFree(ptr.cast());
        text
    }
}

/// Copies the name of a live, named node.
pub(crate) unsafe fn name(node: *const RawNode) -> String {
    unsafe { core::ffi::CStr::from_ptr((*node).name.cast()) }
        .to_str()
        .unwrap()
        .to_owned()
}

/// Copies the names of a live node's children.
pub(crate) unsafe fn kids(node: *const RawNode) -> Vec<String> {
    let mut names = Vec::new();
    unsafe {
        let mut child = (*node).children;
        while !child.is_null() {
            names.push(name(child));
            child = (*child).next;
        }
    }
    names
}
