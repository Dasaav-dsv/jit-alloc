use core::cmp::Ordering;

use slab::Slab;

use crate::{AlignedSize, NodeId, NodeIndex, SizeType};

type Node<S> = (AlignedSize<S>, NodeIndex, NodeId);

#[derive(Default, Debug)]
pub struct Treap<S: SizeType> {
    nodes: Slab<TreapNode<S>>,
    root: Option<NodeIndex>,
}

#[derive(Clone, Copy, Debug)]
struct TreapNode<S: SizeType> {
    inner: Node<S>,

    left: Option<NodeIndex>,
    right: Option<NodeIndex>,
    parent: Option<NodeIndex>,
}

#[derive(Clone, Copy, Debug)]
enum ChildIndex {
    Root,
    Node { parent: NodeIndex, is_left: bool },
}

impl<S: SizeType> Treap<S> {
    pub const fn new() -> Self {
        Self {
            nodes: Slab::new(),
            root: None,
        }
    }

    pub fn insert(&mut self, node: Node<S>) -> Option<Node<S>> {
        let mut replaced = None;

        let i = match self.binary_search(node.0) {
            Ok(replace) => {
                replaced = self.remove_at(replace);
                assert_eq!(self.insert(node), None);
                return replaced;
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
            let node = &self.nodes[i.as_usize()];

            if node.inner.2 <= self.nodes[parent_i.as_usize()].inner.2 {
                break;
            }

            self.rotate(i, parent_i);
        }

        replaced
    }

    pub fn remove(&mut self, node: Node<S>) -> Option<Node<S>> {
        let i = self.binary_search(node.0).ok()?;

        if self.nodes[i.as_usize()].inner != node {
            return None;
        }

        self.remove_at(i)
    }

    pub fn remove_at(&mut self, i: NodeIndex) -> Option<Node<S>> {
        loop {
            let TreapNode { left, right, .. } = self.nodes[i.as_usize()];

            let child_i = match (left, right) {
                (Some(left), Some(right)) => {
                    let left_prio = self.nodes[left.as_usize()].inner.2;
                    let right_prio = self.nodes[right.as_usize()].inner.2;
                    if left_prio >= right_prio { left } else { right }
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

    pub fn first_fit(&self, size: AlignedSize<S>) -> Option<&Node<S>> {
        let mut i = self.root?;
        loop {
            let node = &self.nodes[i.as_usize()];

            match size.cmp(&node.inner.0) {
                Ordering::Less | Ordering::Equal => break Some(&node.inner),
                Ordering::Greater => i = node.right?,
            }
        }
    }

    fn binary_search(&self, size: AlignedSize<S>) -> Result<NodeIndex, ChildIndex> {
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

        let grandparent = parent.parent;

        node.parent = grandparent;
        parent.parent = Some(child_i);

        let reparented = if Some(child_i) == parent.left {
            // rotate right
            parent.left = node.right;
            node.right.replace(parent_i)
        } else {
            // rotate left
            parent.right = node.left;
            node.left.replace(parent_i)
        };

        if let Some(reparented) = reparented {
            self.nodes[reparented.as_usize()].parent = Some(parent_i);
        }

        if let Some(grandparent) = grandparent {
            let grandparent = &mut self.nodes[grandparent.as_usize()];

            if Some(parent_i) == grandparent.left {
                grandparent.left = Some(child_i);
            } else {
                grandparent.right = Some(child_i);
            }
        } else {
            self.root = Some(child_i);
        }
    }

    #[allow(unused)]
    fn assert_treap_properties(&self) {
        for (i, node) in self.nodes.iter() {
            let child = NodeIndex::try_from(i).unwrap();
            if let Some(parent) = node.parent {
                self.assert_parent_child(parent, child);
            } else {
                assert_eq!(self.root.unwrap().as_usize(), i);
            }

            if let Some(left_child) = node.left {
                self.assert_parent_child(child, left_child);
            }

            if let Some(right_child) = node.right {
                self.assert_parent_child(child, right_child);
            }
        }
    }

    fn assert_parent_child(&self, parent_i: NodeIndex, child_i: NodeIndex) {
        assert_ne!(parent_i, child_i);

        let child = &self.nodes[child_i.as_usize()];

        assert_eq!(child.parent, Some(parent_i));

        let parent = &self.nodes[parent_i.as_usize()];

        assert!(child.inner.2 < parent.inner.2);

        if Some(child_i) == parent.left {
            assert!(child.inner.0 < parent.inner.0);
        }

        if Some(child_i) == parent.right {
            assert!(child.inner.0 >= parent.inner.0);
        }
    }
}

impl<S: SizeType> TreapNode<S> {
    const fn new(inner: Node<S>) -> Self {
        Self {
            inner,
            left: None,
            right: None,
            parent: None,
        }
    }
}
