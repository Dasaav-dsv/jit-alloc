#![allow(unused, nonstandard_style, clippy::upper_case_acronyms)]

use core::ffi::{c_char, c_int, c_long, c_uint, c_void};

pub const PAGE_SIZE: c_uint = 4096;

#[repr(C)]
pub struct _timespec {
    pub tv_sec: time_t,
    pub tv_nsec: c_long,
}

macro_rules! os_types_impl {
    ($(
        $(#[cfg($($attr:tt)+)])?
        $vis:vis type $a:ident = $t:ty;
    )+) => {
        $(
            #[cfg(all(unix, not(miri), $($($attr)*)?))]
            $vis use libc::$a;
            #[cfg(any(not(unix), miri, $(not($($attr)*))?))]
            $vis type $a = $t;
        )+
    };
}

macro_rules! os_consts_impl {
    ($(
        $(#[cfg($($attr:tt)+)])?
        $vis:vis const $c:ident: $t:ty = $body:expr;
    )+) => {
        $(
            #[cfg(all(unix, not(miri), $($($attr)*)?))]
            $vis use libc::$c;
            #[cfg(any(not(unix), miri, $(not($($attr)*))?))]
            $vis const $c: $t = $body;
        )+
    };
}

macro_rules! os_mock_impl {
    ($(
        $(#[cfg($($attr:tt)+)])?
        $vis:vis unsafe fn $f:ident($($p:ident: $t:ty),*$(,)?) $(-> $r:ty)? { $($body:tt)* }
    )+) => {
        #[cfg(all(unix, not(miri)))]
        mod sys {
            mod inner {
                #[cfg(all(target_os = "android", feature = "asharedmem"))]
                pub use ndk_sys::ASharedMemory_create;
                pub use libc::*;
            }
            $($(#[cfg($($attr)+)])? pub use inner::$f;)+
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
                #[cfg(all(unix, not(miri)))]
                const SYS: Self = Self {
                    #[cfg(any(not(target_os = "android"), not(feature = "asharedmem")))]
                    ASharedMemory_create: |_, _| ::std::unimplemented!(),
                    $(
                        $(#[cfg($($attr)+)])?
                        $f: |$($p),*| unsafe { $crate::arch::unix::ffi::sys::$f($($p),*) },
                    )+
                };

                #[cfg(any(miri, not(unix)))]
                const SYS: Self = Self {
                    $($f: |$($p),*| ::std::unimplemented!(),)+
                };

                pub const fn new(default: Self) -> Self {
                    cfg_select! {
                        all(unix, not(miri)) => { Self::SYS },
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

        #[cfg(all(unix, not(test), not(miri)))]
        pub use sys::*;

        #[cfg(test)]
        pub use mock::*;
    };
}

os_types_impl! {
    pub type FILE = c_void;
    pub type clockid_t = c_int;
    pub type mode_t = u32;
    pub type off_t = i64;
    pub type pid_t = i32;
    pub type size_t = usize;
    pub type time_t = i64;
    pub type timespec = _timespec;
}

os_consts_impl! {
    pub const _SC_PAGESIZE: c_int = 30;

    pub const CLOCK_MONOTONIC: clockid_t = 1;
    pub const CLOCK_REALTIME: clockid_t = 0;

    pub const MAP_ANONYMOUS: c_int = 0x0020;
    pub const MAP_FAILED: *mut c_void = !0 as *mut c_void;
    #[cfg(target_vendor = "apple")]
    pub const MAP_JIT: c_int = 0x0800;
    pub const MAP_PRIVATE: c_int = 0x0002;
    pub const MAP_SHARED: c_int = 0x0001;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const MFD_CLOEXEC: c_uint = 0x0001;

    #[cfg(target_os = "linux")]
    pub const O_CLOEXEC: c_int = 0o2000000;
    pub const O_CREAT: c_int = 64;
    pub const O_EXCL: c_int = 128;
    pub const O_RDWR: c_int = 2;
    #[cfg(target_os = "linux")]
    pub const O_TMPFILE: c_int = 0o20200000;

    pub const PROT_EXEC: c_int = 4;
    pub const PROT_READ: c_int = 1;
    pub const PROT_WRITE: c_int = 2;

    pub const S_IRUSR: mode_t = 0o0400;
    pub const S_IWUSR: mode_t = 0o0200;

    #[cfg(target_os = "freebsd")]
    pub const SHM_ANON: *mut c_char = 1 as *mut c_char;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const SYS_memfd_create: c_long = 319;
}

os_mock_impl! {
    #[cfg(all(target_os = "android", feature = "asharedmem"))]
    pub unsafe fn ASharedMemory_create(name: *const c_char, size: size_t) -> c_int {
        assert_ne!(size, 0);
    }

    pub unsafe fn clock_gettime(clk_id: clockid_t, tp: *mut timespec) -> c_int {
        assert!(clk_id == CLOCK_REALTIME || clk_id == CLOCK_MONOTONIC);
        assert!(!tp.is_null());
    }

    pub unsafe fn close(fd: c_int) -> c_int {
        assert!(fd >= 0);
    }

    pub unsafe fn dup(fd: c_int) -> c_int {
        assert!(fd >= 0);
    }

    pub unsafe fn fclose(file: *mut FILE) -> c_int {
        assert!(!file.is_null());
    }

    pub unsafe fn fileno(stream: *mut FILE) -> c_int {
        assert!(!stream.is_null());
    }

    pub unsafe fn ftruncate(fd: c_int, length: off_t) -> c_int {
        assert!(fd >= 0);
    }

    pub unsafe fn getenv(s: *const c_char) -> *mut c_char {
        assert!(!s.is_null());
    }

    pub unsafe fn getpid() -> pid_t {}

    pub unsafe fn mmap(
        addr: *mut c_void,
        len: size_t,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: off_t,
    ) -> *mut c_void {
        assert_eq!(addr.align_offset(PAGE_SIZE as usize), 0);
        assert_ne!(len, 0);
        assert!(len.is_multiple_of(PAGE_SIZE as size_t));
        #[cfg(target_vendor = "apple")]
        assert_ne!(flags & MAP_JIT, 0);
        assert!((flags & MAP_SHARED != 0) ^ (flags & MAP_PRIVATE != 0));
        assert!(fd >= 0 || (fd == -1 && flags & MAP_ANONYMOUS != 0));
        assert!((offset as u64).is_multiple_of(PAGE_SIZE as u64));
    }

    pub unsafe fn munmap(addr: *mut c_void, len: size_t) -> c_int {
        assert!(!addr.is_null());
        assert_eq!(addr.align_offset(PAGE_SIZE as usize), 0);
        assert_ne!(len, 0);
    }

    pub unsafe fn open(path: *const c_char, oflag: c_int, mode: mode_t) -> c_int {
        assert!(!path.is_null());
    }

    pub unsafe fn shm_open(name: *const c_char, oflag: c_int, mode: mode_t) -> c_int {
        assert!(!name.is_null());
    }

    pub unsafe fn shm_unlink(name: *const c_char) -> c_int {
        assert!(!name.is_null());
    }

    pub unsafe fn syscall(num: c_long, name: *const c_char, flags: c_uint) -> c_long {
        assert_eq!(num, SYS_memfd_create);
        assert!(!name.is_null());
    }

    pub unsafe fn sysconf(name: c_int) -> c_long {
        assert_eq!(name, _SC_PAGESIZE);
    }

    pub unsafe fn tmpfile() -> *mut FILE {}

    pub unsafe fn unlink(c: *const c_char) -> c_int {
        assert!(!c.is_null());
    }
}

#[cfg(all(unix, not(test), not(miri)))]
pub use sys::*;

#[cfg(test)]
pub use mock::*;
