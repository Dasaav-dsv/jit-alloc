#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use core::{alloc::Layout, mem, num::TryFromIntError, ops::Bound};

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
    num::{Unsigned, nonmax::NonMax, positive::Positive},
    priority_treap::Treap,
};

pub(crate) mod arch;
pub mod error;
pub mod num;
mod priority_treap;

pub trait SizeType: Unsigned + TryFrom<usize, Error = TryFromIntError> {}

impl SizeType for u8 {}

impl SizeType for u16 {}

type Addr = usize;

type AlignedSize<S> = Positive<S>;
type AlignedOffset<S> = S;

type NodeId = i16;
type NodeIndex = NonMax<u32>;

#[derive(Debug)]
pub struct JitAlloc<S: SizeType = u16, const GRANULARITY: usize = 1> {
    source: MemmapSource,

    shards_by_rx: BTreeMap<Addr, Shard<S>>,
    large_shards_by_rx: HashMap<Addr, LargeShard, FxBuildHasher>,

    nodes: NodeList<S, GRANULARITY>,
}

#[derive(Debug)]
struct NodeList<S: SizeType, const GRANULARITY: usize> {
    next_id: NodeId,
    freelist: Treap<S>,
    nodes: Slab<Node<S>>,
}

#[derive(Clone, Debug)]
struct Node<S: SizeType> {
    id: NodeId,
    shard_rx_addr: usize,

    next_free: Option<NodeIndex>,
    prev_free: Option<NodeIndex>,

    next: Option<NodeIndex>,
    prev: Option<NodeIndex>,

    size: AlignedSize<S>,
    offset: AlignedOffset<S>,
    allocated_size: Option<AlignedSize<S>>,
}

#[derive(Debug)]
struct Shard<Size: SizeType> {
    rx_ptr: *const u8,
    rw_ptr: *mut u8,
    nodes: HashMap<Size, NodeIndex, FxBuildHasher>,
    freed: bool,
}

#[derive(Debug)]
struct LargeShard {
    rx_ptr: *const u8,
    rw_ptr: *mut u8,
    size: usize,
    freed: bool,
}

impl<Size: SizeType, const GRANULARITY: usize> JitAlloc<Size, GRANULARITY> {
    pub const SHARD_SIZE: usize = (1 << Size::BITS) * GRANULARITY;
    pub const LARGE_SIZE: usize = Self::SHARD_SIZE / 2;
    pub const MIN_ALIGN: usize = {
        assert!(GRANULARITY.is_power_of_two());
        GRANULARITY
    };
}

impl<Size: SizeType, const GRANULARITY: usize> JitAlloc<Size, GRANULARITY>
where
    Size: TryFrom<usize, Error = TryFromIntError>,
{
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

        let rx_addr = self.nodes.nodes[first_fit_index.as_usize()].shard_rx_addr;
        let shard = self
            .shards_by_rx
            .get_mut(&rx_addr)
            .expect("node is not owned?");

        let (size, offset) =
            self.nodes
                .allocate_in_node(shard, first_fit_index, size, layout.align())?;

        let size_bytes = size.as_wide().as_usize() * Self::MIN_ALIGN;
        assert!(size_bytes >= layout.size(), "size too small");

        let offset_bytes = offset.as_usize() * Self::MIN_ALIGN;

        let rx_ptr = shard.rx_ptr.wrapping_add(offset_bytes);
        assert_eq!(rx_ptr.align_offset(layout.align()), 0, "alignment failed");

        let rw_ptr = shard.rw_ptr.wrapping_add(offset_bytes);
        assert_eq!(rw_ptr.align_offset(layout.align()), 0, "alignment failed");

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
            rx_ptr.addr(),
            LargeShard {
                rx_ptr,
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
            AlignedSize::MAX,
            AlignedOffset::MIN,
            None,
            None,
        ) {
            Ok(index) => index,
            Err(e) => unsafe {
                let _ = self.source.unmap_rw_rx(rx_ptr, rw_ptr, Self::SHARD_SIZE);
                return Err(e);
            },
        };

        self.shards_by_rx.insert(rx_ptr.addr(), shard);

        Ok(node_index)
    }

    /// # Safety
    /// 
    /// TODO
    pub unsafe fn deallocate(
        &mut self,
        rx_ptr: *const u8,
        layout: Layout,
    ) -> Result<(), DeallocateError> {
        if rx_ptr.align_offset(Self::MIN_ALIGN) != 0 {
            return Err(DeallocateError::InvalidParameter);
        }

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
        let (_, size) = self.pad_layout_small(layout)?;

        let min = rx_ptr.addr().saturating_sub(Self::SHARD_SIZE);
        let max = rx_ptr.addr();

        let (shard_addr, shard) = self
            .shards_by_rx
            .range_mut((Bound::Excluded(&min), Bound::Included(&max)))
            .next()
            .ok_or(DeallocateError::InvalidParameter)?;

        let offset = AlignedOffset::<Size>::try_from((max - *shard_addr) / Self::MIN_ALIGN)
            .map_err(|_| DeallocateError::InvalidParameter)?;

        self.nodes.deallocate_in_shard(shard, size, offset)?;

        let size_bytes = size.as_wide().as_usize() * Self::MIN_ALIGN;
        let offset_bytes = offset.as_usize() * Self::MIN_ALIGN;

        let rw_ptr = shard.rw_ptr.wrapping_add(offset_bytes);

        unsafe {
            trap_rx_rw(rx_ptr, rw_ptr, size_bytes);
        }

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
            .get_mut(&rx_ptr.addr())
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

    fn pad_layout_small(&self, layout: Layout) -> Result<(Layout, AlignedSize<Size>), LayoutError> {
        let granularity = self.source.granularity();
        let layout = pad_layout_to(layout, Self::MIN_ALIGN, granularity)?;

        assert!(layout.size() >= Self::MIN_ALIGN, "size padded to align");
        let size = AlignedSize::<Size>::try_from(layout.size() / Self::MIN_ALIGN)?;

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

impl<Size: SizeType, const GRANULARITY: usize> Default for JitAlloc<Size, GRANULARITY> {
    fn default() -> Self {
        const { Self::new() }
    }
}

impl<Size: SizeType, const GRANULARITY: usize> Drop for JitAlloc<Size, GRANULARITY> {
    fn drop(&mut self) {
        unsafe {
            for shard in mem::take(&mut self.shards_by_rx).into_values() {
                if !shard.freed {
                    let _ = self
                        .source
                        .unmap_rw(shard.rx_ptr, shard.rw_ptr, Self::SHARD_SIZE);
                }
            }

            for shard in mem::take(&mut self.large_shards_by_rx).into_values() {
                if !shard.freed {
                    let _ = self.source.unmap_rw(shard.rx_ptr, shard.rw_ptr, shard.size);
                }
            }
        }
    }
}

impl<S: SizeType, const GRANULARITY: usize> NodeList<S, GRANULARITY> {
    const MIN_ALIGN: usize = {
        assert!(GRANULARITY.is_power_of_two());
        GRANULARITY
    };

    const fn new() -> Self {
        Self {
            next_id: 0,
            freelist: Treap::new(),
            nodes: Slab::new(),
        }
    }

    fn allocate_in_node(
        &mut self,
        shard: &mut Shard<S>,
        node_index: NodeIndex,
        size: AlignedSize<S>,
        align: usize,
    ) -> Result<(AlignedSize<S>, AlignedOffset<S>), AllocateError> {
        let mut node = {
            let node = &mut self.nodes[node_index.as_usize()];
            if node.id < 0 {
                node.id = !node.id;
            }
            node.clone()
        };

        let mut original = (node.size, node_index, node.id);

        assert!(node.allocated_size.is_none(), "node already allocated");

        let offset_bytes = node.offset.as_usize() * Self::MIN_ALIGN;

        let rx_ptr_base = shard.rx_ptr.wrapping_add(offset_bytes);
        let rx_align_offset = rx_ptr_base.align_offset(align);

        let rw_ptr_base = shard.rw_ptr.wrapping_add(offset_bytes);
        let rw_align_offset = rw_ptr_base.align_offset(align);

        assert_eq!(rx_align_offset, rw_align_offset, "impossible alignment",);

        let align_offset: AlignedOffset<S> = rx_align_offset
            .div_ceil(Self::MIN_ALIGN)
            .try_into()
            .expect("alignment must fit shard");

        if let Some(prev_size) = AlignedSize::<S>::new(align_offset.as_wide()) {
            let new_offset = node
                .offset
                .checked_add(align_offset)
                .expect("offset must fit shard");

            self.adjust_node(
                shard,
                node_index,
                prev_size.as_wide().as_signed(),
                Some(new_offset),
            );

            self.allocate_in_shard(shard, prev_size, node.offset, Some(node_index), node.prev)?;

            node = self.nodes[node_index.as_usize()].clone();
            original = (node.size, node_index, node.id);
        }

        assert!(node.size >= size, "node is too small");

        if let Some(next_size) = node.size.checked_sub(size) {
            let next_offset = (size + node.offset).get().expect("offset must fit shard");

            self.allocate_in_shard(shard, next_size, next_offset, node.next, Some(node_index))?;

            let updated = &mut self.nodes[node_index.as_usize()];
            updated.size = size;

            node = updated.clone();
        }

        self.remove_from_freelist(original);

        self.nodes[node_index.as_usize()].allocated_size = Some(size);

        Ok((size, node.offset))
    }

    fn allocate_in_shard(
        &mut self,
        shard: &mut Shard<S>,
        size: AlignedSize<S>,
        offset: AlignedOffset<S>,
        next: Option<NodeIndex>,
        prev: Option<NodeIndex>,
    ) -> Result<NodeIndex, AllocateError> {
        let id = self.next_id();
        let key = self.nodes.insert(Node {
            id,
            shard_rx_addr: shard.rx_ptr.addr(),
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

        self.insert_into_freelist((size, node_index, id));

        shard
            .nodes
            .try_insert(offset, node_index)
            .expect("can't not overwrite another node");

        Ok(node_index)
    }

    fn deallocate_in_shard(
        &mut self,
        shard: &mut Shard<S>,
        size: AlignedSize<S>,
        offset: AlignedOffset<S>,
    ) -> Result<(), DeallocateError> {
        let node_index = *shard
            .nodes
            .get(&offset)
            .ok_or(DeallocateError::InvalidParameter)?;

        let node = &mut self.nodes[node_index.as_usize()];

        if node
            .allocated_size
            .is_none_or(|allocated_size| allocated_size != size)
        {
            return Err(DeallocateError::DoubleFree);
        }

        // Fixup if an offset adjustment was skipped due to this node being allocated.
        if offset != node.offset {
            let removed = shard.nodes.remove(&offset);
            assert_eq!(removed, Some(node_index), "wrong shard or index");

            shard
                .nodes
                .try_insert(node.offset, node_index)
                .expect("can't overwrite another node");
        }

        assert!(node.id >= 0, "allocated node has negative priority");

        assert_eq!(node.next_free, None, "allocated node in freelist");
        assert_eq!(node.prev_free, None, "allocated node in freelist");

        let size = node.size;

        node.size = AlignedSize::MIN;
        node.allocated_size = None;

        if let Some(merge_size) = size.checked_sub(AlignedSize::MIN) {
            let merge_offset = node
                .offset
                .checked_add(S::ONE)
                .expect("offset must fit shard");

            if let Some(next_index) = node.next {
                self.adjust_node(
                    shard,
                    next_index,
                    merge_size.as_wide().as_signed(),
                    Some(merge_offset),
                );
            } else {
                let prev = node.prev.unwrap_or(node_index);
                self.allocate_in_shard(shard, merge_size, merge_offset, None, Some(prev))
                    .map_err(|_| DeallocateError::Other)?;
            }
        }

        let Node {
            next, prev, offset, ..
        } = self.nodes[node_index.as_usize()];

        if let Some(prev_index) = prev {
            if let Some(next_index) = next {
                let (prev, next) = self
                    .nodes
                    .get2_mut(prev_index.as_usize(), next_index.as_usize())
                    .expect("Slab indices are unique and don't dangle");

                prev.next = Some(next_index);
                next.prev = Some(prev_index);
            } else {
                let prev = &mut self.nodes[prev_index.as_usize()];
                prev.next = None;
            }

            shard.nodes.remove(&offset);
            self.nodes.remove(node_index.as_usize());

            self.adjust_node(shard, prev_index, S::Wide::ONE.as_signed(), None);

            return Ok(());
        }

        let node = &mut self.nodes[node_index.as_usize()];
        node.id = !node.id;

        let node = (node.size, node_index, node.id);
        self.insert_into_freelist(node);

        Ok(())
    }

    fn insert_into_freelist(
        &mut self,
        (size, node_index, id): (AlignedSize<S>, NodeIndex, NodeId),
    ) {
        assert_eq!(
            self.nodes[node_index.as_usize()].allocated_size,
            None,
            "tried to insert an allocated node",
        );

        let Some((_, p_index, _)) = self.freelist.insert((size, node_index, id)) else {
            return;
        };

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

    fn remove_from_freelist(
        &mut self,
        (size, node_index, id): (AlignedSize<S>, NodeIndex, NodeId),
    ) {
        let Node {
            next_free,
            prev_free,
            ..
        } = self.nodes[node_index.as_usize()];

        if let Some(_removed) = self.freelist.remove((size, node_index, id))
            && let Some(prev_index) = prev_free
        {
            let prev = &self.nodes[prev_index.as_usize()];
            assert_eq!(prev.allocated_size, None, "prev must be free");

            self.freelist.insert((prev.size, prev_index, prev.id));
        }

        let node = &mut self.nodes[node_index.as_usize()];
        node.next_free = None;
        node.prev_free = None;

        if let Some(next_index) = next_free
            && let Some(prev_index) = prev_free
        {
            if next_index == prev_index {
                let prev = &mut self.nodes[prev_index.as_usize()];
                prev.next_free = None;
                prev.prev_free = None;
            } else {
                let (next, prev) = self
                    .nodes
                    .get2_mut(next_index.as_usize(), prev_index.as_usize())
                    .expect("Slab indices are unique and don't dangle");

                next.prev_free = Some(prev_index);
                prev.next_free = Some(next_index);

                assert_ne!(next.next_free, Some(node_index), "broken link");
                assert_ne!(prev.prev_free, Some(node_index), "broken link");
            }
        } else {
            assert!(
                next_free.is_none() && prev_free.is_none(),
                "broken list links",
            );
        }
    }

    fn adjust_node(
        &mut self,
        shard: &mut Shard<S>,
        node_index: NodeIndex,
        delta_size: <S::Wide as Unsigned>::Signed,
        new_offset: Option<AlignedOffset<S>>,
    ) {
        let node = &mut self.nodes[node_index.as_usize()];

        if node.allocated_size.is_none() {
            let node = (node.size, node_index, node.id);
            self.remove_from_freelist(node);
        }

        let node = &mut self.nodes[node_index.as_usize()];

        node.size = node
            .size
            .as_wide()
            .checked_add_signed(delta_size)
            .and_then(AlignedSize::new)
            .expect("size must fit node");

        if let Some(new_offset) = new_offset {
            // Skip offset adjustment if the node is allocated.
            if node.allocated_size.is_none() {
                let removed = shard.nodes.remove(&node.offset);
                assert_eq!(removed, Some(node_index), "wrong shard or index");

                shard
                    .nodes
                    .try_insert(new_offset, node_index)
                    .expect("can't overwrite another node");
            }

            node.offset = new_offset;
        }

        if node.allocated_size.is_none() {
            let node = (node.size, node_index, node.id);
            self.insert_into_freelist(node);
        }
    }

    fn next_id(&mut self) -> NodeId {
        let next_id = self.next_id;
        self.next_id = next_id.checked_add(1).unwrap_or(0);
        next_id
    }
}

unsafe impl<S: SizeType> Send for Shard<S> {}
unsafe impl<S: SizeType> Sync for Shard<S> {}

unsafe impl Send for LargeShard {}
unsafe impl Sync for LargeShard {}
