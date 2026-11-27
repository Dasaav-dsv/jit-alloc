use core::fmt;

use crate::{
    AlignedOffset, AlignedSize, JitAlloc, LargeShard, NodeIndex, Shard, ShardIndex, SizeType,
    node::Node, num::Unsigned,
};

#[derive(Clone, Copy, Debug)]
pub struct MemoryReport {
    pub executable: usize,
    pub executable_used: usize,
    pub overhead: usize,
    pub overhead_used: usize,
}

impl<S: SizeType, const ALIGN: usize> JitAlloc<S, ALIGN> {
    pub fn memory_report(&self) -> MemoryReport {
        let mut overhead = self.shards_by_rx.len() * size_of::<(usize, ShardIndex)>();
        let mut overhead_used = overhead;

        overhead += self.large_shards_by_rx.capacity() * size_of::<(usize, LargeShard)>();
        overhead_used += self.large_shards_by_rx.len() * size_of::<(usize, LargeShard)>();

        overhead += self.nodes.freelist.nodes.capacity() * size_of::<(AlignedSize<S>, NodeIndex)>();
        overhead_used += self.nodes.freelist.nodes.len() * size_of::<(AlignedSize<S>, NodeIndex)>();

        overhead += self.nodes.shards.capacity() * size_of::<Shard<S>>();
        overhead_used += self.nodes.shards.len() * size_of::<Shard<S>>();

        overhead += self.nodes.nodes.capacity() * size_of::<Node<S>>();
        overhead_used += self.nodes.nodes.len() * size_of::<Node<S>>();

        let (mut executable, mut executable_used) =
            self.shards_by_rx
                .values()
                .fold((0, 0), |(total, used), shard_index| {
                    let shard = self.nodes.shard(*shard_index);

                    overhead +=
                        shard.allocated.capacity() * size_of::<(AlignedOffset<S>, NodeIndex)>();
                    overhead_used +=
                        shard.allocated.len() * size_of::<(AlignedOffset<S>, NodeIndex)>();

                    let total = total + shard.size;
                    let used = used
                        + shard
                            .allocated
                            .values()
                            .map(|node_index| self.nodes[*node_index].size().as_wide().as_usize())
                            .sum::<usize>();

                    (total, used)
                });

        for shard in self.large_shards_by_rx.values() {
            executable += shard.size;
            executable_used += shard.size;
        }

        MemoryReport {
            executable,
            executable_used,
            overhead,
            overhead_used,
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
