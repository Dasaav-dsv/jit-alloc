#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use core::alloc::Layout;

use alloc::collections::BTreeMap;
use hashbrown::HashMap;
use roaring::RoaringBitmap;
use rustc_hash::FxBuildHasher;

use crate::{
    arch::{
        MemmapSource,
        trap::{TrapCode, trap_rx_rw},
    },
    error::{AllocateError, DeallocateError, LayoutError},
};

pub(crate) mod arch;
pub mod error;

#[derive(Default, Debug)]
pub struct JitAlloc {
    source: MemmapSource,
    shards_by_rx: BTreeMap<*const u8, Shard>,
    large_shards_by_rx: HashMap<*const u8, LargeShard, FxBuildHasher>,
}

#[derive(Debug)]
struct Shard {
    rw_ptr: *mut u8,
    next_offset: usize,
    allocated: RoaringBitmap,
    used_up: RoaringBitmap,
}

#[derive(Debug)]
struct LargeShard {
    rw_ptr: *mut u8,
    size: usize,
    freed: bool,
}

impl JitAlloc {
    const MIN_ALIGN: usize = 16;
    const SHARD_SIZE: usize = (1 << u16::BITS) * Self::MIN_ALIGN;
    const LARGE_SIZE: usize = Self::SHARD_SIZE / 4;

    pub const fn new() -> Self {
        Self {
            source: MemmapSource::new(),
            shards_by_rx: BTreeMap::new(),
            large_shards_by_rx: HashMap::with_hasher(FxBuildHasher),
        }
    }

    pub fn allocate(&mut self, layout: Layout) -> Result<(*const u8, *mut u8), AllocateError> {
        if layout.size() + layout.align() < Self::LARGE_SIZE {
            self.allocate_small(layout)
        } else {
            self.allocate_large(layout)
        }
    }

    fn allocate_small(&mut self, layout: Layout) -> Result<(*const u8, *mut u8), AllocateError> {
        let layout = self.pad_layout_small(layout)?;
        todo!()
    }

    fn allocate_large(&mut self, layout: Layout) -> Result<(*const u8, *mut u8), AllocateError> {
        let layout = self.pad_layout_large(layout)?;

        let size = layout.size();
        let (rx_ptr, rw_ptr) = self.source.map_rx_rw(size)?.ok_or(AllocateError::Other)?;

        unsafe {
            trap_rx_rw(rx_ptr, rw_ptr, size);
        }

        self.large_shards_by_rx.insert(
            rx_ptr,
            LargeShard {
                rw_ptr,
                size,
                freed: false,
            },
        );

        Ok((rx_ptr, rw_ptr))
    }

    pub unsafe fn deallocate(
        &mut self,
        rx_ptr: *const u8,
        layout: Layout,
    ) -> Result<(), DeallocateError> {
        if layout.size() + layout.align() < Self::LARGE_SIZE {
            self.deallocate_small(rx_ptr, layout)
        } else {
            self.deallocate_large(rx_ptr, layout)
        }
    }

    fn deallocate_small(
        &mut self,
        rx_ptr: *const u8,
        layout: Layout,
    ) -> Result<(), DeallocateError> {
        let layout = self.pad_layout_small(layout)?;
        Ok(())
    }

    fn deallocate_large(
        &mut self,
        rx_ptr: *const u8,
        layout: Layout,
    ) -> Result<(), DeallocateError> {
        let layout = self.pad_layout_large(layout)?;

        let shard = self
            .large_shards_by_rx
            .get_mut(&rx_ptr)
            .ok_or(DeallocateError::InvalidParameter)?;

        if shard.size != layout.size() {
            return Err(DeallocateError::InvalidParameter);
        }

        if shard.freed {
            return Err(DeallocateError::DoubleFree);
        }

        shard.freed = true;

        unsafe {
            self.source.unmap_rw_rx(rx_ptr, shard.rw_ptr, shard.size)?;
        }

        Ok(())
    }

    fn pad_layout_small(&mut self, layout: Layout) -> Result<Layout, LayoutError> {
        let granularity = self.source.granularity();
        pad_layout_to(layout, Self::MIN_ALIGN, granularity)
    }

    fn pad_layout_large(&mut self, layout: Layout) -> Result<Layout, LayoutError> {
        let granularity = self.source.granularity();
        pad_layout_to(layout, granularity, granularity)
    }
}

fn pad_layout_to(
    layout: Layout,
    min_align: usize,
    max_align: usize,
) -> Result<Layout, LayoutError> {
    let (layout, _) = layout
        .align_to(min_align)?
        .extend(Layout::new::<TrapCode>())?;

    if layout.align() > max_align {
        return Err(LayoutError::Overaligned {
            align: layout.align(),
            max_align,
        });
    }

    Ok(layout.pad_to_align())
}
