// src/dom/arena.rs
//
// Chunked bump allocator for DOM nodes with refcounted chunks for adoption
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Allocates [`ElemBox`], [`AttrBox`] and [`NodeBox`] inside stable,
//! 8-byte-aligned heap chunks.
//!
//! Node addresses never move once allocated, so C pointers (`exiNodePtr`,
//! `exiAttrPtr`) remain valid across growth, unlinking and
//! [`Document::adopt`](super::Document::adopt). Each chunk counts how many
//! live nodes still reside in it (`live`) and whether its creating document
//! is still alive (`has_owner`). When a document with no adopted nodes is
//! dropped, every chunk it allocated is freed directly in `O(chunks)`.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem::{align_of, size_of};
use core::ptr;

use super::raw::{
    AttrBox, EXI_ATTRIBUTE_NODE, EXI_DTD_NODE, EXI_ELEMENT_NODE, ElemBox,
    NodeBox, RawCommon,
};

#[repr(C, align(8))]
pub(crate) struct ChunkHeader {
    /// Allocation length in `u64` words (header included).
    cap_u64s: usize,
    /// Next bump offset in bytes from the start of the chunk.
    pos: usize,
    /// Live nodes in this chunk across all documents.
    live: u32,
    /// True while the document that allocated this chunk is still alive.
    has_owner: bool,
}

const HDR_BYTES: usize = size_of::<ChunkHeader>();
const MAX_CHUNK_BYTES: usize = 256 * 1024;

const _: () = {
    assert!(HDR_BYTES % 8 == 0);
    assert!(align_of::<ElemBox>() <= 8);
    assert!(align_of::<AttrBox>() <= 8);
    assert!(align_of::<NodeBox>() <= 8);
    assert!(size_of::<ElemBox>() % 8 == 0);
    assert!(size_of::<AttrBox>() % 8 == 0);
    assert!(size_of::<NodeBox>() % 8 == 0);
};

fn alloc_chunk(min_bytes: usize) -> *mut ChunkHeader {
    let words = min_bytes.div_ceil(8);
    let buf: Box<[u64]> = alloc::vec![0u64; words].into_boxed_slice();
    let hdr = Box::into_raw(buf) as *mut ChunkHeader;
    // SAFETY: `hdr` is freshly allocated with `words * 8 >= HDR_BYTES` bytes
    // and 8-byte alignment.
    unsafe {
        ptr::write(
            hdr,
            ChunkHeader {
                cap_u64s: words,
                pos: HDR_BYTES,
                live: 0,
                has_owner: true,
            },
        );
    }
    hdr
}

/// # Safety
///
/// `hdr` came from [`alloc_chunk`], has `live == 0` and `!has_owner`, and
/// is never used again.
unsafe fn free_chunk(hdr: *mut ChunkHeader) {
    // SAFETY: `hdr` was created from `Box::<[u64]>::into_raw` with `cap_u64s`.
    unsafe {
        let words = (*hdr).cap_u64s;
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            hdr as *mut u64,
            words,
        )));
    }
}

#[inline]
unsafe fn chunk_contains(hdr: *mut ChunkHeader, p: *const RawCommon) -> bool {
    let start = hdr as usize;
    // SAFETY: `hdr` is a live chunk header.
    let end = start + unsafe { (*hdr).cap_u64s } * 8;
    let addr = p as usize;
    addr >= start + HDR_BYTES && addr < end
}

/// Per-document chunked node arena.
pub(crate) struct NodeArena {
    /// Chunks allocated by this document; the last is the active bump chunk.
    chunks: Vec<*mut ChunkHeader>,
    /// Chunks allocated by other documents holding nodes adopted into this one.
    foreign: Vec<*mut ChunkHeader>,
    /// True once any node from `chunks` has been adopted into another document.
    adopted_out: bool,
    free_elem: Vec<(*mut ElemBox, *mut ChunkHeader)>,
    free_attr: Vec<(*mut AttrBox, *mut ChunkHeader)>,
    free_node: Vec<(*mut NodeBox, *mut ChunkHeader)>,
}

impl NodeArena {
    pub fn new() -> Self {
        NodeArena {
            chunks: Vec::new(),
            foreign: Vec::new(),
            adopted_out: false,
            free_elem: Vec::new(),
            free_attr: Vec::new(),
            free_node: Vec::new(),
        }
    }

    fn next_chunk_bytes(&self, need: usize) -> usize {
        // Growth schedule: 4 KiB, 4 KiB, 8 KiB, 16 KiB, 32 KiB, 64 KiB,
        // 128 KiB, 256 KiB (capped). Keeps tiny documents small while
        // amortizing 1 MiB documents to ~85 allocations.
        let planned = match self.chunks.len() {
            0 | 1 => 4096,
            n => (4096usize << (n - 1).min(6)).min(MAX_CHUNK_BYTES),
        };
        planned.max(HDR_BYTES + need)
    }

    #[inline]
    fn bump(&mut self, bytes: usize, live_add: u32) -> *mut u8 {
        if let Some(&hdr) = self.chunks.last() {
            // SAFETY: `hdr` is a live owned chunk.
            unsafe {
                let cap = (*hdr).cap_u64s * 8;
                let pos = (*hdr).pos;
                if pos + bytes <= cap {
                    (*hdr).pos = pos + bytes;
                    (*hdr).live += live_add;
                    return (hdr as *mut u8).add(pos);
                }
            }
        }
        self.bump_slow(bytes, live_add)
    }

    #[cold]
    fn bump_slow(&mut self, bytes: usize, live_add: u32) -> *mut u8 {
        let size = self.next_chunk_bytes(bytes);
        let hdr = alloc_chunk(size);
        self.chunks.push(hdr);
        // SAFETY: `size >= HDR_BYTES + bytes`.
        unsafe {
            let pos = (*hdr).pos;
            (*hdr).pos = pos + bytes;
            (*hdr).live += live_add;
            (hdr as *mut u8).add(pos)
        }
    }

    #[inline]
    pub fn alloc_elem(&mut self, val: ElemBox) -> *mut RawCommon {
        let p = if let Some((p, hdr)) = self.free_elem.pop() {
            // SAFETY: `hdr` is an owned live chunk and `p` is an unallocated slot.
            unsafe { (*hdr).live += 1 };
            p
        } else {
            self.bump(size_of::<ElemBox>(), 1) as *mut ElemBox
        };
        // SAFETY: `p` is properly aligned and writable for `ElemBox`.
        unsafe { ptr::write(p, val) };
        p as *mut RawCommon
    }

    #[inline]
    pub fn alloc_attr(&mut self, val: AttrBox) -> *mut RawCommon {
        let p = if let Some((p, hdr)) = self.free_attr.pop() {
            // SAFETY: as `alloc_elem`.
            unsafe { (*hdr).live += 1 };
            p
        } else {
            self.bump(size_of::<AttrBox>(), 1) as *mut AttrBox
        };
        // SAFETY: `p` is properly aligned and writable for `AttrBox`.
        unsafe { ptr::write(p, val) };
        p as *mut RawCommon
    }

    #[inline]
    pub fn alloc_node(&mut self, val: NodeBox) -> *mut RawCommon {
        let p = if let Some((p, hdr)) = self.free_node.pop() {
            // SAFETY: as `alloc_elem`.
            unsafe { (*hdr).live += 1 };
            p
        } else {
            self.bump(size_of::<NodeBox>(), 1) as *mut NodeBox
        };
        // SAFETY: `p` is properly aligned and writable for `NodeBox`.
        unsafe { ptr::write(p, val) };
        p as *mut RawCommon
    }

    /// Allocates an attribute and its child text node contiguously when
    /// both free lists are empty (the common parse path).
    #[inline]
    pub fn alloc_attr_and_node(
        &mut self,
        attr: AttrBox,
        text: NodeBox,
    ) -> (*mut RawCommon, *mut RawCommon) {
        if self.free_attr.is_empty() && self.free_node.is_empty() {
            let base =
                self.bump(size_of::<AttrBox>() + size_of::<NodeBox>(), 2);
            let ap = base as *mut AttrBox;
            // SAFETY: `size_of::<AttrBox>()` is a multiple of 8, so `tp` is
            // 8-byte aligned and both writes lie inside the bumped region.
            unsafe {
                let tp = base.add(size_of::<AttrBox>()) as *mut NodeBox;
                ptr::write(ap, attr);
                ptr::write(tp, text);
                (ap as *mut RawCommon, tp as *mut RawCommon)
            }
        } else {
            let ap = self.alloc_attr(attr);
            let tp = self.alloc_node(text);
            (ap, tp)
        }
    }

    fn find_chunk(&self, p: *const RawCommon) -> Option<(*mut ChunkHeader, bool)> {
        // Check owned chunks in reverse (recent allocations are most likely).
        for &hdr in self.chunks.iter().rev() {
            // SAFETY: every chunk in `chunks` is live.
            if unsafe { chunk_contains(hdr, p) } {
                return Some((hdr, true));
            }
        }
        for &hdr in self.foreign.iter().rev() {
            // SAFETY: every chunk in `foreign` is live while referenced.
            if unsafe { chunk_contains(hdr, p) } {
                return Some((hdr, false));
            }
        }
        None
    }

    /// Releases a single arena-allocated node slot (called by `free_raw`
    /// after its heap payload has already been dropped).
    ///
    /// # Safety
    ///
    /// `p` is a live non-DTD node owned by this document, whose external
    /// heap payload has already been dropped, and is never read again.
    pub unsafe fn free_slot(&mut self, p: *mut RawCommon) {
        // SAFETY: `p` is a live node in this arena.
        unsafe {
            let kind = (*p).r#type;
            debug_assert_ne!(kind, EXI_DTD_NODE);
            let Some((hdr, owned)) = self.find_chunk(p) else {
                return;
            };
            (*hdr).live -= 1;
            if owned {
                match kind {
                    EXI_ELEMENT_NODE => {
                        self.free_elem.push((p as *mut ElemBox, hdr));
                    }
                    EXI_ATTRIBUTE_NODE => {
                        self.free_attr.push((p as *mut AttrBox, hdr));
                    }
                    _ => {
                        self.free_node.push((p as *mut NodeBox, hdr));
                    }
                }
            } else if (*hdr).live == 0 && !(*hdr).has_owner {
                self.foreign.retain(|&h| h != hdr);
                free_chunk(hdr);
            }
        }
    }

    /// Records that `p` (a non-DTD node) moved from `from` into `to`.
    ///
    /// # Safety
    ///
    /// `p` is a live arena-allocated node currently owned by `from`.
    pub unsafe fn transfer_node(
        from: &mut NodeArena,
        to: &mut NodeArena,
        p: *const RawCommon,
    ) {
        let Some((hdr, owned)) = from.find_chunk(p) else {
            return;
        };
        if owned {
            from.adopted_out = true;
        }
        if !to.chunks.contains(&hdr) && !to.foreign.contains(&hdr) {
            to.foreign.push(hdr);
        }
    }

    /// Releases all chunks belonging to this document when the document
    /// is dropped.
    ///
    /// # Safety
    ///
    /// Called once in `Document::drop`, after every external heap payload
    /// (`nsDef`, `Payload::Text`/`Pi`/`Name`/`Typed`, `DtdBox`) has been
    /// dropped.
    pub unsafe fn drop_all(&mut self, ptrs: &[*mut RawCommon]) {
        // SAFETY: fast path when no nodes crossed document boundaries.
        unsafe {
            if !self.adopted_out && self.foreign.is_empty() {
                for hdr in self.chunks.drain(..) {
                    free_chunk(hdr);
                }
                return;
            }
            for &p in ptrs.iter().skip(1) {
                if !p.is_null() && (*p).r#type != EXI_DTD_NODE {
                    if let Some((hdr, _)) = self.find_chunk(p) {
                        (*hdr).live -= 1;
                    }
                }
            }
            for hdr in self.chunks.drain(..) {
                (*hdr).has_owner = false;
                if (*hdr).live == 0 {
                    free_chunk(hdr);
                }
            }
            for hdr in self.foreign.drain(..) {
                if (*hdr).live == 0 && !(*hdr).has_owner {
                    free_chunk(hdr);
                }
            }
        }
    }
}
