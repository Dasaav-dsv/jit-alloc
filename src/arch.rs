mod miri;
pub mod protect;
pub mod trap;
mod unix;
mod windows;

use crate::error::OsResult;

#[cfg(all(unix, not(miri)))]
use crate::arch::unix::{BackingFdProvider, granularity, map_rx_rw, unmap_rw_rx};

#[cfg(all(windows, not(miri)))]
use crate::arch::windows::{granularity, map_rx_rw, unmap_rw_rx};

#[cfg(miri)]
use crate::arch::miri::{granularity, map_rx_rw, unmap_rw_rx};

#[derive(Debug)]
pub struct MemmapSource {
    #[cfg(all(unix, not(miri)))]
    inner: BackingFdProvider,

    #[cfg(feature = "std")]
    granularity: std::sync::LazyLock<usize>,

    #[cfg(not(feature = "std"))]
    granularity: core::cell::LazyCell<usize>,
}

impl MemmapSource {
    pub const fn new() -> Self {
        Self {
            #[cfg(all(unix, not(miri)))]
            inner: BackingFdProvider::new(),

            #[cfg(feature = "std")]
            granularity: std::sync::LazyLock::new(granularity),

            #[cfg(not(feature = "std"))]
            granularity: core::cell::LazyCell::new(granularity),
        }
    }

    #[cfg(all(unix, not(miri)))]
    pub unsafe fn new_with_fd(fd: core::ffi::c_int) -> OsResult<Self> {
        let inner = unsafe { BackingFdProvider::new_with_fd(fd)? };
        Ok(Self {
            inner,
            ..Self::new()
        })
    }

    pub fn map_rx_rw(&mut self, size: usize) -> OsResult<Option<(*const u8, *mut u8)>> {
        assert!(size.is_multiple_of(self.granularity()));
        #[cfg(all(unix, not(miri)))]
        unsafe {
            map_rx_rw(size, &mut self.inner)
        }
        #[cfg(any(windows, miri))]
        unsafe {
            map_rx_rw(size)
        }
    }

    pub unsafe fn unmap_rw_rx(
        &mut self,
        rx_ptr: *const u8,
        rw_ptr: *mut u8,
        size: usize,
    ) -> OsResult<()> {
        assert!(size.is_multiple_of(self.granularity()));
        unsafe { unmap_rw_rx(rx_ptr, rw_ptr, size) }
    }

    pub fn granularity(&self) -> usize {
        *self.granularity
    }
}

impl Default for MemmapSource {
    fn default() -> Self {
        const { Self::new() }
    }
}
