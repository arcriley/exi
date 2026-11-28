// tests/wasm/src/bin/emscripten.rs
//
// Emscripten binary entry point for wasm32-unknown-emscripten tests
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.

#[path = "../lib.rs"]
mod imp;

pub use imp::*;

fn main() {}
