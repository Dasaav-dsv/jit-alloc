//! Allocator memory usage reporting.

use core::fmt;

use crate::{
    AlignedOffset, AlignedSize, JitAlloc, LargeShard, NodeIndex, Shard, ShardIndex, SizeType,
    node::Node, num::Unsigned,
};

/// A memory usage report.
///
/// Records approximate sizes of allocated executable regions and overhead (in bytes).
#[derive(Clone, Copy, Debug)]
pub struct MemoryReport {
    /// Executable memory that is actually used (in bytes).
    pub executable_used: usize,

    /// Total executable memory (in bytes).
    pub executable: usize,

    /// Metadata overhead that is actually used (in bytes).
    pub overhead_used: usize,

    /// Total overhead used by metadata (in bytes).
    pub overhead: usize,
}

impl<S: SizeType, const ALIGN: usize> JitAlloc<S, ALIGN> {
    /// Returns a [`MemoryReport`] that records memory usage.
    ///
    /// # Examples
    ///
    /// ```
    /// use jit_alloc::JitAlloc;
    ///
    /// fn allocated_memory_total(alloc: &JitAlloc<u16, 1>) -> (usize, usize) {
    ///     let report = alloc.memory_report();
    ///     let used = report.executable_used + report.overhead_used;
    ///     let total = report.executable + report.overhead;
    ///     (used, total)
    /// }
    /// ```
    pub fn memory_report(&self) -> MemoryReport {
        let mut overhead_used = self.shards_by_rx.len() * size_of::<(usize, ShardIndex)>();
        let mut overhead = overhead_used;

        overhead_used += self.large_shards_by_rx.len() * size_of::<(usize, LargeShard)>();
        overhead += self.large_shards_by_rx.capacity() * size_of::<(usize, LargeShard)>();

        overhead_used += self.nodes.freelist.nodes.len() * size_of::<(AlignedSize<S>, NodeIndex)>();
        overhead += self.nodes.freelist.nodes.capacity() * size_of::<(AlignedSize<S>, NodeIndex)>();

        overhead_used += self.nodes.shards.len() * size_of::<Shard<S>>();
        overhead += self.nodes.shards.capacity() * size_of::<Shard<S>>();

        overhead_used += self.nodes.nodes.len() * size_of::<Node<S>>();
        overhead += self.nodes.nodes.capacity() * size_of::<Node<S>>();

        let (mut executable_used, mut executable) =
            self.shards_by_rx
                .values()
                .fold((0, 0), |(used, total), shard_index| {
                    let shard = self.nodes.shard(*shard_index);

                    overhead_used +=
                        shard.allocated.len() * size_of::<(AlignedOffset<S>, NodeIndex)>();
                    overhead +=
                        shard.allocated.capacity() * size_of::<(AlignedOffset<S>, NodeIndex)>();

                    let used = used
                        + shard
                            .allocated
                            .values()
                            .map(|node_index| self.nodes[*node_index].size().as_wide().as_usize())
                            .sum::<usize>();

                    let total = total + shard.size;

                    (used, total)
                });

        for shard in self.large_shards_by_rx.values() {
            executable_used += shard.size;
            executable += shard.size;
        }

        MemoryReport {
            executable_used,
            executable,
            overhead_used,
            overhead,
        }
    }
}

impl fmt::Display for MemoryReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "used/total: {}/{} (executable: {}/{}; overhead: {}/{})",
            self.executable_used + self.overhead_used,
            self.executable + self.overhead,
            self.executable_used,
            self.executable,
            self.overhead_used,
            self.overhead,
        ))
    }
}
