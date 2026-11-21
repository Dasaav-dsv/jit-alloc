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
pub struct JitAlloc<S: SizeType = u16, const ALIGN: usize = 1> {
    source: MemmapSource,

    shards_by_rx: BTreeMap<Addr, Shard<S>>,
    large_shards_by_rx: HashMap<Addr, LargeShard, FxBuildHasher>,

    nodes: NodeList<S, ALIGN>,
}

#[derive(Debug)]
struct NodeList<S: SizeType, const ALIGN: usize> {
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
    allocated_size: AlignedSize<S>,

    offset: AlignedOffset<S>,
    allocated_offset: Option<AlignedOffset<S>>,
}

#[derive(Debug)]
struct Shard<S: SizeType> {
    rx_ptr: *const u8,
    rw_ptr: *mut u8,
    allocated: HashMap<AlignedOffset<S>, NodeIndex, FxBuildHasher>,
    freed: bool,
}

#[derive(Debug)]
struct LargeShard {
    rx_ptr: *const u8,
    rw_ptr: *mut u8,
    size: usize,
    freed: bool,
}

impl<S: SizeType, const ALIGN: usize> JitAlloc<S, ALIGN> {
    pub const SHARD_SIZE: usize = (1 << S::BITS) * ALIGN;
    pub const LARGE_SIZE: usize = Self::SHARD_SIZE / 2;
    pub const MIN_ALIGN: usize = {
        assert!(ALIGN.is_power_of_two());
        ALIGN
    };
}

impl<S: SizeType, const ALIGN: usize> JitAlloc<S, ALIGN>
where
    S: TryFrom<usize, Error = TryFromIntError>,
{
    pub const fn new() -> Self {
        let _ = Self::MIN_ALIGN;
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
        let (layout, size, max_size) = self.pad_layout_small(layout)?;

        let first_fit_index = match self.nodes.freelist.first_fit(max_size) {
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

        let size_bytes = size.as_wide().as_usize() * ALIGN;
        assert!(size_bytes >= layout.size(), "size too small");

        let offset_bytes = offset.as_usize() * ALIGN;

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
        let size = self.shard_size();

        let (rx_ptr, rw_ptr) = self.source.map_rx_rw(size)?.ok_or(AllocateError::Other)?;

        unsafe {
            trap_rx_rw(rx_ptr, rw_ptr, size);
        }

        let mut shard = Shard {
            rx_ptr,
            rw_ptr,
            allocated: HashMap::with_hasher(FxBuildHasher),
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
                let _ = self.source.unmap_rw_rx(rx_ptr, rw_ptr, size);
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
        if rx_ptr.align_offset(ALIGN) != 0 {
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
        let (_, size, _) = self.pad_layout_small(layout)?;

        let min = rx_ptr.addr().saturating_sub(self.shard_size());
        let max = rx_ptr.addr();

        let (shard_addr, shard) = self
            .shards_by_rx
            .range_mut((Bound::Excluded(&min), Bound::Included(&max)))
            .next()
            .ok_or(DeallocateError::InvalidParameter)?;

        let offset = AlignedOffset::<S>::try_from((max - *shard_addr) / ALIGN)
            .map_err(|_| DeallocateError::InvalidParameter)?;

        self.nodes.deallocate_in_shard(shard, size, offset)?;

        let size_bytes = size.as_wide().as_usize() * ALIGN;
        let offset_bytes = offset.as_usize() * ALIGN;

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

    fn pad_layout_small(
        &self,
        layout: Layout,
    ) -> Result<(Layout, AlignedSize<S>, AlignedSize<S>), LayoutError> {
        let granularity = self.source.granularity();
        let (layout, max_size) = pad_layout_to(layout, ALIGN, granularity)?;

        let size = AlignedSize::<S>::try_from(layout.size().div_ceil(ALIGN))?;
        let max_size = AlignedSize::<S>::try_from(max_size.div_ceil(ALIGN))?;

        Ok((layout, size, max_size))
    }

    fn pad_layout_large(&self, layout: Layout) -> Result<Layout, LayoutError> {
        let granularity = self.source.granularity();
        Ok(pad_layout_to(layout, granularity, granularity)?
            .0
            .pad_to_align())
    }

    fn shard_size(&self) -> usize {
        self.source.granularity().max(Self::SHARD_SIZE)
    }
}

fn pad_layout_to(
    layout: Layout,
    min_align: usize,
    max_align: usize,
) -> Result<(Layout, usize), LayoutError> {
    let layout = layout
        .extend(Layout::new::<TrapCode>())?
        .0
        .align_to(min_align)?;

    if layout.align() > max_align {
        return Err(LayoutError::Overaligned {
            align: layout.align(),
            max_align,
        });
    }

    if layout.align() <= min_align {
        return Ok((layout, layout.size()));
    }

    let overhead = Layout::from_size_align(layout.align() - min_align, min_align)?;
    let overhead_layout = layout.extend(overhead)?.0.pad_to_align();

    Ok((layout, overhead_layout.size()))
}

impl<S: SizeType, const ALIGN: usize> Default for JitAlloc<S, ALIGN> {
    fn default() -> Self {
        const { Self::new() }
    }
}

impl<S: SizeType, const ALIGN: usize> Drop for JitAlloc<S, ALIGN> {
    fn drop(&mut self) {
        unsafe {
            for shard in mem::take(&mut self.shards_by_rx).into_values() {
                if !shard.freed {
                    let _ = self
                        .source
                        .unmap_rw(shard.rx_ptr, shard.rw_ptr, self.shard_size());
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

impl<S: SizeType, const ALIGN: usize> NodeList<S, ALIGN> {
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
        let mut node = self.nodes[node_index.as_usize()].clone();
        let mut original = (node.size, node_index, node.id);

        assert!(node.allocated_offset.is_none(), "node already allocated");

        let offset_bytes = node.offset.as_usize() * ALIGN;

        let rx_ptr_base = shard.rx_ptr.wrapping_add(offset_bytes);
        let rx_align_offset = rx_ptr_base.align_offset(align);

        let rw_ptr_base = shard.rw_ptr.wrapping_add(offset_bytes);
        let rw_align_offset = rw_ptr_base.align_offset(align);

        assert_eq!(rx_align_offset, rw_align_offset, "impossible alignment",);

        let align_offset: AlignedOffset<S> = rx_align_offset
            .div_ceil(ALIGN)
            .try_into()
            .expect("alignment must fit shard");

        if let Some(prev_size) = AlignedSize::<S>::new(align_offset.as_wide()) {
            let new_offset = node
                .offset
                .checked_add(align_offset)
                .expect("offset must fit shard");

            self.adjust_node(
                node_index,
                -prev_size.as_wide().as_signed(),
                Some(new_offset),
            );

            self.allocate_in_shard(shard, prev_size, node.offset, Some(node_index), node.prev)?;

            node = self.nodes[node_index.as_usize()].clone();
            original = (node.size, node_index, node.id);
        }

        assert!(node.size >= size, "node is too small");

        if let Some(next_size) = node.size.checked_sub(size) {
            let next_offset = (size + node.offset).get().expect("offset must fit shard");

            if let Some(next_index) = node.next {
                assert!(
                    self.nodes[next_index.as_usize()].offset > next_offset,
                    "next offset contradiction",
                );

                self.adjust_node(
                    next_index,
                    next_size.as_wide().as_signed(),
                    Some(next_offset),
                );
            } else {
                self.allocate_in_shard(shard, next_size, next_offset, node.next, Some(node_index))?;
            }
        }

        self.remove_from_freelist(original);

        let node = &mut self.nodes[node_index.as_usize()];

        if let Err(e) = shard.allocated.try_insert(node.offset, node_index) {
            let node = node.clone();
            let occ = &self.nodes[e.entry.get().as_usize()];
            let exp = &self.nodes[shard
                .allocated
                .get(&occ.allocated_offset.unwrap())
                .unwrap()
                .as_usize()];

            panic!("{node:#?}\n{occ:#?}\n{exp:#?}")
        }

        node.size = size;
        node.allocated_size = size;
        node.allocated_offset = Some(node.offset);

        if node.id < 0 {
            node.id = !node.id;
        }

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
            allocated_size: AlignedSize::MIN,
            offset,
            allocated_offset: None,
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
        } else if let Some(prev_index) = prev {
            assert_eq!(self.nodes[prev_index.as_usize()].next, None, "broken link");
        }

        if let Some(prev_index) = prev {
            self.nodes[prev_index.as_usize()].next = Some(node_index);
        } else if let Some(next_index) = next {
            assert_eq!(self.nodes[next_index.as_usize()].prev, None, "broken link");
        }

        self.insert_into_freelist((size, node_index, id));

        Ok(node_index)
    }

    fn deallocate_in_shard(
        &mut self,
        shard: &mut Shard<S>,
        size: AlignedSize<S>,
        offset: AlignedOffset<S>,
    ) -> Result<(), DeallocateError> {
        let node_index = *shard
            .allocated
            .get(&offset)
            .ok_or(DeallocateError::InvalidParameter)?;

        let node = &mut self.nodes[node_index.as_usize()];

        if node.allocated_offset != Some(offset) || node.allocated_size != size {
            return Err(DeallocateError::DoubleFree);
        }

        assert!(node.id >= 0, "allocated node has negative priority");

        assert_eq!(node.next_free, None, "allocated node in freelist");
        assert_eq!(node.prev_free, None, "allocated node in freelist");

        if let Some(merge_size) = node.size.checked_sub(AlignedSize::MIN) {
            let merge_offset = node
                .offset
                .checked_add(S::ONE)
                .expect("offset must fit shard");

            if let Some(next_index) = node.next {
                self.adjust_node(
                    next_index,
                    merge_size.as_wide().as_signed(),
                    Some(merge_offset),
                );
            } else {
                self.allocate_in_shard(shard, merge_size, merge_offset, None, Some(node_index))
                    .map_err(|_| DeallocateError::Other)?;
            }
        }

        shard.allocated.remove(&offset);

        let node = &mut self.nodes[node_index.as_usize()];

        node.size = AlignedSize::MIN;
        node.allocated_size = AlignedSize::MIN;
        node.allocated_offset = None;

        if let Some(prev_index) = node.prev {
            if let Some(next_index) = node.next {
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

            self.nodes.remove(node_index.as_usize());

            self.adjust_node(prev_index, S::Wide::ONE.as_signed(), None);

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
            self.nodes[node_index.as_usize()].allocated_offset,
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
            assert_eq!(prev.allocated_offset, None, "prev must be free");

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
        node_index: NodeIndex,
        delta_size: <S::Wide as Unsigned>::Signed,
        new_offset: Option<AlignedOffset<S>>,
    ) {
        let node = &mut self.nodes[node_index.as_usize()];

        if node.allocated_offset.is_none() {
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
            node.offset = new_offset;
        }

        if node.allocated_offset.is_none() {
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

#[cfg(test)]
mod tests {
    use alloc::{alloc::Layout, vec};

    use rand::{RngExt, distr::uniform::SampleRange};

    use crate::{JitAlloc, SizeType};

    #[test]
    fn test_u8_1() {
        test_alloc_class::<u8, 1>(1..=120);
    }

    #[test]
    fn test_u8_2() {
        test_alloc_class::<u8, 2>(1..=240);
    }

    #[test]
    fn test_u8_4() {
        test_alloc_class::<u8, 4>(1..=480);
    }

    #[test]
    fn test_u8_8() {
        test_alloc_class::<u8, 8>(1..=960);
    }

    #[test]
    fn test_u8_16() {
        test_alloc_class::<u8, 16>(1..=1920);
    }

    #[test]
    fn test_u8_32() {
        test_alloc_class::<u8, 32>(1..=3840);
    }

    #[test]
    fn test_u16_1_small() {
        test_alloc_class::<u16, 1>(1..=120);
    }

    #[test]
    fn test_u16_1_large() {
        test_alloc_class::<u16, 1>(1..=32760);
    }

    #[test]
    fn test_u16_2_small() {
        test_alloc_class::<u16, 2>(1..=240);
    }

    #[test]
    fn test_u16_2_large() {
        test_alloc_class::<u16, 2>(1..=65520);
    }

    #[test]
    fn test_u16_4_small() {
        test_alloc_class::<u16, 4>(1..=480);
    }

    #[test]
    fn test_u16_4_large() {
        test_alloc_class::<u16, 4>(1..=131040);
    }

    #[test]
    fn test_u16_8_small() {
        test_alloc_class::<u16, 8>(1..=960);
    }

    #[test]
    fn test_u16_8_large() {
        test_alloc_class::<u16, 8>(1..=262080);
    }

    #[test]
    fn test_u16_16_small() {
        test_alloc_class::<u16, 16>(1..=1920);
    }

    #[test]
    fn test_u16_16_large() {
        test_alloc_class::<u16, 16>(1..=524160);
    }

    fn test_alloc_class<S: SizeType, const ALIGN: usize>(size: impl SampleRange<usize> + Clone) {
        let mut rng = rand::rng();

        let mut alloc = JitAlloc::<S, ALIGN>::new();

        let mut allocs = vec![None; 1000];

        for _ in 0..1000000 {
            let i = rng.random_range(0..1000);

            match allocs[i].take() {
                None => {
                    let size = rng.random_range(size.clone());
                    let align = 1 << rng.random_range(0..8);
                    let layout = Layout::from_size_align(size, align).unwrap();
                    allocs[i] = Some((alloc.allocate(layout).unwrap().0, layout));
                }
                Some((rx_ptr, layout)) => unsafe {
                    alloc.deallocate(rx_ptr, layout).unwrap();
                },
            }
        }
    }
}
