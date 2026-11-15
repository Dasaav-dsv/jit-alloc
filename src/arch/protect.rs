/// Using the allocator on macOS requires this guard which calls `pthread_jit_write_protect_np`
/// on [`Self::new`] and on [`Self::drop`].
pub struct JitWriteProtect(());

impl JitWriteProtect {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        unsafe {
            libc::pthread_jit_write_protect_np(0);
        }
        Self(())
    }
}

impl Drop for JitWriteProtect {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe {
            libc::pthread_jit_write_protect_np(1);
        }
    }
}
