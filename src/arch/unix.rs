#![cfg(all(unix, not(miri)))]
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

use libc::{
    _SC_PAGESIZE, CLOCK_MONOTONIC, CLOCK_REALTIME, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE,
    MAP_SHARED, O_CREAT, O_EXCL, O_RDWR, PROT_EXEC, PROT_READ, PROT_WRITE, S_IRUSR, S_IWUSR,
    clock_gettime, close, dup, fclose, fileno, ftruncate, getenv, getpid, mmap, munmap, off_t,
    open, size_t, sysconf, tmpfile, unlink,
};

use crate::{
    arch::unix::ffi::__errno_location,
    error::{OsResult, OsResultExt, os::err_if::os_err_if},
};

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
        let strategies: &'static [OpenStrategy] = &[
            #[cfg(any(target_os = "linux", target_os = "android"))]
            |_, size| BackingFd::try_map_resize(size, memfd_create()?, fixed),
            #[cfg(all(target_os = "android", feature = "asharedmem"))]
            |_, size| BackingFd::try_map_resize(size, asharedmem_create(size)?, fixed),
            #[cfg(all(target_os = "freebsd", feature = "shm_open"))]
            |_, size| BackingFd::try_map_resize(size, shm_open_anon()?, fixed),
            |_, size| BackingFd::try_map_anon(size),
            #[cfg(all(
                feature = "shm_open",
                not(target_vendor = "apple"),
                not(target_os = "android"),
            ))]
            |provider, size| {
                BackingFd::try_map_resize(size, shm_open(&mut provider.random_state)?, fixed)
            },
            #[cfg(feature = "tmp_file")]
            |_, size| BackingFd::try_map_resize(size, tmpfile_fileno()?, resizable),
            #[cfg(all(feature = "tmp_file", target_os = "linux"))]
            |_, size| BackingFd::try_map_resize(size, open_o_tmpfile_in(c"./")?, resizable),
            #[cfg(all(feature = "tmp_file", target_os = "linux"))]
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
    #[cfg(all(unix, not(miri)))]
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
    #[cfg(all(unix, not(miri)))]
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

            #[cfg(target_os = "macos")]
            {
                flags |= libc::MAP_JIT;
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
            target_os = "macos" => mmap_rwx(len, flags, fd, offset).map(Some),
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
                target_os = "macos" => (PROT_RWX, MAP_PRIVATE | libc::MAP_JIT),
                _ => (PROT_RX, MAP_SHARED),
            };

            let rx = os_err_if!(
                mmap(ptr::null_mut::<c_void>(), size, prot, flags, fd, 0) == MAP_FAILED
            )?;
            os_err_if!(munmap(rx, size) == -1)?;

            Ok(f(fd))
        }
    }

    /// Tries mapping a resizable fd as executable memory.
    ///
    /// For memfd, shm and Android shared memory this may be the first and last allowed resize.
    fn try_map_resize(size: size_t, fd: c_int, f: impl FnOnce(c_int) -> Self) -> OsResult<Self> {
        unsafe {
            os_err_if!(ftruncate(fd, size as off_t) == -1)?;
            Self::try_map(size, fd, f).chain_err_with(|| os_err_if!(close(fd) == -1))
        }
    }

    /// Tries mapping anonymous read-write executable memory.
    fn try_map_anon(size: size_t) -> OsResult<Self> {
        unsafe {
            let flags = cfg_select! {
                target_os = "macos" => MAP_PRIVATE | MAP_ANONYMOUS | libc::MAP_JIT,
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

#[cfg(any(target_os = "linux", target_os = "android"))]
fn memfd_create() -> OsResult<c_int> {
    unsafe {
        // N.B. the name used with `memfd_create` does not have to be unique.
        // Use a syscall in case `memfd_create` is not implemented.
        let fd = os_err_if!(
            libc::syscall(
                libc::SYS_memfd_create,
                c"closure-ffi".as_ptr(),
                libc::MFD_CLOEXEC,
            ) == -1
        )? as c_int;

        Ok(fd)
    }
}

#[cfg(all(target_os = "android", feature = "asharedmem"))]
fn asharedmem_create(_size: usize) -> OsResult<c_int> {
    unsafe {
        // ASharedMemory_create is documented to return -EINVAL
        os_err_if!(ndk_sys::ASharedMemory_create(ptr::null(), _size) < 0)
    }
}

#[cfg(target_os = "freebsd")]
fn shm_open_anon() -> OsResult<c_int> {
    unsafe {
        os_err_if!(
            libc::shm_open(libc::SHM_ANON, O_RDWR | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR) == -1
        )
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "android")))]
fn shm_open(random_state: &mut u64) -> OsResult<c_int> {
    unsafe {
        let id = random_id(random_state);

        let name = alloc::format!("/closure-ffi-{id:016x}\0");
        let name_ptr = name.as_ptr() as *const c_char;

        let fd = os_err_if!(
            libc::shm_open(
                name.as_ptr() as *const c_char,
                O_RDWR | O_CREAT | O_EXCL,
                S_IRUSR | S_IWUSR,
            ) == -1
        )?;

        // Don't care if unlinking fails.
        libc::shm_unlink(name_ptr);

        Ok(fd)
    }
}

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

#[cfg(target_os = "linux")]
fn open_o_tmpfile_in(_dir: &CStr) -> OsResult<c_int> {
    unsafe {
        os_err_if!(
            open(
                _dir.as_ptr(),
                O_RDWR | O_EXCL | libc::O_TMPFILE | libc::O_CLOEXEC,
                (S_IRUSR | S_IWUSR) as c_uint,
            ) == -1
        )
    }
}

fn open_in_tmpdir(random_state: &mut u64) -> OsResult<c_int> {
    unsafe {
        let id = random_id(random_state);
        let mut tmp = tmpdir().to_owned().into_bytes();

        tmp.extend_from_slice(format!("/closure-ffi-{id:016x}\0").as_bytes());
        let name_ptr = tmp.as_ptr() as *const c_char;

        let flags = cfg_select! {
            target_os = "linux" => O_RDWR | O_CREAT | O_EXCL | libc::O_CLOEXEC,
            _ => O_RDWR | O_CREAT | O_EXCL,
        };

        let fd = os_err_if!(open(name_ptr, flags, (S_IRUSR | S_IWUSR) as c_uint) == -1)?;

        // Don't care if unlinking fails.
        unlink(name_ptr);

        Ok(fd)
    }
}

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
