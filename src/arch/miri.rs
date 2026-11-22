#![cfg(miri)]

use core::{alloc::Layout, num::NonZero};

use crate::error::OsResult;

const GRANULARITY: usize = 4096;

/// Returns the minimum virtual memory allocation granularity.
pub fn granularity() -> usize {
    GRANULARITY
}

/// Allocates a read-execute and a read-write mapping pointing to the same memory.
///
/// The mappings may overlap and `pthread_jit_write_protect_np` must be called before
/// writing to the mapping on macOS.
///
/// # Safety
///
/// `size` must be a multiple of the value returned by [`granularity`].
pub unsafe fn map_rx_rw(size: usize) -> OsResult<Option<(*const u8, *mut u8)>> {
    unsafe {
        let layout = Layout::from_size_align_unchecked(size, GRANULARITY);
        let ptr = alloc::alloc::alloc(layout);
        Ok(Some((ptr, ptr)))
    }
}

/// Releases a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw_rx(rx_ptr: *const u8, rw_ptr: *mut u8, size: usize) -> OsResult<()> {
    unsafe { unmap_rw(rx_ptr, rw_ptr, size) }
}

/// Releases the read-write portion of a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw(_rx_ptr: *const u8, rw_ptr: *mut u8, size: usize) -> OsResult<()> {
    unsafe {
        let layout = Layout::from_size_align_unchecked(size, GRANULARITY);
        alloc::alloc::dealloc(rw_ptr, layout);
        Ok(())
    }
}
