#![no_std]

extern crate alloc;

use core::{alloc::Layout, mem, num::NonZero};

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

type NodeId = i64;
type NodeIndex = NonMax<u32>;

type AlignSize = NonZero<u16>;
type AlignOffset = NonMax<u16>;

#[derive(Default, Debug)]
pub struct JitAlloc {
    source: MemmapSource,

    shards_by_rx: BTreeMap<*const u8, Shard>,
    large_shards_by_rx: HashMap<*const u8, LargeShard, FxBuildHasher>,

    nodes: NodeList,
}

#[derive(Default, Debug)]
struct NodeList {
    next_id: NodeId,
    freelist: Treap,
    nodes: Slab<Node>,
}

#[derive(Debug)]
struct Shard {
    rx_ptr: *const u8,
    rw_ptr: *mut u8,
    nodes: HashMap<AlignOffset, NodeIndex, FxBuildHasher>,
    freed: bool,
}

#[derive(Debug)]
struct LargeShard {
    _rx_ptr: *const u8,
    rw_ptr: *mut u8,
    size: usize,
    freed: bool,
}

#[derive(Clone, Debug)]
struct Node {
    id: NodeId,
    shard_rx_ptr: *const u8,

    next_free: Option<NodeIndex>,
    prev_free: Option<NodeIndex>,

    next: Option<NodeIndex>,
    prev: Option<NodeIndex>,

    size: AlignSize,
    offset: AlignOffset,
    allocated_size: Option<AlignSize>,
}

impl JitAlloc {
    const MIN_ALIGN: usize = 16;
    const SHARD_SIZE: usize = (1 << u16::BITS) * Self::MIN_ALIGN;
    const LARGE_SIZE: usize = Self::SHARD_SIZE / 3;

    pub const fn new() -> Self {
        Self {
            source: MemmapSource::new(),
            shards_by_rx: BTreeMap::new(),
            large_shards_by_rx: HashMap::with_hasher(FxBuildHasher),
            nodes: NodeList::new(),
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
        let (layout, size) = self.pad_layout_small(layout)?;

        let first_fit_index = match self.nodes.freelist.first_fit(size) {
            Some((_, node_index, _)) => *node_index,
            None => self.allocate_shard()?,
        };

        let rx_ptr = self.nodes.nodes[first_fit_index.as_usize()].shard_rx_ptr;
        let shard = self
            .shards_by_rx
            .get_mut(&rx_ptr)
            .expect("node is not owned?");

        let (size, offset) =
            self.nodes
                .allocate_in_node(shard, first_fit_index, size, layout.align())?;

        let size_bytes = size.get() as usize * Self::MIN_ALIGN;
        assert!(size_bytes >= layout.size(), "bug: size too small");

        let offset_bytes = offset.as_usize() * Self::MIN_ALIGN;

        let rx_ptr = shard.rx_ptr.wrapping_add(offset_bytes);
        assert_eq!(
            rx_ptr.align_offset(layout.align()),
            0,
            "bug: alignment failed"
        );

        let rw_ptr = shard.rw_ptr.wrapping_add(offset_bytes);
        assert_eq!(
            rw_ptr.align_offset(layout.align()),
            0,
            "bug: alignment failed"
        );

        unsafe {
            trap_rx_rw(rx_ptr, rw_ptr, size_bytes);
        }

        Ok((rx_ptr, rw_ptr))
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
                _rx_ptr: rx_ptr,
                rw_ptr,
                size,
                freed: false,
            },
        );

        Ok((rx_ptr, rw_ptr))
    }

    fn allocate_shard(&mut self) -> Result<NodeIndex, AllocateError> {
        let (rx_ptr, rw_ptr) = self
            .source
            .map_rx_rw(Self::SHARD_SIZE)?
            .ok_or(AllocateError::Other)?;

        unsafe {
            trap_rx_rw(rx_ptr, rw_ptr, Self::SHARD_SIZE);
        }

        let mut shard = Shard {
            rx_ptr,
            rw_ptr,
            nodes: HashMap::with_capacity_and_hasher(2, FxBuildHasher),
            freed: false,
        };

        let node_index = match self.nodes.allocate_in_shard(
            &mut shard,
            AlignSize::MAX,
            AlignOffset::MIN,
            None,
            None,
        ) {
            Ok(index) => index,
            Err(e) => unsafe {
                let _ = self.source.unmap_rw_rx(rx_ptr, rw_ptr, Self::SHARD_SIZE);
                return Err(e);
            },
        };

        self.shards_by_rx.insert(rx_ptr, shard);

        Ok(node_index)
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

    fn pad_layout_small(&self, layout: Layout) -> Result<(Layout, AlignSize), LayoutError> {
        let granularity = self.source.granularity();
        let layout = pad_layout_to(layout, Self::MIN_ALIGN, granularity)?;

        assert!(layout.size() >= Self::MIN_ALIGN, "size padded to align");
        let non_zero = NonZero::new(layout.size() / Self::MIN_ALIGN).unwrap();

        let size = AlignSize::try_from(non_zero)?;

        Ok((layout, size))
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
            for (rx_ptr, shard) in mem::take(&mut self.shards_by_rx) {
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

impl NodeList {
    const MIN_ALIGN: usize = JitAlloc::MIN_ALIGN;

    const fn new() -> Self {
        Self {
            next_id: 0,
            freelist: Treap::new(),
            nodes: Slab::new(),
        }
    }

    fn allocate_in_node(
        &mut self,
        shard: &mut Shard,
        node_index: NodeIndex,
        size: AlignSize,
        align: usize,
    ) -> Result<(AlignSize, AlignOffset), AllocateError> {
        let mut node = self.nodes[node_index.as_usize()].clone();
        let original = (node.size, node_index, node.id);

        assert!(node.allocated_size.is_none(), "bug: node already allocated");

        let offset_bytes = node.offset.as_usize() * Self::MIN_ALIGN;

        let rx_ptr_base = shard.rx_ptr.wrapping_add(offset_bytes);
        let rx_align_offset = rx_ptr_base.align_offset(align);

        let rw_ptr_base = shard.rw_ptr.wrapping_add(offset_bytes);
        let rw_align_offset = rw_ptr_base.align_offset(align);

        assert_eq!(
            rx_align_offset, rw_align_offset,
            "bug: impossible alignment",
        );

        let align_offset = rx_align_offset
            .div_ceil(Self::MIN_ALIGN)
            .try_into()
            .expect("alignment must fit shard");

        if let Some(prev_size) = AlignSize::new(align_offset) {
            let new_size = AlignSize::new(node.size.get().strict_sub(prev_size.get()))
                .expect("size must fit node");

            let new_offset = AlignOffset::new(node.offset.get().strict_add(align_offset))
                .expect("offset must fit shard");

            self.allocate_in_shard(shard, prev_size, node.offset, Some(node_index), node.prev)?;

            let updated = &mut self.nodes[node_index.as_usize()];
            updated.size = new_size;
            updated.offset = new_offset;

            node = updated.clone();
        }

        assert!(node.size >= size, "bug: node is too small");

        if let Some(next_size) = AlignSize::new(node.size.get() - size.get()) {
            let next_offset =
                AlignOffset::new(node.offset.get() + size.get()).expect("offset must fit shard");

            self.allocate_in_shard(shard, next_size, next_offset, node.next, Some(node_index))?;

            let updated = &mut self.nodes[node_index.as_usize()];
            updated.size = size;

            node = updated.clone();
        }

        if let Some(_removed) = self.freelist.remove(original)
            && let Some(prev_index) = node.prev_free
        {
            let prev = &self.nodes[prev_index.as_usize()];
            self.freelist.insert((prev.size, prev_index, prev.id));
        }

        if let Some(next_index) = node.next_free
            && let Some(prev_index) = node.prev_free
        {
            if next_index == prev_index {
                let prev = &mut self.nodes[prev_index.as_usize()];
                prev.next_free = None;
                prev.prev_free = None;
            } else {
                let (next, prev) = self
                    .nodes
                    .get2_mut(next_index.as_usize(), prev_index.as_usize())
                    .expect("Slab indices don't dangle");

                next.prev_free = Some(prev_index);
                prev.next_free = Some(next_index);

                assert_ne!(next.next_free, Some(node_index), "bug: broken link");
                assert_ne!(prev.prev_free, Some(node_index), "bug: broken link");
            }
        } else {
            assert!(
                node.next_free.is_none() && node.prev_free.is_none(),
                "bug: broken list links",
            );
        }

        self.nodes[node_index.as_usize()].allocated_size = Some(size);

        Ok((size, node.offset))
    }

    fn allocate_in_shard(
        &mut self,
        shard: &mut Shard,
        size: AlignSize,
        offset: AlignOffset,
        next: Option<NodeIndex>,
        prev: Option<NodeIndex>,
    ) -> Result<NodeIndex, AllocateError> {
        let id = self.next_id();
        let key = self.nodes.insert(Node {
            id,
            shard_rx_ptr: shard.rx_ptr,
            next_free: None,
            prev_free: None,
            next,
            prev,
            size,
            offset,
            allocated_size: None,
        });

        let node_index = match NodeIndex::try_from(key) {
            Ok(index) => index,
            Err(_) => {
                self.nodes.remove(key);
                return Err(AllocateError::Capacity);
            }
        };

        if let Some(next_index) = next {
            self.nodes[next_index.as_usize()].prev = Some(node_index);
        }

        if let Some(prev_index) = prev {
            self.nodes[prev_index.as_usize()].next = Some(node_index);
        }

        if let Some((_, p_index, _)) = self.freelist.insert((size, node_index, id)) {
            let (node, prev) = self
                .nodes
                .get2_mut(node_index.as_usize(), p_index.as_usize())
                .expect("Slab indices are unique and don't dangle");

            let pn_index = prev.next_free;
            let pp_index = prev.prev_free;

            node.next_free = Some(pn_index.unwrap_or(p_index));
            node.prev_free = Some(p_index);

            prev.next_free = Some(node_index);
            prev.prev_free = Some(pp_index.unwrap_or(node_index));

            if let Some(pn_index) = pn_index {
                self.nodes[pn_index.as_usize()].prev_free = Some(node_index);
            }
        }

        shard.nodes.insert(offset, node_index);

        Ok(node_index)
    }

    fn next_id(&mut self) -> NodeId {
        let next_id = self.next_id;
        self.next_id += 1;
        next_id
    }
}

#[test]
fn alloc() {
    extern crate std;

    use std::println;

    let mut alloc = JitAlloc::new();

    println!("{:?}", alloc.allocate(Layout::new::<[u8; 1]>()));
    println!("{:?}", alloc.allocate(Layout::new::<[u8; 32]>()));
    println!("{:?}", alloc.allocate(Layout::new::<[u64; 40]>()));
    println!("{:?}", alloc.allocate(Layout::new::<[u128; 1]>()));
    println!("{:?}", alloc.allocate(Layout::new::<[u128; 132]>()));

    println!("{alloc:#?}");
}
