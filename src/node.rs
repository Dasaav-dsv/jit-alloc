use crate::{AlignedOffset, AlignedSize, NodeIndex, SizeType};

#[derive(Clone, Debug)]
pub enum Node<S: SizeType> {
    Free {
        size: AlignedSize<S>,
        offset: AlignedOffset<S>,

        next_free: Option<NodeIndex>,

        next: Option<NodeIndex>,
        prev: Option<NodeIndex>,

        prev_free: Option<NodeIndex>,

        shard_rx_addr: usize,
    },
    Allocated {
        size: AlignedSize<S>,
        offset: AlignedOffset<S>,

        next: Option<NodeIndex>,
        prev: Option<NodeIndex>,

        allocated_size: AlignedSize<S>,
        allocated_offset: AlignedOffset<S>,
    },
}

#[allow(unused)]
pub struct FreeNode<'a, S: SizeType> {
    pub size: &'a mut AlignedSize<S>,
    pub offset: &'a mut AlignedOffset<S>,
    pub next_free: &'a mut Option<NodeIndex>,
    pub next: &'a mut Option<NodeIndex>,
    pub prev: &'a mut Option<NodeIndex>,
    pub prev_free: &'a mut Option<NodeIndex>,
    pub shard_rx_addr: &'a mut usize,
}

#[allow(unused)]
pub struct AllocatedNode<'a, S: SizeType> {
    pub size: &'a mut AlignedSize<S>,
    pub offset: &'a mut AlignedOffset<S>,
    pub next: &'a mut Option<NodeIndex>,
    pub prev: &'a mut Option<NodeIndex>,
    pub allocated_size: &'a mut AlignedSize<S>,
    pub allocated_offset: &'a mut AlignedOffset<S>,
}

impl<S: SizeType> Node<S> {
    #[track_caller]
    pub fn unwrap_free(&mut self) -> FreeNode<'_, S> {
        let Node::Free {
            size,
            offset,
            next_free,
            next,
            prev,
            prev_free,
            shard_rx_addr,
        } = self
        else {
            panic!("node is allocated");
        };

        FreeNode {
            size,
            offset,
            next_free,
            next,
            prev,
            prev_free,
            shard_rx_addr,
        }
    }

    #[track_caller]
    pub fn unwrap_allocated(&mut self) -> AllocatedNode<'_, S> {
        let Node::Allocated {
            size,
            offset,
            next,
            prev,
            allocated_size,
            allocated_offset,
        } = self
        else {
            panic!("node is free");
        };

        AllocatedNode {
            size,
            offset,
            next,
            prev,
            allocated_size,
            allocated_offset,
        }
    }

    #[track_caller]
    pub fn make_free(
        &mut self,
        next_free: Option<NodeIndex>,
        prev_free: Option<NodeIndex>,
        shard_rx_addr: usize,
    ) -> FreeNode<'_, S> {
        *self = Self::Free {
            size: self.size(),
            offset: self.offset(),
            next_free,
            next: self.next(),
            prev: self.prev(),
            prev_free,
            shard_rx_addr,
        };

        self.unwrap_free()
    }

    #[track_caller]
    pub fn make_allocated(
        &mut self,
        allocated_size: AlignedSize<S>,
        allocated_offset: AlignedOffset<S>,
    ) -> AllocatedNode<'_, S> {
        *self = Self::Allocated {
            size: self.size(),
            offset: self.offset(),
            next: self.next(),
            prev: self.prev(),
            allocated_size,
            allocated_offset,
        };

        self.unwrap_allocated()
    }
}

macro_rules! impl_node {
    ($($i:ident: $t:ty => { $ref:ident, $mut:ident $(,)?}),+ $(,)?) => {
        impl<S: SizeType> Node<S> {
            $(
                pub fn $ref(&self) -> $t {
                    let (Self::Free { $i, .. } | Self::Allocated { $i, .. }) = self;
                    *$i
                }

                pub fn $mut(&mut self) -> &mut $t {
                    let (Self::Free { $i, .. } | Self::Allocated { $i, .. }) = self;
                    $i
                }
            )+
        }
    };
}

impl_node! {
    size: AlignedSize<S> => { size, size_mut },
    offset: AlignedOffset<S> => { offset, offset_mut },
    next: Option<NodeIndex> => { next, next_mut },
    prev: Option<NodeIndex> => { prev, prev_mut },
}
