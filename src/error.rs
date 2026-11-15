use thiserror::Error;

#[derive(Error, Debug)]
pub enum LayoutError {
    #[error("layout is invalid ({0})")]
    SizeAlign(#[from] alloc::alloc::LayoutError),

    #[error("alignment ({align}) exceeds supported maximum ({max_align})")]
    Overaligned { align: usize, max_align: usize },
}

#[derive(Error, Debug)]
pub enum AllocateError {
    #[error(transparent)]
    Layout(#[from] LayoutError),

    #[error("operation failed (OS code {0})")]
    OsError(u32),
}

#[derive(Error, Debug)]
pub enum DeallocateError {
    #[error(transparent)]
    Layout(#[from] LayoutError),

    #[error("not a valid allocation")]
    InvalidParameter,

    #[error("tried to deallocate a previously freed block")]
    DoubleFree,

    #[error("operation failed (OS code {0})")]
    OsError(u32),
}
