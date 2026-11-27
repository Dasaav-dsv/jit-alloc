/// Writing executable memory on macOS requires this guard which calls
/// `pthread_jit_write_protect_np` on [`Self::new`] and on [`Self::drop`].
pub struct JitWriteProtect(());

impl JitWriteProtect {
    /// Creates a new write protect guard, calling `pthread_jit_write_protect_np(0)` on macOS.
    ///
    /// `pthread_jit_write_protect_np(1)` will be called on drop.
    pub fn new() -> Self {
        #[cfg(target_vendor = "apple")]
        unsafe {
            libc::pthread_jit_write_protect_np(0);
        }
        Self(())
    }
}

impl Drop for JitWriteProtect {
    fn drop(&mut self) {
        #[cfg(target_vendor = "apple")]
        unsafe {
            libc::pthread_jit_write_protect_np(1);
        }
    }
}
