#![allow(unused, nonstandard_style, clippy::upper_case_acronyms)]

use core::ffi::c_void;

pub type BOOL = i32;
pub type WCHAR = u16;
pub type WORD = u16;
pub type DWORD = u32;
pub type SIZE_T = usize;
pub type DWORD_PTR = usize;

pub type LPVOID = *mut c_void;
pub type LPCVOID = *const c_void;
pub type LPCWSTR = *const WCHAR;

pub type HANDLE = *mut c_void;

pub const ALLOCATION_GRANULARITY: DWORD = 65536;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

pub const PAGE_EXECUTE_READWRITE: DWORD = 0x00000040;

pub const FILE_MAP_READ: DWORD = 0x00000004;
pub const FILE_MAP_WRITE: DWORD = 0x00000002;
pub const FILE_MAP_EXECUTE: DWORD = 0x00000020;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SYSTEM_INFO {
    pub wProcessorArchitecture: WORD,
    pub wReserved: WORD,
    pub dwPageSize: DWORD,
    pub lpMinimumApplicationAddress: LPVOID,
    pub lpMaximumApplicationAddress: LPVOID,
    pub dwActiveProcessorMask: DWORD_PTR,
    pub dwNumberOfProcessors: DWORD,
    pub dwProcessorType: DWORD,
    pub dwAllocationGranularity: DWORD,
    pub wProcessorLevel: WORD,
    pub wProcessorRevision: WORD,
}

pub type LPSYSTEM_INFO = *mut SYSTEM_INFO;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SECURITY_ATTRIBUTES {
    pub nLength: DWORD,
    pub lpSecurityDescriptor: LPVOID,
    pub bInheritHandle: BOOL,
}

pub type LPSECURITY_ATTRIBUTES = *mut SECURITY_ATTRIBUTES;

macro_rules! os_mock_impl {
    ($(#[$($attr:tt)+])* unsafe extern $cc:literal {
        $($vis:vis unsafe fn $f:ident($($p:ident: $t:ty),*$(,)?) $(-> $r:ty)? { $($body:tt)* })+
    }) => {
        #[cfg(all(windows, not(miri)))]
        mod sys {
            use super::*;

            $(#[$($attr)+])*
            unsafe extern $cc {
                $(
                    $vis unsafe fn $f($($p: $t),*) $(-> $r)?;
                )+
            }
        }

        #[cfg(test)]
        mod mock {
            use super::*;

            pub struct Os {
                $(pub $f: unsafe fn($($t),*) $(-> $r)?,)+
            }

            ::std::thread_local! {
                static OS: ::std::cell::RefCell<Os> = const {
                    ::std::cell::RefCell::new(Os::SYS)
                };
            }

            impl Os {
                #[cfg(all(windows, not(miri)))]
                const SYS: Self = Self {
                    $($f: |$($p),*| unsafe { $crate::arch::windows::ffi::sys::$f($($p),*) },)+
                };

                #[cfg(any(miri, not(windows)))]
                const SYS: Self = Self {
                    $($f: |$($p),*| ::std::unimplemented!(),)+
                };

                pub const fn new(default: Self) -> Self {
                    cfg_select! {
                        all(windows, not(miri)) => { Self::SYS },
                        _ => default,
                    }
                }

                pub fn with<T>(f: impl FnOnce(&Self) -> T) -> T {
                    OS.with_borrow(f)
                }

                pub fn with_mut<T>(f: impl FnOnce(&mut Self) -> T) -> T {
                    OS.with_borrow_mut(f)
                }
            }

            $(
                #[track_caller]
                $vis unsafe fn $f($($p: $t),*) $(-> $r)? {
                    let _ = { $($body)* };
                    unsafe { Os::with(|os| os.$f)($($p,)*) }
                }
            )+
        }

        #[cfg(all(windows, not(test), not(miri)))]
        pub use sys::*;

        #[cfg(test)]
        pub use mock::*;
    };
}

os_mock_impl! {
    #[cfg_attr(target_arch = "x86_64", link(name = "Kernel32", kind = "raw-dylib"))]
    #[cfg_attr(
        target_arch = "x86",
        link(
            name = "Kernel32",
            kind = "raw-dylib",
            import_name_type = "undecorated"
        )
    )]
    unsafe extern "system" {
        pub unsafe fn GetLastError() -> DWORD {}

        pub unsafe fn GetSystemInfo(lpSystemInfo: LPSYSTEM_INFO) {
            assert!(!lpSystemInfo.is_null());
        }

        pub unsafe fn CreateFileMappingW(
            hFile: HANDLE,
            lpFileMappingAttributes: LPSECURITY_ATTRIBUTES,
            flProtect: DWORD,
            dwMaximumSizeHigh: DWORD,
            dwMaximumSizeLow: DWORD,
            lpName: LPCWSTR,
        ) -> HANDLE {
            assert!(!hFile.is_null());
            assert_eq!(flProtect, PAGE_EXECUTE_READWRITE);
            assert!(!hFile.is_null() || dwMaximumSizeHigh != 0 || dwMaximumSizeLow != 0);
        }

        pub unsafe fn MapViewOfFile(
            hFileMappingObject: HANDLE,
            dwDesiredAccess: DWORD,
            dwFileOffsetHigh: DWORD,
            dwFileOffsetLow: DWORD,
            dwNumberOfBytesToMap: SIZE_T,
        ) -> LPVOID {
            assert!(!hFileMappingObject.is_null());
            assert_eq!(dwDesiredAccess & !(FILE_MAP_READ | FILE_MAP_WRITE | FILE_MAP_EXECUTE), 0);
            assert!(dwNumberOfBytesToMap.is_multiple_of(ALLOCATION_GRANULARITY as SIZE_T));
        }

        pub unsafe fn UnmapViewOfFile(lpBaseAddress: LPCVOID) -> BOOL {
            assert!(!lpBaseAddress.is_null());
        }

        pub unsafe fn CloseHandle(hObject: HANDLE) -> BOOL {
            assert!(!hObject.is_null());
        }
    }
}
