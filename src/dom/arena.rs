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

use alloc::vec::Vec;
use core::alloc::Layout;
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
    assert!(HDR_BYTES.is_multiple_of(8));
    assert!(align_of::<ElemBox>() <= 8);
    assert!(align_of::<AttrBox>() <= 8);
    assert!(align_of::<NodeBox>() <= 8);
    assert!(size_of::<ElemBox>().is_multiple_of(8));
    assert!(size_of::<AttrBox>().is_multiple_of(8));
    assert!(size_of::<NodeBox>().is_multiple_of(8));
};

/// A block of `bytes` (a multiple of 8), 8-aligned and not zeroed.
/// Full-size blocks come from the thread's cache when it has one.
fn block_alloc(bytes: usize) -> *mut u8 {
    #[cfg(feature = "std")]
    if bytes == MAX_CHUNK_BYTES {
        if let Some(p) = cache::take() {
            return p;
        }
    }
    let layout = Layout::from_size_align(bytes, 8).expect("arena chunk size");
    // SAFETY: `bytes > 0`.
    let p = unsafe { alloc::alloc::alloc(layout) };
    if p.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    p
}

/// Frees a block from [`block_alloc`]; full-size blocks go to the
/// thread's cache while it has room.
///
/// # Safety
///
/// `p` came from `block_alloc(bytes)` and is not used again.
unsafe fn block_free(p: *mut u8, bytes: usize) {
    #[cfg(feature = "std")]
    if bytes == MAX_CHUNK_BYTES && cache::give(p) {
        return;
    }
    let layout = Layout::from_size_align(bytes, 8).expect("arena chunk size");
    // SAFETY: the caller's contract.
    unsafe { alloc::alloc::dealloc(p, layout) };
}

/// Freed full-size chunks kept for the next documents on the thread.
///
/// Building a tree writes every byte of its chunks. Memory the
/// allocator has just returned to the system comes back as fresh pages,
/// and the kernel faults in and zeroes each one: for a 24 MiB tree that
/// costs more than parsing the document. Keeping freed chunks, as
/// mimalloc or jemalloc keep freed spans, lets the next document reuse
/// pages that are already mapped. At most [`CACHE_BYTES`] are kept per
/// thread; [`release_cache`] returns them.
#[cfg(feature = "std")]
mod cache {
    use super::MAX_CHUNK_BYTES;
    use core::alloc::Layout;
    use std::cell::RefCell;
    use std::vec::Vec;

    pub const CACHE_BYTES: usize = 32 << 20;

    struct Blocks(Vec<*mut u8>);

    impl Drop for Blocks {
        fn drop(&mut self) {
            free(&mut self.0);
        }
    }

    fn free(blocks: &mut Vec<*mut u8>) {
        let layout = Layout::from_size_align(MAX_CHUNK_BYTES, 8).unwrap();
        for p in blocks.drain(..) {
            // SAFETY: every cached block is a full-size block from the
            // global allocator, owned by the cache alone.
            unsafe { alloc::alloc::dealloc(p, layout) };
        }
    }

    std::thread_local! {
        static BLOCKS: RefCell<Blocks> = const { RefCell::new(Blocks(Vec::new())) };
    }

    pub fn take() -> Option<*mut u8> {
        BLOCKS.try_with(|b| b.borrow_mut().0.pop()).ok().flatten()
    }

    /// Keeps `p` if there is room; `false` when the caller must free it.
    pub fn give(p: *mut u8) -> bool {
        BLOCKS
            .try_with(|b| {
                let mut b = b.borrow_mut();
                if (b.0.len() + 1) * MAX_CHUNK_BYTES > CACHE_BYTES {
                    return false;
                }
                b.0.push(p);
                true
            })
            .unwrap_or(false)
    }

    pub fn release() {
        let _ = BLOCKS.try_with(|b| free(&mut b.borrow_mut().0));
    }
}

/// Returns the memory this thread keeps for building trees to the
/// allocator. A dropped document's large chunks (256 KiB each, up to
/// 32 MiB per thread) are kept for the next document, because reusing
/// mapped pages is faster than having the system fault in fresh ones.
///
/// ```
/// use exi::dom::Document;
/// # use exi::strings::{GlobalConfig, GlobalStrings};
/// # let strings = GlobalStrings::new(GlobalConfig::new())?;
/// let xml = format!("<r>{}</r>", "<e a='1'>text</e>".repeat(20_000));
/// drop(Document::parse_xml(&strings, xml.as_bytes())?); // its chunks are kept
/// exi::dom::release_cache();                          // and now freed
/// # Ok::<(), exi::Error>(())
/// ```
#[cfg(feature = "std")]
pub fn release_cache() {
    cache::release();
}

/// A chunk of at least `min_bytes`, not zeroed: every node is written
/// whole with `ptr::write` before anything reads it, so filling the
/// chunk first would only double the memory traffic of building a tree.
fn alloc_chunk(min_bytes: usize) -> *mut ChunkHeader {
    let words = min_bytes.div_ceil(8);
    let hdr = block_alloc(words * 8) as *mut ChunkHeader;
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
    // SAFETY: `hdr` came from `alloc_chunk`, `cap_u64s` words long.
    unsafe {
        let words = (*hdr).cap_u64s;
        block_free(hdr as *mut u8, words * 8);
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
    /// Byte chunks `(start, capacity)` for text too long to store inline;
    /// the last is the active one.
    text: Vec<(*mut u8, usize)>,
    /// Next free byte in the active text chunk.
    text_pos: usize,
}

/// Text up to this many bytes goes in the text arena; longer text gets
/// a heap buffer of its own.
pub(crate) const TEXT_MAX: usize = 4096;

impl NodeArena {
    pub fn new() -> Self {
        NodeArena {
            chunks: Vec::new(),
            foreign: Vec::new(),
            adopted_out: false,
            free_elem: Vec::new(),
            free_attr: Vec::new(),
            free_node: Vec::new(),
            text: Vec::new(),
            text_pos: 0,
        }
    }

    /// A NUL-terminated copy of `s` (at most [`TEXT_MAX`] bytes), alive
    /// until the document is dropped. Text is never moved or freed
    /// alone: a node that stops using it leaves it in place, and a node
    /// adopted by another document copies its text out first.
    #[inline]
    pub fn alloc_text(&mut self, s: &[u8]) -> *mut u8 {
        debug_assert!(s.len() <= TEXT_MAX);
        let need = s.len() + 1;
        let p = match self.text.last() {
            Some(&(base, cap)) if self.text_pos + need <= cap => {
                // SAFETY: `text_pos + need <= cap` bytes of this chunk.
                let p = unsafe { base.add(self.text_pos) };
                self.text_pos += need;
                p
            }
            _ => self.text_slow(need),
        };
        // SAFETY: `p` has `need` writable bytes that `s` does not overlap.
        unsafe {
            ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
            *p.add(s.len()) = 0;
        }
        p
    }

    #[cold]
    fn text_slow(&mut self, need: usize) -> *mut u8 {
        // The same growth schedule as node chunks: 4 KiB doubling to
        // 256 KiB.
        let n = self.text.len();
        let cap = (4096usize << n.min(6))
            .min(MAX_CHUNK_BYTES)
            .max(need.next_multiple_of(8));
        let base = block_alloc(cap);
        self.text.push((base, cap));
        self.text_pos = need;
        base
    }

    /// Frees every text chunk.
    fn drop_text(&mut self) {
        for (base, cap) in self.text.drain(..) {
            // SAFETY: from `block_alloc(cap)` in `text_slow`.
            unsafe { block_free(base, cap) };
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
        // Adopted nodes copied their text out, so no other document uses
        // these chunks.
        self.drop_text();
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
