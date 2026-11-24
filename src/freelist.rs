use alloc::collections::BTreeMap;

use crate::{AlignedSize, NodeIndex, SizeType};

type Node<S> = (AlignedSize<S>, NodeIndex);

#[derive(Default, Debug)]
pub struct Freelist<S: SizeType> {
    nodes: BTreeMap<AlignedSize<S>, NodeIndex>,
}

impl<S: SizeType> Freelist<S> {
    pub const fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, node: Node<S>) -> Option<Node<S>> {
        self.nodes
            .insert(node.0, node.1)
            .map(|index| (node.0, index))
    }

    pub fn remove(&mut self, node: Node<S>) -> Option<Node<S>> {
        self.nodes.remove(&node.0).map(|index| (node.0, index))
    }

    pub fn first_fit(&self, size: AlignedSize<S>) -> Option<Node<S>> {
        self.nodes
            .range(size..)
            .next()
            .map(|(size, index)| (*size, *index))
    }
}
