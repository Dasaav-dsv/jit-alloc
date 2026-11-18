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
    priority_treap::Treap,
};

pub(crate) mod arch;
pub mod error;
mod nonmax;
mod priority_treap;

type ASize = NonZero<u16>;

type NodeId = i64;
type NodeIndex = NonMax<u32>;

type ShardOffset = NonMax<u16>;

#[derive(Default, Debug)]
pub struct JitAlloc {
    source: MemmapSource,

    rw_by_rx: BTreeMap<*const u8, Shard>,
    large_shards_by_rx: HashMap<*const u8, LargeShard, FxBuildHasher>,

    free_list: Treap,
    free_start: Option<NodeIndex>,

    next_id: NodeId,
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
struct Node {
    id: NodeId,
    shard_rx_ptr: *const u8,

    next_free: Option<NodeIndex>,
    prev_free: Option<NodeIndex>,

    next: Option<NodeIndex>,
    prev: Option<NodeIndex>,

    size: Option<ASize>,
    offset: Option<ShardOffset>,
    allocated_size: Option<ASize>,
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
            free_list: Treap::new(),
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

        let first_fit = match self.free_list.first_fit(asize) {
            Some(first_fit) => first_fit,
            None => {
                self.allocate_shard()?;

                self.free_list
                    .first_fit(asize)
                    .ok_or(AllocateError::Other)?
            }
        };

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

    fn allocate_shard(&mut self) -> Result<(), AllocateError> {
        let (rx_ptr, rw_ptr) = self
            .source
            .map_rx_rw(Self::SHARD_SIZE)?
            .ok_or(AllocateError::Other)?;

        unsafe {
            trap_rx_rw(rx_ptr, rw_ptr, Self::SHARD_SIZE);
        }

        let mut shard = Shard {
            rw_ptr,
            nodes: HashMap::with_capacity_and_hasher(2, FxBuildHasher),
            freed: false,
        };

        let id = self.next_id();
        let key = self.nodes.insert(Node {
            id,
            shard_rx_ptr: rx_ptr,
            next_free: None,
            prev_free: None,
            next: None,
            prev: None,
            size: Some(ASize::MAX),
            offset: Some(ShardOffset::MIN),
            allocated_size: None,
        });

        let node_index = match NodeIndex::try_from(key) {
            Ok(index) => index,
            Err(_) => unsafe {
                let _ = self.nodes.remove(key);
                let _ = self.source.unmap_rw_rx(rx_ptr, rw_ptr, Self::SHARD_SIZE);
                return Err(AllocateError::Capacity);
            },
        };

        shard.nodes.insert(ShardOffset::MIN, node_index);

        todo!()
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

    fn next_id(&mut self) -> NodeId {
        let next_id = self.next_id;
        self.next_id += 1;
        next_id
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
    const fn new(id: NodeId) -> Self {
        Self {
            id,
            next_free: None,
            prev_free: None,
            next: None,
            prev: None,
            shard_rx_ptr: ptr::null(),
            size: None,
            offset: None,
            allocated_size: None,
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        const { Node::new(0) }
    }
}
