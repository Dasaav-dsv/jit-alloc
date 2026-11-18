use core::cmp::Ordering;

use slab::Slab;

use crate::{ASize, NodeId, NodeIndex};

type Node = (ASize, NodeIndex, NodeId);

#[derive(Default, Debug)]
pub struct Treap {
    nodes: Slab<TreapNode>,
    root: Option<NodeIndex>,
}

#[derive(Clone, Copy, Debug)]
struct TreapNode {
    inner: Node,

    left: Option<NodeIndex>,
    right: Option<NodeIndex>,
    parent: Option<NodeIndex>,
}

#[derive(Clone, Copy, Debug)]
enum ChildIndex {
    Root,
    Node { parent: NodeIndex, is_left: bool },
}

impl Treap {
    pub const fn new() -> Self {
        Self {
            nodes: Slab::new(),
            root: None,
        }
    }

    pub fn insert(&mut self, node: Node) -> Option<Node> {
        let mut replaced = None;

        let mut i = match self.binary_search(node.0) {
            Ok(replace) => {
                let to_replace = &mut self.nodes[replace.as_usize()];
                replaced = Some(to_replace.inner);
                to_replace.inner = node;
                replace
            }
            Err(ChildIndex::Root) => {
                let root = self
                    .nodes
                    .insert(TreapNode::new(node))
                    .try_into()
                    .expect("exceeded u32::MAX nodes");

                self.root = Some(root);
                return None;
            }
            Err(ChildIndex::Node { parent, is_left }) => {
                let child = self
                    .nodes
                    .insert(TreapNode {
                        parent: Some(parent),
                        ..TreapNode::new(node)
                    })
                    .try_into()
                    .expect("exceeded u32::MAX nodes");

                if is_left {
                    self.nodes[parent.as_usize()].left = Some(child);
                } else {
                    self.nodes[parent.as_usize()].right = Some(child);
                }

                child
            }
        };

        while let Some(parent_i) = self.nodes[i.as_usize()].parent {
            if self.nodes[i.as_usize()].inner.2 <= self.nodes[parent_i.as_usize()].inner.2 {
                break;
            }

            self.rotate(i, parent_i);

            i = parent_i;
        }

        replaced
    }

    pub fn remove(&mut self, i: NodeIndex) -> Option<Node> {
        loop {
            let node = self.nodes.get(i.as_usize())?;

            let child_i = match (node.left, node.right) {
                (Some(left), Some(right)) => {
                    let left_size = self.nodes[left.as_usize()].inner.0;
                    let right_size = self.nodes[left.as_usize()].inner.0;
                    if left_size < right_size { left } else { right }
                }
                (Some(left), None) => left,
                (None, Some(right)) => right,
                _ => break,
            };

            self.rotate(child_i, i);
        }

        let removed = self.nodes.remove(i.as_usize());

        match removed.parent {
            None => self.root = None,
            Some(parent_i) => {
                let parent = &mut self.nodes[parent_i.as_usize()];
                if Some(i) == parent.left {
                    parent.left = None;
                } else {
                    parent.right = None;
                }
            }
        }

        Some(removed.inner)
    }

    pub fn get(&self, size: ASize) -> Option<&Node> {
        let i = self.binary_search(size).ok()?;
        Some(&self.nodes[i.as_usize()].inner)
    }

    pub fn get_mut(&mut self, size: ASize) -> Option<&mut Node> {
        let i = self.binary_search(size).ok()?;
        Some(&mut self.nodes[i.as_usize()].inner)
    }

    pub fn first_fit(&self, size: ASize) -> Option<&Node> {
        let mut i = self.root?;
        loop {
            let node = &self.nodes[i.as_usize()];

            match size.cmp(&node.inner.0) {
                Ordering::Less | Ordering::Equal => break Some(&node.inner),
                Ordering::Greater => i = node.right?,
            }
        }
    }

    fn binary_search(&self, size: ASize) -> Result<NodeIndex, ChildIndex> {
        let mut i = self.root.ok_or(ChildIndex::Root)?;
        loop {
            let node = &self.nodes[i.as_usize()];

            match size.cmp(&node.inner.0) {
                Ordering::Less => {
                    i = node.left.ok_or(ChildIndex::Node {
                        parent: i,
                        is_left: true,
                    })?
                }
                Ordering::Greater => {
                    i = node.right.ok_or(ChildIndex::Node {
                        parent: i,
                        is_left: false,
                    })?
                }
                Ordering::Equal => break Ok(i),
            }
        }
    }

    fn rotate(&mut self, child_i: NodeIndex, parent_i: NodeIndex) {
        let (node, parent) = self
            .nodes
            .get2_mut(child_i.as_usize(), parent_i.as_usize())
            .expect("treap contradiction");

        node.parent = parent.parent;
        parent.parent = Some(child_i);

        if Some(child_i) == parent.left {
            // rotate right
            parent.left = node.right;
            node.right = Some(parent_i);
        } else {
            // rotate left
            parent.right = node.left;
            node.left = Some(parent_i);
        }

        if let Some(grandparent_i) = node.parent {
            let grandparent = &mut self.nodes[grandparent_i.as_usize()];

            if Some(parent_i) == grandparent.left {
                grandparent.left = Some(child_i);
            } else {
                grandparent.right = Some(child_i);
            }
        } else {
            self.root = Some(child_i);
        }
    }
}

impl TreapNode {
    const fn new(inner: Node) -> Self {
        Self {
            inner,
            left: None,
            right: None,
            parent: None,
        }
    }
}
