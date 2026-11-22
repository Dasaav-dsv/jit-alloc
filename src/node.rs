use crate::{AlignedOffset, AlignedSize, NodeId, NodeIndex, SizeType};

#[derive(Clone, Debug)]
#[cfg_attr(target_pointer_width = "16", repr(C, u16))]
#[cfg_attr(target_pointer_width = "32", repr(C, u16, align(4)))]
#[cfg_attr(target_pointer_width = "64", repr(C, u16, align(8)))]
pub enum Node<S: SizeType> {
    Free(FreeNode<S>),
    Allocated(AllocatedNode<S>),
}

#[derive(Clone, Debug)]
#[repr(C, packed(2))]
pub struct FreeNode<S: SizeType> {
    pub id: NodeId,

    pub size: AlignedSize<S>,
    pub offset: AlignedOffset<S>,

    pub next: Option<NodeIndex>,
    pub prev: Option<NodeIndex>,

    pub next_free: Option<NodeIndex>,
    pub prev_free: Option<NodeIndex>,

    pub shard_rx_addr: usize,
}

#[derive(Clone, Debug)]
#[repr(C, packed(2))]
pub struct AllocatedNode<S: SizeType> {
    pub id: NodeId,

    pub size: AlignedSize<S>,
    pub offset: AlignedOffset<S>,

    pub next: Option<NodeIndex>,
    pub prev: Option<NodeIndex>,

    pub allocated_size: AlignedSize<S>,
    pub allocated_offset: AlignedOffset<S>,
}

impl<S: SizeType> Node<S> {
    #[track_caller]
    pub fn unwrap_free(&self) -> &FreeNode<S> {
        let Node::Free(node) = self else {
            panic!("node is allocated");
        };
        node
    }

    #[track_caller]
    pub fn unwrap_free_mut(&mut self) -> &mut FreeNode<S> {
        let Node::Free(node) = self else {
            panic!("node is allocated");
        };
        node
    }

    #[track_caller]
    pub fn unwrap_allocated(&self) -> &AllocatedNode<S> {
        let Node::Allocated(node) = self else {
            panic!("node is free");
        };
        node
    }

    #[track_caller]
    pub fn unwrap_allocated_mut(&mut self) -> &mut AllocatedNode<S> {
        let Node::Allocated(node) = self else {
            panic!("node is free");
        };
        node
    }

    #[track_caller]
    pub fn make_free(
        &mut self,
        next_free: Option<NodeIndex>,
        prev_free: Option<NodeIndex>,
        shard_rx_addr: usize,
    ) -> &mut FreeNode<S> {
        let free = self.unwrap_allocated().clone();

        *self = Self::Free(FreeNode {
            id: free.id,
            size: free.size,
            offset: free.offset,
            next: free.next,
            prev: free.prev,
            next_free,
            prev_free,
            shard_rx_addr,
        });

        self.unwrap_free_mut()
    }

    #[track_caller]
    pub fn make_allocated(
        &mut self,
        allocated_size: AlignedSize<S>,
        allocated_offset: AlignedOffset<S>,
    ) -> &mut AllocatedNode<S> {
        let free = self.unwrap_free().clone();

        *self = Self::Allocated(AllocatedNode {
            id: free.id,
            size: free.size,
            offset: free.offset,
            next: free.next,
            prev: free.prev,
            allocated_size,
            allocated_offset,
        });

        self.unwrap_allocated_mut()
    }
}

macro_rules! impl_node {
    ($($i:ident: $t:ty => { $get:ident, $set:ident $(,)?}),+ $(,)?) => {
        impl<S: SizeType> Node<S> {
            $(
                pub fn $get(&self) -> $t {
                    match self {
                        Self::Free(node) => node.$i,
                        Self::Allocated(node) => node.$i,
                    }
                }

                pub fn $set(&mut self, $i: $t) {
                    match self {
                        Self::Free(node) => node.$i = $i,
                        Self::Allocated(node) => node.$i = $i,
                    }
                }
            )+
        }
    };
}

impl_node! {
    id: NodeId => { get_id, set_id },
    size: AlignedSize<S> => { get_size, set_size },
    offset: AlignedOffset<S> => { get_offset, set_offset },
    next: Option<NodeIndex> => { get_next, set_next },
    prev: Option<NodeIndex> => { get_prev, set_prev },
}
