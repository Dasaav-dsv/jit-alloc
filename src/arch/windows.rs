#![cfg(all(any(test, windows), not(miri)))]
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

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, ptr, thread_local};

    use hashbrown::HashSet;
    use rustc_hash::FxBuildHasher;

    use crate::arch::windows::{
        ffi::{HANDLE, Os},
        granularity, map_rx_rw, unmap_rw_rx,
    };

    #[test]
    fn test_granularity() {
        OsState::with(|| {
            assert!(granularity().is_power_of_two());
        });
    }

    #[test]
    fn test_map_rx_rw() {
        OsState::with(|| {
            let granularity = granularity();
            let (rx, rw) = unsafe { map_rx_rw(granularity).unwrap().unwrap() };

            assert_eq!(rx.align_offset(granularity), 0);
            assert_eq!(rw.align_offset(granularity), 0);

            unsafe {
                unmap_rw_rx(rx, rw, granularity).unwrap();
            }
        });
    }

    #[test]
    fn test_map_rx_rw_create_fail() {
        OsState::with(|| {
            Os::with_mut(|os| os.CreateFileMappingW = |_, _, _, _, _, _| ptr::null_mut());

            unsafe {
                let granularity = granularity();
                map_rx_rw(granularity).unwrap_err();
            }
        });
    }

    #[test]
    fn test_map_rx_rw_map_fail() {
        OsState::with(|| {
            Os::with_mut(|os| os.MapViewOfFile = |_, _, _, _, _| ptr::null_mut());

            unsafe {
                let granularity = granularity();
                map_rx_rw(granularity).unwrap_err();
            }
        });
    }

    #[test]
    fn test_map_rx_rw_map2_fail() {
        OsState::with(|| {
            Os::with_mut(|os| {
                os.MapViewOfFile = |_1, _2, _3, _4, _5| {
                    let prev = Os::with_mut(|os| {
                        let prev = os.MapViewOfFile;
                        os.MapViewOfFile = |_, _, _, _, _| ptr::null_mut();
                        prev
                    });
                    prev(_1, _2, _3, _4, _5)
                };
            });

            unsafe {
                let granularity = granularity();
                map_rx_rw(granularity).unwrap_err();
            }
        });
    }

    #[test]
    fn test_map_rx_rw_unmap_fail() {
        OsState::with(|| {
            Os::with_mut(|os| os.UnmapViewOfFile = |_| 0);

            let granularity = granularity();
            let (rx, rw) = unsafe { map_rx_rw(granularity).unwrap().unwrap() };

            assert_eq!(rx.align_offset(granularity), 0);
            assert_eq!(rw.align_offset(granularity), 0);

            unsafe {
                unmap_rw_rx(rx, rw, granularity).unwrap_err();
            }
        });
    }

    struct OsState {
        next_handle: usize,
        handles: HashSet<usize, FxBuildHasher>,
        next_map: usize,
        maps: HashSet<usize, FxBuildHasher>,
    }

    thread_local! {
        static OS_STATE: RefCell<OsState> = const {
            RefCell::new(OsState {
                next_handle: 1,
                handles: HashSet::with_hasher(FxBuildHasher),
                next_map: 1,
                maps: HashSet::with_hasher(FxBuildHasher),
            })
        };
    }

    impl OsState {
        fn with(f: impl FnOnce()) {
            Os::reset();

            OS_STATE.with_borrow_mut(|state| {
                state.next_handle = 1;
                state.handles.clear();
                state.next_map = 1;
                state.maps.clear();
            });

            f();

            OS_STATE.with_borrow(|state| {
                assert_eq!(state.handles.len(), 0);
                assert_eq!(state.maps.len(), 0);
            });
        }

        fn open_handle(&mut self) -> HANDLE {
            let handle_value = self.next_handle;
            self.next_handle += 1;
            self.handles.insert(handle_value);
            ptr::without_provenance_mut(handle_value)
        }

        #[track_caller]
        fn close_handle(&mut self, handle: HANDLE) {
            assert!(self.handles.remove(&handle.addr()), "handle already closed");
        }
    }

    impl Os {
        fn reset() {
            Os::with_mut(|os| *os = Default::default());
        }
    }

    impl Default for Os {
        fn default() -> Self {
            Os {
                granularity: 65536,
                GetLastError: || 1,
                GetSystemInfo: |sys_info| unsafe {
                    (*sys_info).dwAllocationGranularity = Os::with(|os| os.granularity);
                },
                CreateFileMappingW: |_, _, _, _, _, _| {
                    OS_STATE.with_borrow_mut(OsState::open_handle)
                },
                MapViewOfFile: |_, _, _, _, _| {
                    let granularity = Os::with(|os| os.granularity) as usize;
                    ptr::without_provenance_mut(granularity)
                },
                UnmapViewOfFile: |_| 1,
                CloseHandle: |handle| {
                    OS_STATE.with_borrow_mut(|os| os.close_handle(handle));
                    1
                },
            }
        }
    }
}
