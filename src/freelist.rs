use alloc::vec::Vec;

use crate::{AlignedSize, NodeIndex, SizeType};

type Node<S> = (AlignedSize<S>, NodeIndex);

#[derive(Default, Debug)]
pub struct Freelist<S: SizeType> {
    nodes: Vec<(AlignedSize<S>, NodeIndex)>,
}

impl<S: SizeType> Freelist<S> {
    pub const fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn insert(&mut self, node: Node<S>) -> Option<Node<S>> {
        let index = self.nodes.partition_point(|(size, _)| *size < node.0);

        if let Some(&(size, node_index)) = self.nodes.get(index)
            && size == node.0
        {
            self.nodes[index] = node;
            Some((size, node_index))
        } else {
            self.nodes.insert(index, (node.0, node.1));
            None
        }
    }

    pub fn remove(&mut self, node: Node<S>) -> Option<Node<S>> {
        let index = self
            .nodes
            .binary_search_by_key(&node.0, |(size, _)| *size)
            .ok()?;

        if node.1 == self.nodes[index].1 {
            self.nodes.remove(index);
            Some(node)
        } else {
            None
        }
    }

    pub fn first_fit(&self, size: AlignedSize<S>) -> Option<Node<S>> {
        let index = self.nodes.partition_point(|node| node.0 < size);
        self.nodes.get(index).cloned()
    }
}
