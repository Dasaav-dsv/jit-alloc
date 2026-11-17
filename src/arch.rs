pub mod protect;
pub mod trap;
mod unix;
mod windows;

use core::num::NonZero;

use crate::error::OsResult;

#[cfg(unix)]
pub use crate::arch::unix::last_os_error;
#[cfg(unix)]
use crate::arch::unix::{BackingFdProvider, granularity, map_rx_rw, unmap_rw, unmap_rw_rx};
#[cfg(windows)]
pub use crate::arch::windows::last_os_error;
#[cfg(windows)]
use crate::arch::windows::{granularity, map_rx_rw, unmap_rw, unmap_rw_rx};

#[derive(Debug)]
pub struct MemmapSource {
    #[cfg(unix)]
    inner: BackingFdProvider,
    granularity: Option<NonZero<usize>>,
}

impl MemmapSource {
    pub const fn new() -> Self {
        Self {
            #[cfg(unix)]
            inner: BackingFdProvider::new(),
            granularity: None,
        }
    }

    pub fn map_rx_rw(&mut self, size: usize) -> OsResult<Option<(*const u8, *mut u8)>> {
        assert!(size.is_multiple_of(self.granularity()));
        #[cfg(unix)]
        unsafe {
            map_rx_rw(size, &mut self.inner)
        }
        #[cfg(windows)]
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

    pub unsafe fn unmap_rw(
        &mut self,
        rx_ptr: *const u8,
        rw_ptr: *mut u8,
        size: usize,
    ) -> OsResult<()> {
        assert!(size.is_multiple_of(self.granularity()));
        unsafe { unmap_rw(rx_ptr, rw_ptr, size) }
    }

    pub fn granularity(&mut self) -> usize {
        self.granularity
            .get_or_insert_with(|| {
                NonZero::new(granularity()).expect("system allocation granularity should not be 0")
            })
            .get()
    }
}

impl Default for MemmapSource {
    fn default() -> Self {
        const { Self::new() }
    }
}
