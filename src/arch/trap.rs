use core::slice;

use crate::arch::protect::JitWriteProtect;

pub unsafe fn trap_rx_rw(rx_ptr: *const u8, rw_ptr: *mut u8, size: usize) {
    unsafe {
        let _write_protect = JitWriteProtect::new();

        let bytes = slice::from_raw_parts_mut(rw_ptr, size);
        bytes.align_to_mut::<TrapCode>().1.fill(TRAP);

        clear_cache::clear_cache(rx_ptr, rx_ptr.add(size));
    }
}

cfg_select! {
    any(target_arch = "x86", target_arch = "x86_64") => {
        /// Trap instruction: int3
        pub const TRAP: TrapCode = 0xcc;
        pub type TrapCode = u8;
    }
    target_arch = "aarch64" => {
        /// Trap instruction: brk #0xF000
        pub const TRAP: TrapCode = TrapCode::from_be_bytes([0xd4, 0x3e, 0x00, 0x00]);
        pub type TrapCode = u32;
    }
    all(target_arch = "arm", target_feature = "thumb-mode") => {
        /// Trap instruction: bkpt #0
        pub const TRAP: TrapCode = TrapCode::from_be_bytes([0xbe, 0x00]);
        pub type TrapCode = u16;
    }
    target_arch = "arm" => {
        /// Trap instruction: bkpt #0
        pub const TRAP: TrapCode = TrapCode::from_be_bytes([0xe1, 0x20, 0x00, 0x70]);
        pub type TrapCode = u32;
    }
    _ => {
        /// Trap instruction zerofill fallback
        pub const TRAP: TrapCode = 0;
        pub type TrapCode = u32;
    }
}
