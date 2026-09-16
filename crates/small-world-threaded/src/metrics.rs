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

pub(crate) fn local_clustering(graph: &Graph, u: usize) -> f64 {
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
use crate::{Graph, PathEstimate};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::collections::VecDeque;
