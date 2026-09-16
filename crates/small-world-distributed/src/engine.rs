use crate::model::{
    BfsMethod, BfsProgress, ExperimentConfig, ExperimentResult, Phase, ProgressEvent,
    ProgressReporter,
};
use crate::owner;
use crate::process::{WorkerClient, request_all};
use crate::protocol::{ClusterConfig, WorkerCommand, WorkerResponse};
use crate::sampling::sample_sources;
use futures::future::{join_all, try_join_all};
use std::time::Instant;

const CLUSTERING_SAMPLE_LIMIT: usize = 20_000;

fn source_progress(visited: u64, nodes: u32, complete: bool) -> f64 {
    if complete {
        return 1.0;
    }
    visited.saturating_sub(1) as f64 / f64::from(nodes - 1)
}

fn begin_phase(reporter: &impl ProgressReporter, phase: Phase) {
    reporter.report(ProgressEvent::PhaseStarted(phase));
}

fn update_phase(
    reporter: &impl ProgressReporter,
    phase: Phase,
    progress: f64,
    detail: impl Into<String>,
    started: Instant,
) {
    reporter.report(ProgressEvent::PhaseProgress {
        phase,
        progress: progress.clamp(0.0, 1.0),
        detail: detail.into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
}

fn complete_phase(reporter: &impl ProgressReporter, phase: Phase, started: Instant) {
    reporter.report(ProgressEvent::PhaseCompleted {
        phase,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
}

fn update_bfs(reporter: &impl ProgressReporter, bfs: BfsProgress, started: Instant) {
    let traversal = bfs.level.map_or_else(
        || "exact ring distances".to_owned(),
        |level| format!("level {level}"),
    );
    update_phase(
        reporter,
        Phase::Bfs,
        (f64::from(bfs.source_index - 1) + bfs.source_progress) / f64::from(bfs.source_total),
        format!(
            "Source {} of {}; {traversal}; {} / {} vertices reached",
            bfs.source_index, bfs.source_total, bfs.visited_nodes, bfs.total_nodes
        ),
        started,
    );
    reporter.report(ProgressEvent::Bfs(bfs));
}

pub async fn run(
    config: ExperimentConfig,
    reporter: &impl ProgressReporter,
) -> Result<ExperimentResult, String> {
    let config = config.validate()?;
    let run_started = Instant::now();
    let cluster_config = ClusterConfig {
        nodes: config.nodes,
        degree: config.degree,
        workers: config.workers,
        seed: config.seed,
    };
    let mut workers = try_join_all((0..config.workers as usize).map(WorkerClient::spawn)).await?;
    reporter.report(ProgressEvent::WorkersStarted(config.workers));

    begin_phase(reporter, Phase::Build);
    let phase_started = Instant::now();
    let build_responses = request_all(
        &mut workers,
        (0..config.workers)
            .map(|_| WorkerCommand::Build(cluster_config))
            .collect(),
    )
    .await?;
    let mut built_nodes = 0u64;
    for response in build_responses {
        let WorkerResponse::Built { local_nodes, .. } = response else {
            return Err("unexpected build response".to_owned());
        };
        built_nodes += u64::from(local_nodes);
    }
    if built_nodes != u64::from(config.nodes) {
        return Err(format!(
            "workers built {built_nodes} nodes, expected {}",
            config.nodes
        ));
    }
    update_phase(
        reporter,
        Phase::Build,
        1.0,
        format!(
            "{built_nodes} vertices stored across {} processes",
            config.workers
        ),
        phase_started,
    );
    complete_phase(reporter, Phase::Build, phase_started);

    begin_phase(reporter, Phase::Rewire);
    let phase_started = Instant::now();
    let mut rewired_edges = 0u64;
    let mut cross_shard_messages = 0u64;
    for source_worker in 0..workers.len() {
        workers[source_worker]
            .send(WorkerCommand::Rewire {
                probability: config.probability,
                seed: config.seed ^ 0x736f_6d65_7073_6575,
            })
            .await?;
        loop {
            match workers[source_worker].receive().await? {
                WorkerResponse::RewireMutations { target, mutations } => {
                    let target = target as usize;
                    if target == source_worker || target >= workers.len() {
                        return Err("invalid rewiring batch target".to_owned());
                    }
                    let expected = mutations.len() as u64;
                    let response = workers[target]
                        .request(WorkerCommand::ApplyMutations(mutations))
                        .await?;
                    if !matches!(response, WorkerResponse::MutationsApplied { count } if count == expected)
                    {
                        return Err("unexpected mutation application response".to_owned());
                    }
                    cross_shard_messages += expected;
                }
                WorkerResponse::Rewired { rewired, .. } => {
                    rewired_edges += rewired;
                    break;
                }
                _ => return Err("unexpected rewiring response".to_owned()),
            }
        }
        update_phase(
            reporter,
            Phase::Rewire,
            (source_worker + 1) as f64 / workers.len() as f64,
            format!("{rewired_edges} edges rewired; endpoint updates routed"),
            phase_started,
        );
    }

    let stats = request_all(
        &mut workers,
        (0..config.workers)
            .map(|_| WorkerCommand::GraphStats)
            .collect(),
    )
    .await?;
    let mut adjacency_entries = 0u64;
    for response in stats {
        let WorkerResponse::GraphStats {
            adjacency_entries: entries,
            ..
        } = response
        else {
            return Err("unexpected graph stats response".to_owned());
        };
        adjacency_entries += entries;
    }
    let expected_entries = u64::from(config.nodes) * u64::from(config.degree);
    if adjacency_entries != expected_entries {
        return Err(format!(
            "edge-count invariant failed: {adjacency_entries} adjacency entries, expected {expected_entries}"
        ));
    }
    complete_phase(reporter, Phase::Rewire, phase_started);

    begin_phase(reporter, Phase::Clustering);
    let phase_started = Instant::now();
    let clustering_count = CLUSTERING_SAMPLE_LIMIT.min(config.nodes as usize);
    let clustering_vertices = sample_sources(
        config.nodes as usize,
        clustering_count,
        config.seed ^ 0x646f_7261_6e64_6f6d,
    );
    let mut vertices_by_owner = vec![Vec::new(); workers.len()];
    for vertex in clustering_vertices {
        let vertex = vertex as u32;
        vertices_by_owner[owner(vertex, config.nodes, config.workers)].push(vertex);
    }
    let mut clustering_sum = 0.0f64;
    let mut clustering_seen = 0u64;

    for origin in 0..workers.len() {
        let local_vertices = std::mem::take(&mut vertices_by_owner[origin]);
        if local_vertices.is_empty() {
            continue;
        }
        let mut triangle_counts = vec![0u64; local_vertices.len()];
        workers[origin]
            .send(WorkerCommand::PrepareClustering {
                vertices: local_vertices,
            })
            .await?;
        let degrees = loop {
            match workers[origin].receive().await? {
                WorkerResponse::ClusteringQueries { target, queries } => {
                    let target = target as usize;
                    if target == origin || target >= workers.len() {
                        return Err("invalid clustering batch target".to_owned());
                    }
                    cross_shard_messages += queries.len() as u64;
                    let response = workers[target]
                        .request(WorkerCommand::ResolveEdgeQueries(queries))
                        .await?;
                    let WorkerResponse::EdgeQueriesResolved { counts } = response else {
                        return Err("unexpected edge-query response".to_owned());
                    };
                    for (index, count) in counts {
                        let total = triangle_counts
                            .get_mut(index as usize)
                            .ok_or_else(|| "invalid edge-query origin index".to_owned())?;
                        *total += count;
                    }
                }
                WorkerResponse::ClusteringPrepared {
                    degrees,
                    local_counts,
                } => {
                    if degrees.len() != triangle_counts.len() || local_counts.len() != degrees.len()
                    {
                        return Err("clustering summary count mismatch".to_owned());
                    }
                    for (total, local) in triangle_counts.iter_mut().zip(local_counts) {
                        *total += local;
                    }
                    break degrees;
                }
                _ => return Err("unexpected clustering preparation response".to_owned()),
            }
        };
        for (degree, triangle_count) in degrees.into_iter().zip(triangle_counts) {
            if degree >= 2 {
                clustering_sum +=
                    2.0 * triangle_count as f64 / (f64::from(degree) * f64::from(degree - 1));
            }
            clustering_seen += 1;
        }
        update_phase(
            reporter,
            Phase::Clustering,
            clustering_seen as f64 / clustering_count as f64,
            format!("{clustering_seen} sampled vertices resolved"),
            phase_started,
        );
    }
    let clustering_coefficient = clustering_sum / clustering_seen as f64;
    complete_phase(reporter, Phase::Clustering, phase_started);

    begin_phase(reporter, Phase::Bfs);
    let phase_started = Instant::now();
    let sources = sample_sources(
        config.nodes as usize,
        config.bfs_samples as usize,
        config.seed ^ 0x6c79_6765_6e65_7261,
    );
    let mut total_distance = 0u64;
    let mut total_reachable = 0u64;
    let mut vertices_visited = 0u64;

    for (source_index, source) in sources.into_iter().enumerate() {
        if config.probability == 0.0 {
            let responses = request_all(
                &mut workers,
                (0..config.workers)
                    .map(|_| WorkerCommand::RingDistances {
                        source: source as u32,
                    })
                    .collect(),
            )
            .await?;
            let mut visited = 0u64;
            for response in responses {
                let WorkerResponse::RingDistances {
                    distance_sum,
                    reachable,
                    visited: local_visited,
                } = response
                else {
                    return Err("unexpected ring distance response".to_owned());
                };
                total_distance += distance_sum;
                total_reachable += reachable;
                visited += local_visited;
            }
            vertices_visited += visited;
            update_bfs(
                reporter,
                BfsProgress {
                    source_index: source_index as u32 + 1,
                    source_total: config.bfs_samples,
                    level: None,
                    frontier_by_worker: vec![0; workers.len()],
                    visited_nodes: visited,
                    total_nodes: config.nodes,
                    source_progress: 1.0,
                    method: BfsMethod::Ring,
                },
                phase_started,
            );
            continue;
        }
        request_all(
            &mut workers,
            (0..config.workers)
                .map(|_| WorkerCommand::StartBfs {
                    source: source as u32,
                })
                .collect(),
        )
        .await?;
        let mut level = 0u32;
        loop {
            let responses = request_all(
                &mut workers,
                (0..config.workers)
                    .map(|_| WorkerCommand::ExpandBfs)
                    .collect(),
            )
            .await?;
            let mut routed = vec![Vec::<u32>::new(); workers.len()];
            let mut newly_discovered = 0u64;
            for response in responses {
                let WorkerResponse::BfsExpanded {
                    local_discovered,
                    discoveries_by_owner,
                } = response
                else {
                    return Err("unexpected BFS expansion response".to_owned());
                };
                newly_discovered += local_discovered;
                for (target, discoveries) in discoveries_by_owner.into_iter().enumerate() {
                    routed[target].extend(discoveries);
                }
            }
            for discoveries in &mut routed {
                discoveries.sort_unstable();
                discoveries.dedup();
            }
            cross_shard_messages += routed.iter().map(|batch| batch.len() as u64).sum::<u64>();
            let responses = request_all(
                &mut workers,
                routed
                    .into_iter()
                    .map(WorkerCommand::FinishBfsLevel)
                    .collect(),
            )
            .await?;
            let mut frontier_by_worker = Vec::with_capacity(workers.len());
            let mut visited_this_source = 0u64;
            for response in responses {
                let WorkerResponse::BfsLevelFinished {
                    accepted,
                    frontier,
                    visited,
                } = response
                else {
                    return Err("unexpected BFS level response".to_owned());
                };
                newly_discovered += accepted;
                frontier_by_worker.push(frontier);
                visited_this_source += visited;
            }
            total_distance += newly_discovered * u64::from(level + 1);
            total_reachable += newly_discovered;
            let complete = frontier_by_worker.iter().sum::<u64>() == 0;
            update_bfs(
                reporter,
                BfsProgress {
                    source_index: source_index as u32 + 1,
                    source_total: config.bfs_samples,
                    level: Some(level + 1),
                    frontier_by_worker,
                    visited_nodes: visited_this_source,
                    total_nodes: config.nodes,
                    source_progress: source_progress(visited_this_source, config.nodes, complete),
                    method: BfsMethod::Traversal,
                },
                phase_started,
            );
            if complete {
                vertices_visited += visited_this_source;
                break;
            }
            level += 1;
        }
    }
    complete_phase(reporter, Phase::Bfs, phase_started);

    let average_path_length = total_distance as f64 / total_reachable as f64;
    let results = ExperimentResult {
        average_path_length,
        clustering_coefficient,
        elapsed_ms: run_started.elapsed().as_millis() as u64,
        cross_shard_messages,
        vertices_visited,
        rewired_edges,
        clustering_samples: clustering_seen as u32,
        adjacency_entries,
    };
    join_all(workers.into_iter().map(WorkerClient::shutdown)).await;
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::source_progress;
    use crate::{ExperimentConfig, system_worker_limit};

    #[test]
    fn progress_advances_within_a_source_and_completes_disconnected_sources() {
        assert_eq!(source_progress(1, 101, false), 0.0);
        assert_eq!(source_progress(51, 101, false), 0.5);
        assert_eq!(source_progress(101, 101, true), 1.0);
        assert_eq!(source_progress(20, 101, true), 1.0);
    }

    #[test]
    fn accepts_node_counts_above_the_previous_demo_cap() {
        let config = ExperimentConfig {
            nodes: 5_000_001,
            workers: 1,
            ..ExperimentConfig::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn worker_count_is_limited_by_available_parallelism() {
        let limit = system_worker_limit();
        let valid = ExperimentConfig {
            workers: limit,
            ..ExperimentConfig::default()
        };
        assert!(valid.validate().is_ok());

        if let Some(too_many) = limit.checked_add(1) {
            let invalid = ExperimentConfig {
                workers: too_many,
                ..ExperimentConfig::default()
            };
            assert!(invalid.validate().is_err());
        }
    }
}
