#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use core::{alloc::Layout, mem, num::NonZero, ptr};

use alloc::collections::BTreeMap;
use hashbrown::HashMap;
use rustc_hash::FxBuildHasher;
use slab::Slab;

use crate::{
    arch::{
        MemmapSource,
        trap::{TrapCode, trap_rx_rw},
    },
    error::{AllocateError, DeallocateError, LayoutError},
    nonmax::NonMax,
};

pub(crate) mod arch;
pub mod error;
mod nonmax;

type ASize = NonZero<u16>;

type NodeGen = i64;
type NodeIndex = NonMax<u32>;

type ShardOffset = NonMax<u16>;

#[derive(Default, Debug)]
pub struct JitAlloc {
    source: MemmapSource,

    rw_by_rx: BTreeMap<*const u8, Shard>,
    large_shards_by_rx: HashMap<*const u8, LargeShard, FxBuildHasher>,

    free_heads: BTreeMap<ASize, NodeIndex>,
    free_start: Option<NodeIndex>,

    next_id: NodeGen,
    nodes: Slab<Node>,
}

#[derive(Debug)]
struct Shard {
    rw_ptr: *mut u8,
    nodes: HashMap<ShardOffset, NodeIndex, FxBuildHasher>,
    freed: bool,
}

#[derive(Debug)]
struct LargeShard {
    rw_ptr: *mut u8,
    size: usize,
    freed: bool,
}

#[derive(Debug)]
#[repr(C)]
struct Node {
    gen_id: NodeGen,

    next_free: Option<NodeIndex>,
    prev_free: Option<NodeIndex>,

    next: Option<NodeIndex>,
    prev: Option<NodeIndex>,

    shard_rx_ptr: *const u8,
    shard_rw_ptr: *mut u8,

    size: Option<ASize>,
    offset: Option<ShardOffset>,
    allocated_size: Option<ASize>,

    _pad2e: u16,
}

impl JitAlloc {
    const MIN_ALIGN: usize = 16;
    const SHARD_SIZE: usize = (1 << u16::BITS) * Self::MIN_ALIGN;
    const LARGE_SIZE: usize = Self::SHARD_SIZE / 3;

    pub const fn new() -> Self {
        Self {
            source: MemmapSource::new(),
            rw_by_rx: BTreeMap::new(),
            large_shards_by_rx: HashMap::with_hasher(FxBuildHasher),
            free_heads: BTreeMap::new(),
            free_start: None,
            next_id: 0,
            nodes: Slab::new(),
        }
    }

    pub fn allocate(&mut self, layout: Layout) -> Result<(*const u8, *mut u8), AllocateError> {
        if layout.pad_to_align().size() < Self::LARGE_SIZE {
            self.allocate_small(layout)
        } else {
            self.allocate_large(layout)
        }
    }

    fn allocate_small(&mut self, layout: Layout) -> Result<(*const u8, *mut u8), AllocateError> {
        let (layout, asize) = self.pad_layout_small(layout)?;
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
        if layout.pad_to_align().size() < Self::LARGE_SIZE {
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
        let (layout, size) = self.pad_layout_small(layout)?;
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

    fn get_or_insert_node(&mut self, asize: ASize) -> Result<&mut Node, AllocateError> {
        todo!()
    }

    fn pad_layout_small(&self, layout: Layout) -> Result<(Layout, ASize), LayoutError> {
        let granularity = self.source.granularity();
        let layout = pad_layout_to(layout, Self::MIN_ALIGN, granularity)?;

        assert!(layout.size() >= Self::MIN_ALIGN, "size padded to align");
        let non_zero = NonZero::new(layout.size() / Self::MIN_ALIGN).unwrap();

        let asize = ASize::try_from(non_zero)?;

        Ok((layout, asize))
    }

    fn pad_layout_large(&self, layout: Layout) -> Result<Layout, LayoutError> {
        let granularity = self.source.granularity();
        pad_layout_to(layout, granularity, granularity)
    }
}

fn pad_layout_to(
    layout: Layout,
    min_align: usize,
    max_align: usize,
) -> Result<Layout, LayoutError> {
    let (mut layout, _) = layout
        .align_to(min_align)?
        .extend(Layout::new::<TrapCode>())?;

    if layout.align() > max_align {
        return Err(LayoutError::Overaligned {
            align: layout.align(),
            max_align,
        });
    }

    if layout.align() > min_align {
        let overhead = Layout::from_size_align(layout.align() - min_align, min_align)?;
        (layout, _) = layout.extend(overhead)?;
    }

    Ok(layout.pad_to_align())
}

impl Drop for JitAlloc {
    fn drop(&mut self) {
        unsafe {
            for (rx_ptr, shard) in mem::take(&mut self.rw_by_rx) {
                if !shard.freed {
                    let _ = self.source.unmap_rw(rx_ptr, shard.rw_ptr, Self::SHARD_SIZE);
                }
            }

            for (rx_ptr, shard) in mem::take(&mut self.large_shards_by_rx) {
                if !shard.freed {
                    let _ = self.source.unmap_rw(rx_ptr, shard.rw_ptr, shard.size);
                }
            }
        }
    }
}

unsafe impl Send for JitAlloc {}

impl Node {
    const fn new(gen_id: NodeGen) -> Self {
        Self {
            gen_id,
            next_free: None,
            prev_free: None,
            next: None,
            prev: None,
            shard_rx_ptr: ptr::null(),
            shard_rw_ptr: ptr::null_mut(),
            size: None,
            offset: None,
            allocated_size: None,
            _pad2e: 0,
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        const { Node::new(0) }
    }
}
