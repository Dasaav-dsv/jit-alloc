#![cfg(any(test, all(unix, not(miri))))]
#![allow(
    irrefutable_let_patterns,
    clippy::unnecessary_cast,
    clippy::useless_conversion
)]

use alloc::{borrow::ToOwned, format};
use core::{
    ffi::{CStr, c_char, c_int, c_uint, c_void},
    hash::{Hash, Hasher},
    hint,
    num::NonZero,
    ptr,
};

use crate::{
    arch::unix::{
        errno::__errno_location,
        ffi::{
            _SC_PAGESIZE, CLOCK_MONOTONIC, CLOCK_REALTIME, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE,
            MAP_SHARED, MFD_CLOEXEC, O_CLOEXEC, O_CREAT, O_EXCL, O_RDWR, O_TMPFILE, PROT_EXEC,
            PROT_READ, PROT_WRITE, S_IRUSR, S_IWUSR, SYS_memfd_create, clock_gettime, close, dup,
            fclose, fileno, ftruncate, getenv, getpid, mmap, mode_t, munmap, off_t, open, shm_open,
            shm_unlink, size_t, syscall, sysconf, tmpfile, unlink,
        },
    },
    error::{OsResult, OsResultExt, os::err_if::os_err_if},
};

mod errno;
mod ffi;

#[allow(unused)]
const PROT_RX: c_int = PROT_READ | PROT_EXEC;
#[allow(unused)]
const PROT_RW: c_int = PROT_READ | PROT_WRITE;
#[allow(unused)]
const PROT_RWX: c_int = PROT_READ | PROT_WRITE | PROT_EXEC;

/// Returns the last error set by an OS function.
pub fn last_os_error() -> Option<NonZero<i32>> {
    let errno = unsafe { *__errno_location().as_ref()? as i32 };
    NonZero::new(errno)
}

/// Returns the minimum virtual memory allocation granularity.
pub fn granularity() -> usize {
    unsafe { sysconf(_SC_PAGESIZE) as usize }
}

/// Allocates a read-execute and a read-write mapping pointing to the same memory.
///
/// The mappings may overlap and `pthread_jit_write_protect_np` must be called before
/// writing to the mapping on macOS.
///
/// # Safety
///
/// `size` must be a multiple of the value returned by [`granularity`].
pub unsafe fn map_rx_rw(
    size: usize,
    fd_provider: &mut BackingFdProvider,
) -> OsResult<Option<(*const u8, *mut u8)>> {
    unsafe { fd_provider.mmap_rx_rw(size, 0) }
}

/// Releases a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw_rx(rx_ptr: *const u8, rw_ptr: *mut u8, size: usize) -> OsResult<()> {
    unsafe {
        let res = os_err_if!(munmap(rx_ptr as *mut c_void, size as size_t) == -1);
        res.chain(unmap_rw(rx_ptr, rw_ptr, size))?;
        Ok(())
    }
}

/// Releases the read-write portion of a read-execute and a read-write mapping pair.
///
/// # Safety
///
/// `rx_ptr` and `rw_ptr` must be both returned by a call to [`map_rx_rw`] with the same `size`.
pub unsafe fn unmap_rw(rx_ptr: *const u8, rw_ptr: *mut u8, size: usize) -> OsResult<()> {
    unsafe {
        if rx_ptr != rw_ptr {
            os_err_if!(munmap(rw_ptr as *mut c_void, size as size_t) == -1)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct BackingFdProvider {
    last_fd: Option<BackingFd>,
    last_strategy: usize,
    strategies: &'static [OpenStrategy],
    random_state: u64,
}

#[derive(Debug)]
enum BackingFd {
    Resizable { fd: c_int, offset: usize },
    FixedSize { fd: c_int },
    Anonymous,
}

type OpenStrategy = fn(&mut BackingFdProvider, size: usize) -> OsResult<BackingFd>;

impl BackingFdProvider {
    /// Returns a provider with multiple platform and feature flag dependent strategies
    /// for opening a fd that can be mapped as executable.
    ///
    /// The first one to succeed is memoized, failed strategies are discarded.
    pub(super) const fn new() -> Self {
        /// Fixed size fd (memfd, shm, Android shared memory).
        fn fixed(fd: c_int) -> BackingFd {
            BackingFd::FixedSize { fd }
        }

        /// Resizable fd (file backed).
        fn resizable(fd: c_int) -> BackingFd {
            BackingFd::Resizable { fd, offset: 0 }
        }

        // 1. `memfd_create` (Linux and Android only)
        // 2. `ASharedMemory_create` (Android only)
        // 3. `shm_open` with `SHM_ANON` (FreeBSD only)
        // 4. anonymous RWX mapping
        //
        // Target feature "shm_open" only:
        // 5. `shm_open` with a randomly assigned name
        //
        // Target feature "tmp_file" only:
        // 6. `tmpfile` into `fileno`
        // 7. `open` with `O_TMPFILE` in the CWD (Linux only)
        // 8. `open` with `O_TMPFILE` in $TMPDIR or /tmp (Linux only)
        // 9. `open` in $TMPDIR or /tmp with a randomly assigned name
        #[cfg(all(unix, not(miri)))]
        let strategies: &'static [OpenStrategy] = &[
            #[cfg(any(target_os = "linux", target_os = "android"))]
            |_, size| BackingFd::try_map_resize(size, memfd_create()?, fixed),
            #[cfg(all(target_os = "android", feature = "asharedmem"))]
            |_, size| BackingFd::try_map(size, asharedmem_create(size)?, fixed),
            #[cfg(all(target_os = "freebsd", feature = "shm_open"))]
            |_, size| BackingFd::try_map_resize(size, shm_open_anon()?, fixed),
            |_, size| BackingFd::try_map_anon(size),
            #[cfg(all(
                not(target_vendor = "apple"),
                not(target_os = "android"),
                feature = "shm_open",
            ))]
            |provider, size| {
                BackingFd::try_map_resize(size, shm_open_named(&mut provider.random_state)?, fixed)
            },
            #[cfg(feature = "tmp_file")]
            |_, size| BackingFd::try_map_resize(size, tmpfile_fileno()?, resizable),
            #[cfg(all(target_os = "linux", feature = "tmp_file"))]
            |_, size| BackingFd::try_map_resize(size, open_o_tmpfile_in(c"./")?, resizable),
            #[cfg(all(target_os = "linux", feature = "tmp_file"))]
            |_, size| BackingFd::try_map_resize(size, open_o_tmpfile_in(tmpdir())?, resizable),
            #[cfg(feature = "tmp_file")]
            |provider, size| {
                BackingFd::try_map_resize(
                    size,
                    open_in_tmpdir(&mut provider.random_state)?,
                    resizable,
                )
            },
        ];

        #[cfg(any(not(unix), miri))]
        let strategies: &'static [OpenStrategy] = &[
            |_, size| BackingFd::try_map_resize(size, memfd_create()?, fixed),
            #[cfg(feature = "asharedmem")]
            |_, size| BackingFd::try_map(size, asharedmem_create(size)?, fixed),
            #[cfg(feature = "shm_open")]
            |_, size| BackingFd::try_map_resize(size, shm_open_anon()?, fixed),
            |_, size| BackingFd::try_map_anon(size),
            #[cfg(feature = "shm_open")]
            |provider, size| {
                BackingFd::try_map_resize(size, shm_open_named(&mut provider.random_state)?, fixed)
            },
            #[cfg(feature = "tmp_file")]
            |_, size| BackingFd::try_map_resize(size, tmpfile_fileno()?, resizable),
            #[cfg(feature = "tmp_file")]
            |_, size| BackingFd::try_map_resize(size, open_o_tmpfile_in(c"./")?, resizable),
            #[cfg(feature = "tmp_file")]
            |_, size| BackingFd::try_map_resize(size, open_o_tmpfile_in(tmpdir())?, resizable),
            #[cfg(feature = "tmp_file")]
            |provider, size| {
                BackingFd::try_map_resize(
                    size,
                    open_in_tmpdir(&mut provider.random_state)?,
                    resizable,
                )
            },
        ];

        Self {
            last_fd: None,
            last_strategy: 0,
            strategies,
            random_state: 0x9e3779b97f4a7c13,
        }
    }

    /// Returns a provider that uses a user provided fd to map as executable memory.
    /// The ownership of the file descriptor is passed to this function. It must be opened
    /// with at least read and write permissions.
    ///
    /// # Safety
    ///
    /// Other code must not modify the underlying file or use this fd. All memory mapped file
    /// dangers and caveats apply.
    #[cfg(unix)]
    pub unsafe fn new_with_fd(fd: c_int) -> OsResult<Self> {
        let fd = unsafe { BackingFd::with_user_fd(fd)? };
        Ok(Self {
            last_fd: Some(fd),
            last_strategy: 0,
            strategies: &[],
            random_state: 0x9e3779b97f4a7c13,
        })
    }

    /// Allocates a read-execute and a read-write mapping pointing to the same memory.
    ///
    /// The mappings may overlap and `pthread_jit_write_protect_np` must be called before
    /// writing to the mapping on macOS.
    ///
    /// # Safety
    ///
    /// `size` must be a multiple of the value returned by [`granularity`].
    unsafe fn mmap_rx_rw(
        &mut self,
        size: usize,
        flags: c_int,
    ) -> OsResult<Option<(*const u8, *mut u8)>> {
        if self.last_fd.is_none() {
            // This is first time initialization or the last fd was discarded.
            // Need a new backing fd.
            self.last_fd = unsafe { self.new_fd(size)? };
        }

        // Take the fd so it is discarded on fail (or if it's fixed size).
        let Some(mut fd) = self.last_fd.take() else {
            return Ok(None);
        };

        let rx_rw = unsafe { fd.mmap_rx_rw(size, flags)? };

        // Fixed size fd are mapped fully and can't be reused.
        if !matches!(fd, BackingFd::FixedSize { .. }) {
            self.last_fd = Some(fd);
        }

        Ok(rx_rw)
    }

    /// Goes through available fd opening strategies in order attempting to open a fd
    /// that allows for executable memory.
    ///
    /// The first strategy to succeed is memoized, failed strategies are discarded.
    ///
    /// # Safety
    ///
    /// `size` must be a multiple of the value returned by [`granularity`].
    unsafe fn new_fd(&mut self, size: usize) -> OsResult<Option<BackingFd>> {
        let mut res = Ok(None);

        for strategy in &self.strategies[self.last_strategy..] {
            match strategy(self, size) {
                Ok(fd) => return Ok(Some(fd)),
                next_err => res = res.chain(next_err),
            }

            // Don't try this strategy again.
            self.last_strategy += 1;
        }

        res
    }
}

impl BackingFd {
    #[cfg(unix)]
    pub unsafe fn with_user_fd(fd: c_int) -> OsResult<Self> {
        let fd = unsafe { os_err_if!(dup(fd) == -1).chain(os_err_if!(close(fd) == -1))? };
        let size = granularity();
        BackingFd::try_map_resize(size, fd, |fd| BackingFd::Resizable { fd, offset: 0 })
    }

    /// Allocates a read-execute and a read-write mapping pointing to the same memory.
    ///
    /// The mappings may overlap and `pthread_jit_write_protect_np` must be called before
    /// writing to the mapping on macOS.
    ///
    /// # Safety
    ///
    /// `size` must be a multiple of the value returned by [`granularity`].
    unsafe fn mmap_rx_rw(
        &mut self,
        size: usize,
        flags: c_int,
    ) -> OsResult<Option<(*const u8, *mut u8)>> {
        /// Allocates a single RWX mapping (fallback or macOS specific).
        unsafe fn mmap_rwx(
            len: size_t,
            mut flags: c_int,
            fd: c_int,
            offset: off_t,
        ) -> OsResult<(*const u8, *mut u8)> {
            flags |= MAP_PRIVATE;

            if fd == -1 {
                flags |= MAP_ANONYMOUS;
            }

            #[cfg(any(test, target_vendor = "apple"))]
            {
                flags |= ffi::MAP_JIT;
            }

            let rwx = unsafe {
                os_err_if!(
                    mmap(ptr::null_mut::<c_void>(), len, PROT_RWX, flags, fd, offset) == MAP_FAILED
                )?
            };

            Ok((rwx as *const u8, rwx as *mut u8))
        }

        let Ok(len) = size.try_into() else {
            return Ok(None);
        };

        let (fd, offset) = match self {
            Self::Resizable { fd, offset } => {
                let Ok(cur_offset) = off_t::try_from(*offset) else {
                    return Ok(None);
                };

                let Some(new_offset) = offset.checked_add(size) else {
                    return Ok(None);
                };

                let Ok(new_size) = new_offset.try_into() else {
                    return Ok(None);
                };

                // Grow the file by `size` to map the newly allocated memory.
                unsafe {
                    os_err_if!(ftruncate(*fd, new_size) == -1)?;
                }

                *offset = new_offset;

                (*fd, cur_offset)
            }
            Self::FixedSize { fd } => (*fd, 0),
            Self::Anonymous => unsafe {
                return mmap_rwx(len, flags, -1, 0).map(Some);
            },
        };

        cfg_select! {
            // On macOS return a single mapping with the MAP_JIT flag.
            target_vendor = "apple" => mmap_rwx(len, flags, fd, offset).map(Some),
            // On other platforms return a double mapping.
            _ => {unsafe {
                let flags = flags | MAP_SHARED;

                let rx = os_err_if!(
                    mmap(ptr::null_mut::<c_void>(), len, PROT_RX, flags, fd, offset) == MAP_FAILED
                )?;

                let rw = os_err_if!(
                    mmap(ptr::null_mut::<c_void>(), len, PROT_RW, flags, fd, offset) == MAP_FAILED
                )
                .chain_err_with(|| os_err_if!(munmap(rx, len) == -1))?;

                Ok(Some((rx as *const u8, rw as *mut u8)))
            }}
        }
    }

    /// Tries mapping a fd as executable memory. A security policy might disallow this
    /// for example when /dev/shm or /tmp are mounted as noexec.
    fn try_map(size: size_t, fd: c_int, f: impl FnOnce(c_int) -> Self) -> OsResult<Self> {
        unsafe {
            let (prot, flags) = cfg_select! {
                any(test, target_vendor = "apple") => (PROT_RWX, MAP_PRIVATE | ffi::MAP_JIT),
                _ => (PROT_RX, MAP_SHARED),
            };

            let rx =
                os_err_if!(mmap(ptr::null_mut::<c_void>(), size, prot, flags, fd, 0) == MAP_FAILED)
                    .chain_err_with(|| os_err_if!(close(fd) == -1))?;

            os_err_if!(munmap(rx, size) == -1).chain_err_with(|| os_err_if!(close(fd) == -1))?;

            Ok(f(fd))
        }
    }

    /// Tries mapping a resizable fd as executable memory.
    ///
    /// For memfd, shm and Android shared memory this may be the first and last allowed resize.
    fn try_map_resize(size: size_t, fd: c_int, f: impl FnOnce(c_int) -> Self) -> OsResult<Self> {
        unsafe {
            os_err_if!(ftruncate(fd, size as off_t) == -1)?;
            Self::try_map(size, fd, f)
        }
    }

    /// Tries mapping anonymous read-write executable memory.
    fn try_map_anon(size: size_t) -> OsResult<Self> {
        unsafe {
            let flags = cfg_select! {
                any(test, target_vendor = "apple") => MAP_PRIVATE | MAP_ANONYMOUS | ffi::MAP_JIT,
                _ => MAP_PRIVATE | MAP_ANONYMOUS,
            };

            let rx = os_err_if!(
                mmap(ptr::null_mut::<c_void>(), size, PROT_RWX, flags, -1, 0) == MAP_FAILED
            )?;
            os_err_if!(munmap(rx, size) == -1)?;

            Ok(Self::Anonymous)
        }
    }
}

impl Drop for BackingFd {
    fn drop(&mut self) {
        let (Self::Resizable { fd, .. } | Self::FixedSize { fd, .. }) = self else {
            return;
        };

        let fd = *fd;
        if fd < 0 {
            return;
        }

        unsafe {
            close(fd);
        }
    }
}

#[cfg(any(not(unix), miri, target_os = "linux", target_os = "android"))]
fn memfd_create() -> OsResult<c_int> {
    unsafe {
        // N.B. the name used with `memfd_create` does not have to be unique.
        // Use a syscall in case `memfd_create` is not implemented.
        let fd = os_err_if!(syscall(SYS_memfd_create, c"closure-ffi".as_ptr(), MFD_CLOEXEC) == -1)?
            as c_int;

        Ok(fd)
    }
}

#[cfg(all(any(not(unix), miri, target_os = "android"), feature = "asharedmem"))]
fn asharedmem_create(size: size_t) -> OsResult<c_int> {
    unsafe {
        // ASharedMemory_create is documented to return -EINVAL
        os_err_if!(ffi::ASharedMemory_create(ptr::null::<c_char>(), size) < 0)
    }
}

#[cfg(all(any(not(unix), miri, target_os = "freebsd"), feature = "shm_open"))]
fn shm_open_anon() -> OsResult<c_int> {
    unsafe {
        os_err_if!(shm_open(ffi::SHM_ANON, O_RDWR | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR) == -1)
    }
}

#[cfg(all(
    any(
        not(unix),
        miri,
        all(not(target_vendor = "apple"), not(target_os = "android"))
    ),
    feature = "shm_open"
))]
fn shm_open_named(random_state: &mut u64) -> OsResult<c_int> {
    unsafe {
        let id = random_id(random_state);

        let name = alloc::format!("/closure-ffi-{id:016x}\0");
        let name_ptr = name.as_ptr() as *const c_char;

        let fd = os_err_if!(
            shm_open(
                name.as_ptr() as *const c_char,
                O_RDWR | O_CREAT | O_EXCL,
                S_IRUSR | S_IWUSR,
            ) == -1
        )?;

        // Don't care if unlinking fails.
        shm_unlink(name_ptr);

        Ok(fd)
    }
}

#[cfg(feature = "tmp_file")]
fn tmpfile_fileno() -> OsResult<c_int> {
    unsafe {
        let file = os_err_if!(tmpfile().is_null())?;

        os_err_if!(fileno(file) == -1)
            .and_then(|fd| {
                // This fd still belongs to the stream, dup it.
                os_err_if!(dup(fd) == -1)
            })
            .chain(
                // Close the stream (and the original fd).
                os_err_if!(fclose(file) == -1),
            )
    }
}

#[cfg(all(any(not(unix), miri, target_os = "linux"), feature = "tmp_file"))]
fn open_o_tmpfile_in(_dir: &CStr) -> OsResult<c_int> {
    unsafe {
        os_err_if!(
            open(
                _dir.as_ptr(),
                O_RDWR | O_EXCL | O_TMPFILE | O_CLOEXEC,
                (S_IRUSR | S_IWUSR) as mode_t,
            ) == -1
        )
    }
}

#[cfg(feature = "tmp_file")]
fn open_in_tmpdir(random_state: &mut u64) -> OsResult<c_int> {
    unsafe {
        let id = random_id(random_state);
        let mut tmp = tmpdir().to_owned().into_bytes();

        tmp.extend_from_slice(format!("/closure-ffi-{id:016x}\0").as_bytes());
        let name_ptr = tmp.as_ptr() as *const c_char;

        let flags = cfg_select! {
            target_os = "linux" => O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC,
            _ => O_RDWR | O_CREAT | O_EXCL,
        };

        let fd = os_err_if!(open(name_ptr, flags, (S_IRUSR | S_IWUSR) as c_uint) == -1)?;

        // Don't care if unlinking fails.
        unlink(name_ptr);

        Ok(fd)
    }
}

#[cfg(feature = "tmp_file")]
fn tmpdir() -> &'static CStr {
    // Get TMPDIR or default to /tmp
    let env = unsafe { getenv(c"TMPDIR".as_ptr()) };
    if !env.is_null() {
        unsafe { CStr::from_ptr(env) }
    } else {
        c"/tmp"
    }
}

/// Builds a pseudo-random 64-bit ID by hashing a state, function address, stack address,
/// timestamps and current PID.
#[cfg(any(feature = "shm_open", feature = "tmp_file"))]
fn random_id(random_state: &mut u64) -> u64 {
    let mut hasher = rustc_hash::FxHasher::with_seed(random_id as *const () as usize);

    random_state.hash(&mut hasher);

    hint::black_box(&raw mut hasher).hash(&mut hasher);

    unsafe {
        let mut ts_real = core::mem::zeroed();
        if clock_gettime(CLOCK_REALTIME, &mut ts_real) != -1 {
            ts_real.tv_nsec.hash(&mut hasher);
            ts_real.tv_sec.hash(&mut hasher);
        }

        let mut ts_monotonic = core::mem::zeroed();
        if clock_gettime(CLOCK_MONOTONIC, &mut ts_monotonic) != -1 {
            ts_monotonic.tv_nsec.hash(&mut hasher);
            ts_monotonic.tv_sec.hash(&mut hasher);
        }

        getpid().hash(&mut hasher);
    }

    let hash = hasher.finish();
    *random_state = random_state.wrapping_mul(hash) ^ hash;

    hash
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        ffi::{c_int, c_void},
        ptr, thread_local,
    };

    use hashbrown::HashMap;
    use rustc_hash::FxBuildHasher;

    use crate::arch::unix::{
        BackingFdProvider,
        ffi::{FILE, Os, PAGE_SIZE, timespec},
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
        do_map_rx_rw(|| ());
    }

    #[test]
    fn test_map_rx_rw_no_memfd() {
        do_map_rx_rw(|| {
            Os::with_mut(|os| os.syscall = |_, _, _| -1);
        });
    }

    #[test]
    fn test_map_rx_rw_no_asharedmem() {
        do_map_rx_rw(|| {
            Os::with_mut(|os| {
                os.syscall = |_, _, _| -1;
                os.ASharedMemory_create = |_, _| -1;
            });
        });
    }

    #[test]
    fn test_map_rx_rw_no_shm_open() {
        do_map_rx_rw(|| {
            Os::with_mut(|os| {
                os.syscall = |_, _, _| -1;
                os.ASharedMemory_create = |_, _| -1;
                os.shm_open = |_, _, _| -1;
            });
        });
    }

    fn do_map_rx_rw(setup: impl FnOnce()) {
        OsState::with(|| {
            setup();

            let mut fd_provider = BackingFdProvider::new();

            let granularity = granularity();
            let (rx, rw) = unsafe { map_rx_rw(granularity, &mut fd_provider).unwrap().unwrap() };

            assert_eq!(rx.align_offset(granularity), 0);
            assert_eq!(rw.align_offset(granularity), 0);

            unsafe {
                unmap_rw_rx(rx, rw, granularity).unwrap();
            }
        });
    }

    #[cfg(any(not(unix), miri))]
    mod mock {
        use crate::arch::unix::{BackingFd, granularity, memfd_create, tests::OsState};

        #[test]
        fn test_memfd_create() {
            OsState::with(|| {
                let size = granularity();
                let fd = memfd_create().unwrap();
                let _backing_fd =
                    BackingFd::try_map_resize(size, fd, |fd| BackingFd::FixedSize { fd }).unwrap();
            });
        }

        #[cfg(feature = "asharedmem")]
        #[test]
        fn test_asharedmem_create() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::asharedmem_create(size).unwrap();
                let _backing_fd =
                    BackingFd::try_map(size, fd, |fd| BackingFd::FixedSize { fd }).unwrap();
            });
        }

        #[cfg(feature = "shm_open")]
        #[test]
        fn test_shm_open_anon() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::shm_open_anon().unwrap();
                let _backing_fd =
                    BackingFd::try_map_resize(size, fd, |fd| BackingFd::FixedSize { fd }).unwrap();
            });
        }

        #[test]
        fn test_map_anon() {
            OsState::with(|| {
                let size = granularity();
                let _backing_fd = BackingFd::try_map_anon(size);
            });
        }

        #[cfg(feature = "shm_open")]
        #[test]
        fn test_shm_open_named() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::shm_open_named(&mut 1).unwrap();
                let _backing_fd =
                    BackingFd::try_map_resize(size, fd, |fd| BackingFd::FixedSize { fd }).unwrap();
            });
        }

        #[cfg(feature = "tmp_file")]
        #[test]
        fn test_tmpfile_fileno() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::tmpfile_fileno().unwrap();
                let _backing_fd = BackingFd::try_map_resize(size, fd, |fd| BackingFd::Resizable {
                    fd,
                    offset: 0,
                })
                .unwrap();
            });
        }

        #[cfg(feature = "tmp_file")]
        #[test]
        fn test_tmpfile_in_cwd() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::open_o_tmpfile_in(c"./").unwrap();
                let _backing_fd = BackingFd::try_map_resize(size, fd, |fd| BackingFd::Resizable {
                    fd,
                    offset: 0,
                })
                .unwrap();
            });
        }

        #[cfg(feature = "tmp_file")]
        #[test]
        fn test_tmpfile_in_tmpdir() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::open_o_tmpfile_in(crate::arch::unix::tmpdir()).unwrap();
                let _backing_fd = BackingFd::try_map_resize(size, fd, |fd| BackingFd::Resizable {
                    fd,
                    offset: 0,
                })
                .unwrap();
            });
        }

        #[cfg(feature = "tmp_file")]
        #[test]
        fn test_open_in_tmpdir() {
            OsState::with(|| {
                let size = granularity();
                let fd = crate::arch::unix::open_in_tmpdir(&mut 1).unwrap();
                let _backing_fd = BackingFd::try_map_resize(size, fd, |fd| BackingFd::Resizable {
                    fd,
                    offset: 0,
                })
                .unwrap();
            });
        }
    }

    struct OsState {
        next_fd: c_int,
        fds: HashMap<c_int, Fd, FxBuildHasher>,
        next_file: usize,
        files: HashMap<usize, c_int, FxBuildHasher>,
        next_map: usize,
        maps: HashMap<usize, usize, FxBuildHasher>,
        next_time: usize,
    }

    struct Fd {
        size: Option<usize>,
        resizable: bool,
    }

    thread_local! {
        static OS_STATE: RefCell<OsState> = const {
            RefCell::new(OsState {
                next_fd: 0,
                fds: HashMap::with_hasher(FxBuildHasher),
                next_file: 1,
                files: HashMap::with_hasher(FxBuildHasher),
                next_map: 1,
                maps: HashMap::with_hasher(FxBuildHasher),
                next_time: 1,
            })
        };
    }

    impl OsState {
        fn with(f: impl FnOnce()) {
            Os::with_mut(|os| *os = Default::default());

            OS_STATE.with_borrow_mut(|state| {
                state.next_fd = 0;
                state.fds.clear();
                state.next_file = 1;
                state.files.clear();
            });

            f();

            OS_STATE.with_borrow(|state| {
                assert_eq!(state.fds.len(), 0);
                assert_eq!(state.files.len(), 0);
                assert_eq!(state.maps.len(), 0);
            });
        }

        fn open_fd(&mut self, size: Option<usize>, resizable: bool) -> c_int {
            let fd = self.next_fd;
            self.next_fd += 1;
            self.fds.insert(fd, Fd { size, resizable });
            fd
        }

        #[track_caller]
        fn close_fd(&mut self, fd: c_int) {
            self.fds.remove(&fd).unwrap();
        }

        fn dup_fd(&mut self, fd: c_int) -> c_int {
            let fd = &self.fds[&fd];
            self.open_fd(fd.size, fd.resizable)
        }

        #[track_caller]
        fn resize_fd(&mut self, fd: c_int, new_size: usize) {
            let fd = self.fds.get_mut(&fd).unwrap();
            assert!(fd.resizable || fd.size.is_none());
            fd.size = Some(new_size);
        }

        fn open_file(&mut self) -> *mut FILE {
            let fd = self.open_fd(None, true);
            let file_value = self.next_file;
            self.next_file += 1;
            self.files.insert(file_value, fd);
            ptr::without_provenance_mut(file_value)
        }

        #[track_caller]
        fn close_file(&mut self, file: *mut FILE) {
            let fd = self.files.remove(&file.addr()).unwrap();
            self.close_fd(fd);
        }

        fn open_map(&mut self, len: usize, fd: c_int) -> *mut c_void {
            if fd != -1 {
                let fd = &self.fds[&fd];
                assert!(fd.size.is_none_or(|size| size == len));
            }

            let map_value = self.next_map;
            self.next_map += 1;
            self.maps.insert(map_value, len);
            ptr::without_provenance_mut(map_value * PAGE_SIZE as usize)
        }

        #[track_caller]
        fn close_map(&mut self, len: usize, map: *mut c_void) {
            assert_eq!(
                self.maps.remove(&(map.addr() / PAGE_SIZE as usize)),
                Some(len)
            );
        }
    }

    impl Default for Os {
        fn default() -> Self {
            Self::new(Self {
                ASharedMemory_create: |_, size| {
                    OS_STATE.with_borrow_mut(|os| os.open_fd(Some(size as usize), false))
                },
                clock_gettime: |_, ts| {
                    OS_STATE.with_borrow_mut(|os| unsafe {
                        ts.write(timespec {
                            tv_sec: os.next_time as _,
                            tv_nsec: 0,
                        });
                        os.next_time += 1;
                        0
                    })
                },
                close: |fd| {
                    OS_STATE.with_borrow_mut(|os| {
                        os.close_fd(fd);
                        0
                    })
                },
                dup: |fd| OS_STATE.with_borrow_mut(|os| os.dup_fd(fd)),
                fclose: |file| {
                    OS_STATE.with_borrow_mut(|os| {
                        os.close_file(file);
                        0
                    })
                },
                fileno: |file| OS_STATE.with_borrow(|os| os.files[&file.addr()]),
                ftruncate: |fd, size| {
                    OS_STATE.with_borrow_mut(|os| os.resize_fd(fd, size as usize));
                    0
                },
                getenv: |_| ptr::null_mut(),
                getpid: || 123,
                mmap: |_, len, _, _, fd, _| {
                    OS_STATE.with_borrow_mut(|os| os.open_map(len as usize, fd))
                },
                munmap: |map, len| {
                    OS_STATE.with_borrow_mut(|os| {
                        os.close_map(len as usize, map);
                        0
                    })
                },
                open: |_, _, _| OS_STATE.with_borrow_mut(|os| os.open_fd(None, true)),
                shm_open: |_, _, _| OS_STATE.with_borrow_mut(|os| os.open_fd(None, false)),
                shm_unlink: |_| 0,
                syscall: |_, _, _| OS_STATE.with_borrow_mut(|os| os.open_fd(None, false)) as _,
                sysconf: |_| PAGE_SIZE as _,
                tmpfile: || OS_STATE.with_borrow_mut(|os| os.open_file()),
                unlink: |_| 0,
            })
        }
    }
}
