//! Error types for allocation and deallocation.

#![cfg_attr(miri, allow(unused))]

use core::num::NonZero;

use thiserror::Error;

use crate::error::os::OsErrorInner;

pub(crate) mod os;

pub(crate) type OsResult<T> = Result<T, OsError>;

/// An operating system error encountered managing executable memory.
#[derive(Error, Debug)]
#[error(transparent)]
pub struct OsError {
    inner: OsErrorInner,
}

/// An allocator error for unsupported layouts.
///
/// Alignment greater than the OS page allocation granularity (as returned by
/// [`page_granularity`](crate::JitAlloc::page_granularity)) are not supported because of
/// the difficulty aligning allocations in a double rx-rw memory mapping.
#[derive(Error, Debug)]
pub enum LayoutError {
    /// Invalid layout (Rust standard library [`LayoutError`]).
    #[error("layout is invalid ({0})")]
    SizeAlign(#[from] alloc::alloc::LayoutError),

    /// Unsupported layout size.
    #[error("layout is too big ({0})")]
    Size(usize),

    /// Unsupported layout alignment (see [`LayoutError`] top level explanation).
    #[error("alignment ({align}) exceeds supported maximum ({max_align})")]
    Overaligned {
        /// The requested alignment.
        align: usize,

        /// The greatest possible alignment.
        max_align: usize,
    },
}

/// Errors returned by [`allocate`](crate::JitAlloc::allocate).
#[derive(Error, Debug)]
pub enum AllocateError {
    /// Unsupported layout (see [`LayoutError`]).
    #[error(transparent)]
    Layout(#[from] LayoutError),

    /// Failed to allocate executable memory by the OS.
    #[error(transparent)]
    Os(#[from] OsError),

    /// This allocator can't allocate any more memory.
    #[error("allocator is at maximum capacity")]
    Capacity,

    /// Other (uncommon) errors.
    #[error("could not allocate memory")]
    Other,
}

/// Errors returned by [`deallocate`](crate::JitAlloc::deallocate).
#[derive(Error, Debug)]
pub enum DeallocateError {
    /// Unsupported layout (see [`LayoutError`]).
    #[error(transparent)]
    Layout(#[from] LayoutError),

    /// Attempted to deallocate memory more than once or with a different layout.
    #[error("not a valid allocation")]
    InvalidParameter,

    /// Attempted to deallocate memory more than once.
    #[error("tried to deallocate a previously freed block")]
    DoubleFree,

    /// Failed to free executable memory by the OS.
    #[error(transparent)]
    Os(#[from] OsError),

    /// Other (uncommon) errors.
    #[error("could not deallocate memory")]
    Other,
}

impl OsError {
    #[cfg(not(debug_assertions))]
    pub(crate) fn new(code: Option<NonZero<i32>>, expr: &'static str) -> Self {
        Self {
            inner: OsErrorInner::new(code, expr),
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn new(
        code: Option<NonZero<i32>>,
        args: core::fmt::Arguments<'_>,
        file: &'static str,
        line: u32,
        column: u32,
    ) -> Self {
        Self {
            inner: OsErrorInner::new(code, args, file, line, column),
        }
    }

    pub(crate) fn chain(&mut self, other: Self) {
        self.inner.chain(other.inner);
    }
}

pub(crate) trait OsResultExt: Sized {
    fn chain<U>(self, other: OsResult<U>) -> Self;

    fn chain_err_with<U, F>(self, f: F) -> Self
    where
        F: FnOnce() -> OsResult<U>;
}

impl<T> OsResultExt for OsResult<T> {
    fn chain<U>(self, other: OsResult<U>) -> Self {
        match (self, other) {
            (res, Ok(_)) => res,
            (Ok(_), Err(other)) => Err(other),
            (Err(mut e), Err(other)) => {
                e.chain(other);
                Err(e)
            }
        }
    }

    fn chain_err_with<U, F>(self, f: F) -> Self
    where
        F: FnOnce() -> OsResult<U>,
    {
        self.map_err(|mut err| {
            if let Err(other) = f() {
                err.chain(other);
            }
            err
        })
    }
}
