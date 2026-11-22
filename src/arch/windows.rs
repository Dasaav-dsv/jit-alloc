#![cfg(all(windows, not(miri)))]
#![allow(clippy::unnecessary_cast, clippy::useless_conversion)]

use core::{mem, num::NonZero};

use crate::{
    arch::windows::ffi::{
        CloseHandle, CreateFileMappingW, DWORD, FILE_MAP_EXECUTE, FILE_MAP_READ, FILE_MAP_WRITE,
        GetLastError, GetSystemInfo, INVALID_HANDLE_VALUE, LPCVOID, LPCWSTR, LPSECURITY_ATTRIBUTES,
        MapViewOfFile, PAGE_EXECUTE_READWRITE, UnmapViewOfFile,
    },
    error::{OsResult, OsResultExt, os::err_if::os_err_if},
};

mod ffi;

/// Returns the last error set by an OS function.
pub fn last_os_error() -> Option<NonZero<i32>> {
    unsafe { NonZero::new(GetLastError() as i32) }
}

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
pub unsafe fn map_rx_rw(size: usize) -> OsResult<Option<(*const u8, *mut u8)>> {
    unsafe {
        let mapping = os_err_if!(
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                LPSECURITY_ATTRIBUTES::default(),
                PAGE_EXECUTE_READWRITE,
                ((size as u64) >> 32) as DWORD,
                size as DWORD,
                LPCWSTR::default(),
            )
            .is_null()
        )?;

        // Setting `dwNumberOfBytesToMap` to 0 maps the entire file.
        let rx =
            os_err_if!(MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_EXECUTE, 0, 0, 0).is_null())
                .chain_err_with(|| os_err_if!(CloseHandle(mapping) == 0))?;

        let rw =
            os_err_if!(MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0).is_null())
                .chain_err_with(|| os_err_if!(UnmapViewOfFile(rx) == 0))
                .chain(
                    // The mapping handle is closed regardless.
                    os_err_if!(CloseHandle(mapping) == 0),
                )?;

        Ok(Some((rx as *const u8, rw as *mut u8)))
    }
}

/// Releases a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw_rx(rx_ptr: *const u8, rw_ptr: *mut u8, _size: usize) -> OsResult<()> {
    unsafe {
        let res = os_err_if!(UnmapViewOfFile(rx_ptr as LPCVOID) == 0);
        res.chain(unmap_rw(rx_ptr, rw_ptr, _size))?;
        Ok(())
    }
}

/// Releases the read-write portion of a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw(rx_ptr: *const u8, rw_ptr: *mut u8, _size: usize) -> OsResult<()> {
    unsafe {
        if rx_ptr != rw_ptr {
            os_err_if!(UnmapViewOfFile(rw_ptr as LPCVOID) == 0)?;
        }
        Ok(())
    }
}
