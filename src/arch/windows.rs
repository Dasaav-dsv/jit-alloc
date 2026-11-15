#![cfg(windows)]
#![allow(clippy::unnecessary_cast, clippy::useless_conversion)]

use core::{mem, ptr};

use crate::arch::windows::ffi::{
    CloseHandle, CreateFileMappingW, DWORD, FILE_MAP_EXECUTE, FILE_MAP_READ, FILE_MAP_WRITE,
    GetLastError, GetSystemInfo, INVALID_HANDLE_VALUE, LPCVOID, MapViewOfFile,
    PAGE_EXECUTE_READWRITE, UnmapViewOfFile,
};

mod ffi;

/// Returns the minimum virtual memory allocation granularity.
pub fn granularity() -> usize {
    let sys_info = unsafe {
        let mut info = mem::zeroed();
        GetSystemInfo(&mut info);
        info
    };

    sys_info.dwAllocationGranularity as usize
}

/// Allocates a read-execute and a read-write mapping pointing to the same memory.
///
/// The mappings may overlap and `pthread_jit_write_protect_np` must be called before
/// writing to the mapping on macOS.
///
/// # Safety
///
/// `size` must be a multiple of the value returned by [`granularity`].
pub unsafe fn map_rx_rw(size: usize) -> Result<(*const u8, *mut u8), u32> {
    unsafe {
        debug_assert!(size.is_multiple_of(granularity()));

        let mapping = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null_mut(),
            PAGE_EXECUTE_READWRITE,
            ((size as u64) >> 32) as DWORD,
            size as DWORD,
            ptr::null_mut(),
        );

        if mapping.is_null() {
            return Err(GetLastError() as u32);
        }

        // Setting `dwNumberOfBytesToMap` to 0 maps the entire file.
        let rx = MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_EXECUTE, 0, 0, 0);

        if rx.is_null() {
            let _ = CloseHandle(mapping);
            return Err(GetLastError() as u32);
        }

        let rw = MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0);

        // The mapping handle is closed regardless.
        let _ = CloseHandle(mapping);

        if rw.is_null() {
            let _ = UnmapViewOfFile(rx);
            return Err(GetLastError() as u32);
        }

        Ok((rx as *const u8, rw as *mut u8))
    }
}

/// Releases a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw_rx(rx_ptr: *const u8, rw_ptr: *mut u8, _size: usize) -> Result<(), u32> {
    unsafe {
        let res = UnmapViewOfFile(rx_ptr as LPCVOID);
        let err = GetLastError() as u32;

        if rx_ptr != rw_ptr {
            let _ = UnmapViewOfFile(rw_ptr as LPCVOID);
        }

        if res != 0 { Ok(()) } else { Err(err) }
    }
}
