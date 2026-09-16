use rand::{Rng, SeedableRng, seq::index::sample};
use rand_chacha::ChaCha8Rng;
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::collections::{HashSet, VecDeque};
use std::fmt;

#[derive(Debug, Clone)]
pub struct Graph {
    adjacency: Vec<Vec<usize>>,
    edge_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct RewireStats {
    pub considered_edges: usize,
    pub rewired_edges: usize,
    pub cross_partition_updates: usize,
    pub partitions: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PathEstimate {
    pub mean_distance: f64,
    pub reachable_pairs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    InvalidNodeCount,
    InvalidDegree,
    InvalidProbability,
    InvalidPartitionCount,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNodeCount => write!(f, "N must be at least 3"),
            Self::InvalidDegree => write!(f, "K must be even and satisfy 2 <= K < N"),
            Self::InvalidProbability => write!(f, "p must be in [0, 1]"),
            Self::InvalidPartitionCount => write!(f, "partitions must be at least 1"),
        }
    }
}

impl std::error::Error for GraphError {}

impl Graph {
    pub fn node_count(&self) -> usize {
        self.adjacency.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub fn degree(&self, node: usize) -> usize {
        self.adjacency[node].len()
    }

    pub fn neighbors(&self, node: usize) -> &[usize] {
        &self.adjacency[node]
    }

    pub fn has_edge(&self, u: usize, v: usize) -> bool {
        self.adjacency[u].binary_search(&v).is_ok()
    }

    pub fn validate(&self) -> Result<(), String> {
        let n = self.node_count();
        let mut degree_sum = 0usize;

        for (u, neighbors) in self.adjacency.iter().enumerate() {
            if neighbors.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(format!("adjacency list {u} is not strictly sorted"));
            }
            for &v in neighbors {
                if v >= n {
                    return Err(format!("edge ({u}, {v}) has an invalid endpoint"));
                }
                if u == v {
                    return Err(format!("self-loop at node {u}"));
                }
                if !self.has_edge(v, u) {
                    return Err(format!("edge ({u}, {v}) is not symmetric"));
                }
            }
            degree_sum += neighbors.len();
        }

        if !degree_sum.is_multiple_of(2) || degree_sum / 2 != self.edge_count {
            return Err("stored edge count does not match adjacency lists".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct EdgeTask {
    index: usize,
    u: usize,
    v: usize,
}

#[derive(Debug, Clone, Copy)]
struct Proposal {
    task: EdgeTask,
    should_rewire: bool,
    target: Option<usize>,
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn owner(node: usize, n: usize, partitions: usize) -> usize {
    node.saturating_mul(partitions) / n
}

fn choose_target(
    rng: &mut ChaCha8Rng,
    u: usize,
    old_v: usize,
    adjacency: &[HashSet<usize>],
) -> Option<usize> {
    let n = adjacency.len();
    if adjacency[u].len() + 1 >= n {
        return None;
    }

    for _ in 0..64 {
        let candidate = rng.gen_range(0..n);
        if candidate != u && candidate != old_v && !adjacency[u].contains(&candidate) {
            return Some(candidate);
        }
    }

    (0..n).find(|&candidate| {
        candidate != u && candidate != old_v && !adjacency[u].contains(&candidate)
    })
}

fn propose_rewire(task: EdgeTask, probability: f64, seed: u64, n: usize, k: usize) -> Proposal {
    let mut rng = ChaCha8Rng::seed_from_u64(splitmix64(seed ^ task.index as u64));
    let should_rewire = rng.gen_bool(probability);
    // Proposals always read the original ring, even when earlier batches have
    // already committed. This preserves canonical two-phase generation.
    let eligible = |candidate: usize| {
        let separation = task.u.abs_diff(candidate);
        separation.min(n - separation) > k / 2
    };
    let target = if should_rewire && k + 1 < n {
        (0..64)
            .map(|_| rng.gen_range(0..n))
            .find(|&v| eligible(v))
            .or_else(|| (0..n).find(|&v| eligible(v)))
    } else {
        None
    };
    Proposal {
        task,
        should_rewire,
        target,
    }
}

fn sort_adjacency(adjacency: Vec<HashSet<usize>>) -> Vec<Vec<usize>> {
    #[cfg(feature = "parallel")]
    {
        adjacency
            .into_par_iter()
            .map(|neighbors| {
                let mut neighbors: Vec<_> = neighbors.into_iter().collect();
                neighbors.sort_unstable();
                neighbors
            })
            .collect()
    }

    #[cfg(not(feature = "parallel"))]
    {
        adjacency
            .into_iter()
            .map(|neighbors| {
                let mut neighbors: Vec<_> = neighbors.into_iter().collect();
                neighbors.sort_unstable();
                neighbors
            })
            .collect()
    }
}

/// Generates a Watts-Strogatz graph through a two-phase, partition-aware model.
/// Logical partitions produce independent proposals in parallel. A coordinator
/// commits them in canonical edge order so loop, duplicate, and edge-count
/// invariants remain deterministic.
pub fn generate_ws_partitioned(
    n: usize,
    k: usize,
    p: f64,
    partitions: usize,
    seed: u64,
) -> Result<(Graph, RewireStats), GraphError> {
    if n < 3 {
        return Err(GraphError::InvalidNodeCount);
    }
    if k < 2 || k >= n || !k.is_multiple_of(2) {
        return Err(GraphError::InvalidDegree);
    }
    if !(0.0..=1.0).contains(&p) {
        return Err(GraphError::InvalidProbability);
    }
    if partitions == 0 {
        return Err(GraphError::InvalidPartitionCount);
    }

    let expected_edges = n * k / 2;
    let mut adjacency = vec![HashSet::with_capacity(k + 2); n];

    for u in 0..n {
        for offset in 1..=k / 2 {
            let v = (u + offset) % n;
            adjacency[u].insert(v);
            adjacency[v].insert(u);
        }
    }

    let mut rewired_edges = 0usize;
    let mut cross_partition_updates = 0usize;

    // Bound temporary proposal storage independently of the number of edges.
    const PROPOSAL_BATCH: usize = 65_536;
    for start in (0..if p == 0.0 { 0 } else { expected_edges }).step_by(PROPOSAL_BATCH) {
        let end = (start + PROPOSAL_BATCH).min(expected_edges);
        let propose = |index| {
            let u = index / (k / 2);
            let v = (u + index % (k / 2) + 1) % n;
            propose_rewire(EdgeTask { index, u, v }, p, seed, n, k)
        };
        #[cfg(feature = "parallel")]
        let proposals: Vec<_> = (start..end).into_par_iter().map(propose).collect();
        #[cfg(not(feature = "parallel"))]
        let proposals: Vec<_> = (start..end).map(propose).collect();
        for proposal in proposals {
            if !proposal.should_rewire {
                continue;
            }

            let EdgeTask { index, u, v } = proposal.task;
            let valid_proposal = proposal
                .target
                .filter(|&w| w != u && w != v && !adjacency[u].contains(&w));
            let w = if let Some(w) = valid_proposal {
                w
            } else {
                let mut fallback_rng =
                    ChaCha8Rng::seed_from_u64(splitmix64(seed ^ (index as u64) ^ 0xa5a5_a5a5));
                match choose_target(&mut fallback_rng, u, v, &adjacency) {
                    Some(w) => w,
                    None => continue,
                }
            };

            let removed_forward = adjacency[u].remove(&v);
            let removed_reverse = adjacency[v].remove(&u);
            let inserted_forward = adjacency[u].insert(w);
            let inserted_reverse = adjacency[w].insert(u);
            debug_assert!(removed_forward && removed_reverse);
            debug_assert!(inserted_forward && inserted_reverse);
            rewired_edges += 1;

            let source_owner = owner(u, n, partitions);
            cross_partition_updates += usize::from(owner(v, n, partitions) != source_owner);
            cross_partition_updates += usize::from(owner(w, n, partitions) != source_owner);
        }
    }
    let adjacency = sort_adjacency(adjacency);

    let graph = Graph {
        adjacency,
        edge_count: expected_edges,
    };
    let stats = RewireStats {
        considered_edges: expected_edges,
        rewired_edges,
        cross_partition_updates,
        partitions,
    };
    Ok((graph, stats))
}

pub fn sample_sources(n: usize, count: usize, seed: u64) -> Vec<usize> {
    let actual = count.min(n);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    sample(&mut rng, n, actual).into_vec()
}

fn bfs_totals(graph: &Graph, source: usize) -> (u64, u64) {
    let n = graph.node_count();
    let mut distances = vec![u32::MAX; n];
    let mut queue = VecDeque::with_capacity(n.min(4096));
    distances[source] = 0;
    queue.push_back(source);

    let mut distance_sum = 0u64;
    let mut reachable = 0u64;
    while let Some(u) = queue.pop_front() {
        let next_distance = distances[u] + 1;
        for &v in graph.neighbors(u) {
            if distances[v] == u32::MAX {
                distances[v] = next_distance;
                distance_sum += u64::from(next_distance);
                reachable += 1;
                queue.push_back(v);
            }
        }
    }
    (distance_sum, reachable)
}

fn finish_path_estimate((distance_sum, reachable_pairs): (u64, u64)) -> PathEstimate {
    PathEstimate {
        mean_distance: if reachable_pairs == 0 {
            0.0
        } else {
            distance_sum as f64 / reachable_pairs as f64
        },
        reachable_pairs,
    }
}

pub fn sampled_path_length_sequential(graph: &Graph, sources: &[usize]) -> PathEstimate {
    finish_path_estimate(
        sources
            .iter()
            .map(|&source| bfs_totals(graph, source))
            .fold((0u64, 0u64), |a, b| (a.0 + b.0, a.1 + b.1)),
    )
}

pub fn sampled_path_length_parallel(graph: &Graph, sources: &[usize]) -> PathEstimate {
    #[cfg(feature = "parallel")]
    {
        finish_path_estimate(
            sources
                .par_iter()
                .map(|&source| bfs_totals(graph, source))
                .reduce(|| (0u64, 0u64), |a, b| (a.0 + b.0, a.1 + b.1)),
        )
    }

    #[cfg(not(feature = "parallel"))]
    {
        sampled_path_length_sequential(graph, sources)
    }
}

pub fn exact_path_length(graph: &Graph) -> PathEstimate {
    let sources: Vec<_> = (0..graph.node_count()).collect();
    sampled_path_length_sequential(graph, &sources)
}

fn local_clustering(graph: &Graph, u: usize) -> f64 {
    let neighbors = graph.neighbors(u);
    if neighbors.len() < 2 {
        return 0.0;
    }
    let mut links = 0usize;
    for (i, &v) in neighbors.iter().enumerate() {
        let tail = &neighbors[i + 1..];
        let adjacent = graph.neighbors(v);
        // Binary search wins for short tails; merging removes repeated searches
        // for larger neighborhoods without penalizing the default sparse graph.
        if tail.len() <= 16 {
            links += tail
                .iter()
                .filter(|&&w| adjacent.binary_search(&w).is_ok())
                .count();
            continue;
        }
        let mut j = adjacent.partition_point(|&w| w <= v);
        let mut t = 0;
        while t < tail.len() && j < adjacent.len() {
            match tail[t].cmp(&adjacent[j]) {
                std::cmp::Ordering::Less => t += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    links += 1;
                    t += 1;
                    j += 1;
                }
            }
        }
    }
    2.0 * links as f64 / (neighbors.len() * (neighbors.len() - 1)) as f64
}

pub fn average_clustering_parallel(graph: &Graph) -> f64 {
    if graph.node_count() == 0 {
        return 0.0;
    }

    #[cfg(feature = "parallel")]
    {
        (0..graph.node_count())
            .into_par_iter()
            .map(|u| local_clustering(graph, u))
            .sum::<f64>()
            / graph.node_count() as f64
    }

    #[cfg(not(feature = "parallel"))]
    {
        (0..graph.node_count())
            .map(|u| local_clustering(graph, u))
            .sum::<f64>()
            / graph.node_count() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
