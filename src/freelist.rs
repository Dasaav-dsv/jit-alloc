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

    pub fn try_insert(&mut self, node: Node<S>) -> Result<(), &mut Node<S>> {
        let index = self.nodes.partition_point(|(size, _)| *size < node.0);

        if let Some((size, _)) = self.nodes.get(index)
            && *size == node.0
        {
            Err(&mut self.nodes[index])
        } else {
            self.nodes.insert(index, (node.0, node.1));
            Ok(())
        }
    }

    pub fn remove(&mut self, node: Node<S>) {
        let Ok(index) = self.nodes.binary_search_by_key(&node.0, |(size, _)| *size) else {
            return;
        };

        if node.1 == self.nodes[index].1 {
            self.nodes.remove(index);
        }
    }

    pub fn replace(&mut self, node: Node<S>, prev: NodeIndex) {
        let Ok(index) = self.nodes.binary_search_by_key(&node.0, |(size, _)| *size) else {
            return;
        };

        if node.1 == self.nodes[index].1 {
            self.nodes[index].1 = prev;
        }
    }

    pub fn first_fit(&self, size: AlignedSize<S>) -> Option<Node<S>> {
        let index = self.nodes.partition_point(|node| node.0 < size);
        self.nodes.get(index).cloned()
    }
}
