//! Shared-memory Watts-Strogatz generation and analysis using Rayon.
//!
//! This crate owns a complete graph in one process. Logical partitions model
//! ownership boundaries for generation statistics; they are not OS processes.

mod generation;
mod graph;
mod metrics;

pub use generation::{generate_ws_partitioned, sample_sources};
pub use graph::{Graph, GraphError, PathEstimate, RewireStats};
pub use metrics::{
    average_clustering_parallel, exact_path_length, sampled_path_length_parallel,
    sampled_path_length_sequential,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::choose_target;
    use crate::metrics::local_clustering;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;
    use std::collections::HashSet;

    #[cfg(feature = "parallel")]
    #[test]
    fn batched_generation_is_independent_of_thread_count() {
        let generate = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| generate_ws_partitioned(15_000, 10, 0.4, 4, 42).unwrap())
        };
        let (sequential, a) = generate(1);
        let (parallel, b) = generate(4);
        assert_eq!(sequential.adjacency, parallel.adjacency);
        assert_eq!(a.rewired_edges, b.rewired_edges);
        parallel.validate().unwrap();
    }

    #[test]
    fn bfs_handles_disconnected_and_repeated_sources() {
        let graph = Graph {
            adjacency: vec![vec![1], vec![0, 2], vec![1], vec![]],
            edge_count: 2,
        };
        let sources = [0, 3, 1, 0];
        for estimate in [
            sampled_path_length_sequential(&graph, &sources),
            sampled_path_length_parallel(&graph, &sources),
        ] {
            assert_eq!(estimate.reachable_pairs, 6);
            assert_eq!(estimate.mean_distance, 8.0 / 6.0);
        }
        assert_eq!(sampled_path_length_parallel(&graph, &[]).reachable_pairs, 0);
    }

    #[test]
    fn clustering_matches_pairwise_reference_on_irregular_graphs() {
        for (n, k, p) in [(101, 2, 1.0), (200, 30, 0.4), (32, 30, 1.0)] {
            let (graph, _) = generate_ws_partitioned(n, k, p, 3, 17).unwrap();
            graph.validate().unwrap();
            for u in 0..n {
                let neighbors = graph.neighbors(u);
                let links: usize = neighbors
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        neighbors[i + 1..]
                            .iter()
                            .filter(|&&w| graph.has_edge(v, w))
                            .count()
                    })
                    .sum();
                let expected = if neighbors.len() < 2 {
                    0.0
                } else {
                    2.0 * links as f64 / (neighbors.len() * (neighbors.len() - 1)) as f64
                };
                assert_eq!(local_clustering(&graph, u), expected);
            }
        }
    }

    #[test]
    fn rewiring_can_use_the_last_non_neighbor() {
        let adjacency = vec![
            HashSet::from([1, 2]),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
        ];
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        assert_eq!(choose_target(&mut rng, 0, 1, &adjacency), Some(3));
        let (graph, stats) = generate_ws_partitioned(4, 2, 1.0, 2, 7).unwrap();
        assert!(stats.rewired_edges > 0);
        graph.validate().unwrap();
    }

    #[test]
    fn ring_lattice_matches_stage_one_example() {
        let (graph, stats) = generate_ws_partitioned(20, 4, 0.0, 4, 7).unwrap();
        assert_eq!(graph.neighbors(0), &[1, 2, 18, 19]);
        assert_eq!(graph.edge_count(), 40);
        assert_eq!(stats.rewired_edges, 0);
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn rewiring_preserves_simple_undirected_graph() {
        let (graph, stats) = generate_ws_partitioned(500, 10, 1.0, 4, 42).unwrap();
        assert_eq!(graph.edge_count(), 2_500);
        assert_eq!(stats.rewired_edges, 2_500);
        assert!(stats.cross_partition_updates > 0);
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn parallel_and_sequential_bfs_agree() {
        let (graph, _) = generate_ws_partitioned(800, 10, 0.05, 4, 11).unwrap();
        let sources = sample_sources(800, 40, 12);
        let sequential = sampled_path_length_sequential(&graph, &sources);
        let parallel = sampled_path_length_parallel(&graph, &sources);
        assert_eq!(sequential.reachable_pairs, parallel.reachable_pairs);
        assert_eq!(sequential.mean_distance, parallel.mean_distance);
    }

    #[test]
    fn sampling_is_within_five_percent_on_fixed_small_graph() {
        let (graph, _) = generate_ws_partitioned(400, 10, 0.05, 4, 21).unwrap();
        let exact = exact_path_length(&graph).mean_distance;
        let sources = sample_sources(400, 100, 22);
        let approximate = sampled_path_length_sequential(&graph, &sources).mean_distance;
        let relative_error = (approximate - exact).abs() / exact;
        assert!(relative_error < 0.05, "relative error was {relative_error}");
    }

    #[test]
    fn regular_k4_clustering_is_about_one_half() {
        let (graph, _) = generate_ws_partitioned(500, 4, 0.0, 2, 1).unwrap();
        let clustering = average_clustering_parallel(&graph);
        assert!((clustering - 0.5).abs() < 1e-12);
    }
}
