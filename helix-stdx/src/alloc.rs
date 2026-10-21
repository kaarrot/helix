//! A global allocator that can zero memory as it is freed.
//!
//! Decrypted files are copied all over the editor: the document's rope, its
//! undo history, the syntax tree, render caches, save buffers. Freed memory
//! normally keeps its bytes until something reuses it, so wiping only the
//! buffers we own would leave most of those copies behind. Once [`enable`] is
//! called, every block is zeroed before it goes back to the system.
//!
//! Wiping costs a write over every freed byte, so it stays off until the
//! first encrypted file is opened and then stays on for the session: copies
//! of the plaintext may be freed long after the document closes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};

use zeroize::Zeroize;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Starts zeroing every freed block, for the rest of the process.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Overwrites `len` bytes at `ptr` with zeros in a way the compiler can't
/// drop as a dead store before the block is freed.
///
/// # Safety
///
/// `ptr` must be valid for writes of `len` bytes.
unsafe fn wipe(ptr: *mut u8, len: usize) {
    std::slice::from_raw_parts_mut(ptr, len).zeroize();
}

/// [`System`], zeroing every block it frees once [`enable`] was called.
pub struct WipeOnFree;

unsafe impl GlobalAlloc for WipeOnFree {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if is_enabled() {
            wipe(ptr, layout.size());
        }
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if !is_enabled() {
            return System.realloc(ptr, layout, new_size);
        }
        // `System.realloc` may move the block and free the old one without
        // us seeing it, so move it ourselves.
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = System.alloc(new_layout);
        if !new_ptr.is_null() {
            std::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_zeroes_the_whole_block() {
        let mut block = vec![0xAAu8; 4096];
        unsafe { wipe(block.as_mut_ptr(), block.len()) };
        assert!(block.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn realloc_keeps_contents_while_enabled() {
        enable();
        let layout = Layout::from_size_align(16, 8).unwrap();
        unsafe {
            let ptr = WipeOnFree.alloc(layout);
            assert!(!ptr.is_null());
            for i in 0..16 {
                *ptr.add(i) = i as u8;
            }
            let grown = WipeOnFree.realloc(ptr, layout, 64);
            assert!(!grown.is_null());
            for i in 0..16 {
                assert_eq!(*grown.add(i), i as u8);
            }
            WipeOnFree.dealloc(grown, Layout::from_size_align(64, 8).unwrap());
        }
    }
}
