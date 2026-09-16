pub mod coordinator;
pub mod protocol;
pub mod worker;

pub(crate) fn owner(node: u32, nodes: u32, workers: u32) -> usize {
    debug_assert!(node < nodes);
    debug_assert!(workers > 0);
    (((u64::from(node) + 1) * u64::from(workers) - 1) / u64::from(nodes)) as usize
}

#[cfg(test)]
mod tests {
    use super::owner;

    #[test]
    fn ownership_matches_floor_partition_boundaries() {
        for nodes in 1_u32..=257 {
            for workers in 1_u32..=nodes.min(16) {
                for worker in 0..workers {
                    let start = (u64::from(nodes) * u64::from(worker) / u64::from(workers)) as u32;
                    let end =
                        (u64::from(nodes) * u64::from(worker + 1) / u64::from(workers)) as u32;
                    for node in start..end {
                        assert_eq!(owner(node, nodes, workers), worker as usize);
                    }
                }
            }
        }
    }

    #[test]
    fn routes_the_reported_ten_million_node_boundary_correctly() {
        assert_eq!(owner(8_333_332, 10_000_000, 6), 4);
        assert_eq!(owner(8_333_333, 10_000_000, 6), 5);
    }
}
