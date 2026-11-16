use core::{fmt, num::NonZero};

use thiserror::Error;

use crate::error::os::OsErrorInner;

pub(crate) mod os;

pub type OsResult<T> = Result<T, OsError>;

#[derive(Error, Debug)]
#[error(transparent)]
pub struct OsError {
    inner: OsErrorInner,
}

#[derive(Error, Debug)]
pub enum LayoutError {
    #[error("layout is invalid ({0})")]
    SizeAlign(#[from] alloc::alloc::LayoutError),

    #[error("alignment ({align}) exceeds supported maximum ({max_align})")]
    Overaligned { align: usize, max_align: usize },

    #[error(transparent)]
    Os(#[from] OsError),
}

#[derive(Error, Debug)]
pub enum AllocateError {
    #[error(transparent)]
    Layout(#[from] LayoutError),

    #[error(transparent)]
    Os(#[from] OsError),

    #[error("could not allocate memory")]
    Other,
}

#[derive(Error, Debug)]
pub enum DeallocateError {
    #[error(transparent)]
    Layout(#[from] LayoutError),

    #[error("not a valid allocation")]
    InvalidParameter,

    #[error("tried to deallocate a previously freed block")]
    DoubleFree,

    #[error(transparent)]
    Os(#[from] OsError),
}

impl OsError {
    pub(crate) fn new(code: Option<NonZero<i32>>, args: fmt::Arguments<'_>) -> Self {
        Self {
            inner: OsErrorInner::new(code, args),
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
