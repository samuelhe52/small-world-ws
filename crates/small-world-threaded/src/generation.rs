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

pub(crate) fn choose_target(
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
use crate::{Graph, GraphError, RewireStats};
use rand::{Rng, SeedableRng, seq::index::sample};
use rand_chacha::ChaCha8Rng;
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::collections::HashSet;
