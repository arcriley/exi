// src/input.rs
//
// Input ring, cylical fixed size FIFO buffer
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Every reader (EXI and XML text) owns one ring of `N` octets, stored
//! inline, so a reader declared as a local variable keeps its input on
//! the stack. Input memory never grows with the size of a document.
//!
//! # Rules
//!
//! - `N` is a power of two, at least 64. Ring arithmetic is a mask. A
//!   ring of another size does not compile (a `const` assertion).
//! - The filled region runs from the read cursor to the write cursor,
//!   and may wrap around the end of the buffer. [`InputRing::filled`]
//!   returns it as two slices.
//! - Input comes in two ways: [`InputRing::feed`] copies as much as
//!   fits; or [`InputRing::buffer`] exposes a contiguous free region,
//!   the caller reads straight into it (from a socket or file), and
//!   [`InputRing::commit`] makes it visible.
//! - A decoder reads through a checkpoint. When an event is incomplete,
//!   the reader rewinds to the checkpoint and asks for more input; no
//!   octets move. Once an event completes, the reader
//!   [`consume`](InputRing::consume)s its octets, and that space is
//!   free again.
//! - A value longer than the ring is decoded in pieces into its
//!   destination (the global tables or a scratch buffer) as octets
//!   arrive, so the ring size limits nothing about value sizes.

/// A ring buffer of `N` octets.
///
/// ```
/// use exi::input::InputRing;
/// let mut r = InputRing::<64>::new();
/// assert_eq!((r.len(), r.free()), (0, 64));
/// assert_eq!(r.feed(&[1; 100]), 64);           // takes what fits
/// assert_eq!((r.len(), r.free()), (64, 0));
/// assert_eq!(r.feed(&[2]), 0);                 // full
/// r.consume(60);
/// assert_eq!(r.feed(&[3; 10]), 10);            // wraps around
/// let (a, b) = r.filled();
/// assert_eq!((a.len(), b.len()), (4, 10));     // the tail, then the wrapped part
/// assert_eq!(b, [3; 10]);
/// ```
///
/// Reading in place:
///
/// ```
/// use exi::input::InputRing;
/// let mut r = InputRing::<64>::new();
/// let free = r.buffer();
/// assert_eq!(free.len(), 64);
/// free[..3].copy_from_slice(b"abc");
/// r.commit(3);
/// assert_eq!(r.filled().0, b"abc");
/// r.consume(3);
/// assert_eq!(r.buffer().len(), 64);            // empty: the cursors moved back to 0
/// r.commit(10);
/// r.consume(5);
/// assert_eq!(r.buffer().len(), 54);            // from the write cursor to the end
/// ```
///
/// Sizes that are not a power of two, or below 64, do not compile:
///
/// ```compile_fail
/// let _ = exi::input::InputRing::<48>::new();
/// ```
///
/// ```compile_fail
/// let _ = exi::input::InputRing::<32>::new();
/// ```
pub struct InputRing<const N: usize> {
    buf: [u8; N],
    // Counters that only grow (wrapping); the index into `buf` is the
    // counter masked by N − 1, and the filled length is `write − read`.
    read: usize,
    write: usize,
}

impl<const N: usize> InputRing<N> {
    /// Compile-time check of `N`; readers evaluate it in their
    /// constructors.
    pub const VALID: () = assert!(
        N.is_power_of_two() && N >= 64,
        "ring size: a power of two, at least 64"
    );

    pub const fn new() -> Self {
        let () = Self::VALID;
        InputRing { buf: [0; N], read: 0, write: 0 }
    }
    /// Octets filled.
    pub const fn len(&self) -> usize {
        self.write.wrapping_sub(self.read)
    }
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Octets free.
    pub const fn free(&self) -> usize {
        N - self.len()
    }
    /// Copies as much of `data` as fits and returns how much.
    pub fn feed(&mut self, data: &[u8]) -> usize {
        let mut done = 0;
        while done < data.len() {
            let free = self.buffer();
            let n = free.len().min(data.len() - done);
            if n == 0 {
                break;
            }
            free[..n].copy_from_slice(&data[done..done + n]);
            self.commit(n);
            done += n;
        }
        done
    }
    /// The free region from the write cursor up to the end of the
    /// buffer or the read cursor, whichever comes first. When the ring
    /// is empty both cursors first move to 0, so the whole buffer is
    /// offered.
    pub fn buffer(&mut self) -> &mut [u8] {
        if self.is_empty() {
            self.read = 0;
            self.write = 0;
        }
        let w = self.write & (N - 1);
        let end = if self.free() == 0 {
            w
        } else {
            let r = self.read & (N - 1);
            if r > w { r } else { N }
        };
        &mut self.buf[w..end]
    }
    /// Makes `n` octets written into [`buffer`](Self::buffer) part of the filled region.
    ///
    /// # Panics
    ///
    /// When `n` exceeds the length `buffer` returned: a bug in the
    /// caller.
    pub fn commit(&mut self, n: usize) {
        let w = self.write & (N - 1);
        let r = self.read & (N - 1);
        let room = if self.free() == 0 {
            0
        } else if r > w {
            r - w
        } else {
            N - w
        };
        assert!(n <= room, "commit beyond the buffer given");
        self.write = self.write.wrapping_add(n);
    }
    /// The filled region, as the part up to the end of the buffer and
    /// the wrapped part.
    pub fn filled(&self) -> (&[u8], &[u8]) {
        let len = self.len();
        let r = self.read & (N - 1);
        if r + len <= N {
            (&self.buf[r..r + len], &[])
        } else {
            (&self.buf[r..], &self.buf[..r + len - N])
        }
    }
    /// Frees the first `n` filled octets.
    ///
    /// # Panics
    ///
    /// When `n` exceeds `len()`.
    pub fn consume(&mut self, n: usize) {
        assert!(n <= self.len(), "consume beyond the filled region");
        self.read = self.read.wrapping_add(n);
    }
    /// Empties the ring.
    pub fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
    }
}

impl<const N: usize> Default for InputRing<N> {
    fn default() -> Self {
        Self::new()
    }
}
