use super::owner;
use super::protocol::{ClusterConfig, WorkerCommand, WorkerResponse, read_frame, write_frame};
use futures::future::{join_all, try_join_all};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::RwLock;

use small_world_core::sample_sources;

const CLUSTERING_SAMPLE_LIMIT: usize = 20_000;

pub fn system_worker_limit() -> u32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(u32::MAX as usize) as u32)
        .unwrap_or(1)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub nodes: u32,
    pub degree: u32,
    pub probability: f64,
    pub bfs_samples: u32,
    pub workers: u32,
    pub seed: u64,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            nodes: 1_000_000,
            degree: 10,
            probability: 0.05,
            bfs_samples: 32,
            workers: system_worker_limit().min(4),
            seed: 42,
        }
    }
}

impl RunConfig {
    pub fn validate(self) -> Result<Self, String> {
        if self.nodes < 10_000 {
            return Err("nodes must be at least 10,000".to_owned());
        }
        if self.degree < 2
            || self.degree >= self.nodes
            || !self.degree.is_multiple_of(2)
            || self.degree > 100
        {
            return Err("degree must be even and between 2 and 100".to_owned());
        }
        if !(0.0..=1.0).contains(&self.probability) {
            return Err("rewiring probability must be in [0, 1]".to_owned());
        }
        if !(1..=256).contains(&self.bfs_samples) {
            return Err("BFS samples must be between 1 and 256".to_owned());
        }
        let worker_limit = system_worker_limit();
        if self.workers == 0 || self.workers > worker_limit {
            return Err(format!(
                "worker processes must be between 1 and {worker_limit} on this system"
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseView {
    pub key: &'static str,
    pub label: &'static str,
    pub status: &'static str,
    pub progress: f64,
    pub detail: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BfsView {
    pub source_index: u32,
    pub source_total: u32,
    pub level: Option<u32>,
    pub frontier_by_worker: Vec<u64>,
    pub visited_nodes: u64,
    pub total_nodes: u32,
    pub source_progress: f64,
    pub method: &'static str,
}

fn source_progress(visited: u64, nodes: u32, complete: bool) -> f64 {
    if complete {
        return 1.0;
    }
    visited.saturating_sub(1) as f64 / f64::from(nodes - 1)
}

async fn update_bfs(state: &SharedDashboardState, bfs: BfsView, started: Instant) {
    let mut dashboard = state.write().await;
    if let Some(phase) = dashboard.phases.iter_mut().find(|phase| phase.key == "bfs") {
        phase.progress =
            (f64::from(bfs.source_index - 1) + bfs.source_progress) / f64::from(bfs.source_total);
        let traversal = bfs.level.map_or_else(
            || "exact ring distances".to_owned(),
            |level| format!("level {level}"),
        );
        phase.detail = format!(
            "Source {} of {}; {traversal}; {} / {} vertices reached",
            bfs.source_index, bfs.source_total, bfs.visited_nodes, bfs.total_nodes
        );
        phase.elapsed_ms = started.elapsed().as_millis() as u64;
    }
    dashboard.current_bfs = Some(bfs);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResults {
    pub average_path_length: f64,
    pub clustering_coefficient: f64,
    pub elapsed_ms: u64,
    pub cross_shard_messages: u64,
    pub vertices_visited: u64,
    pub rewired_edges: u64,
    pub clustering_samples: u32,
    pub adjacency_entries: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: u64,
    pub finished_at_ms: u64,
    pub config: RunConfig,
    pub results: RunResults,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardState {
    pub status: &'static str,
    pub workers_online: u32,
    pub worker_limit: u32,
    pub config: RunConfig,
    pub phases: Vec<PhaseView>,
    pub current_bfs: Option<BfsView>,
    pub results: Option<RunResults>,
    pub history: Vec<RunRecord>,
    pub error: Option<String>,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            status: "idle",
            workers_online: 0,
            worker_limit: system_worker_limit(),
            config: RunConfig::default(),
            phases: fresh_phases(),
            current_bfs: None,
            results: None,
            history: Vec::new(),
            error: None,
        }
    }
}

pub type SharedDashboardState = Arc<RwLock<DashboardState>>;

fn fresh_phases() -> Vec<PhaseView> {
    vec![
        PhaseView {
            key: "build",
            label: "Build shards",
            status: "pending",
            progress: 0.0,
            detail: "Partition nodes and construct local rings".to_owned(),
            elapsed_ms: 0,
        },
        PhaseView {
            key: "rewire",
            label: "Distributed rewiring",
            status: "pending",
            progress: 0.0,
            detail: "Route endpoint mutations to shard owners".to_owned(),
            elapsed_ms: 0,
        },
        PhaseView {
            key: "clustering",
            label: "Clustering coefficient",
            status: "pending",
            progress: 0.0,
            detail: "Resolve sampled triangle queries across shards".to_owned(),
            elapsed_ms: 0,
        },
        PhaseView {
            key: "bfs",
            label: "Sampled distributed BFS",
            status: "pending",
            progress: 0.0,
            detail: "Run level-synchronous BFS across all workers".to_owned(),
            elapsed_ms: 0,
        },
    ]
}

struct WorkerClient {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}

impl WorkerClient {
    async fn spawn(id: usize) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut child = Command::new(executable)
            .arg("worker")
            .arg("--id")
            .arg(id.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to spawn worker {id}: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| format!("worker {id} has no stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| format!("worker {id} has no stdout"))?;
        Ok(Self {
            child,
            input,
            output,
        })
    }

    async fn request(&mut self, command: WorkerCommand) -> Result<WorkerResponse, String> {
        self.send(command).await?;
        self.receive().await
    }

    // Streaming phases send intermediate batches before their final response.
    // Route batches only to other workers, keeping this worker's stream unread
    // by ordinary request/response calls until the summary has been consumed.
    async fn send(&mut self, command: WorkerCommand) -> Result<(), String> {
        write_frame(&mut self.input, &command)
            .await
            .map_err(|error| error.to_string())
    }

    async fn receive(&mut self) -> Result<WorkerResponse, String> {
        let response = read_frame(&mut self.output)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "worker closed its protocol stream".to_owned())?;
        match response {
            WorkerResponse::Error(error) => Err(error),
            response => Ok(response),
        }
    }

    async fn shutdown(mut self) {
        let _ = self.request(WorkerCommand::Shutdown).await;
        let _ = self.child.wait().await;
    }
}

async fn request_all(
    workers: &mut [WorkerClient],
    commands: Vec<WorkerCommand>,
) -> Result<Vec<WorkerResponse>, String> {
    if workers.len() != commands.len() {
        return Err("worker command count mismatch".to_owned());
    }
    try_join_all(
        workers
            .iter_mut()
            .zip(commands)
            .map(|(worker, command)| worker.request(command)),
    )
    .await
}

async fn begin_phase(state: &SharedDashboardState, key: &str) {
    let mut state = state.write().await;
    for phase in &mut state.phases {
        if phase.key == key {
            phase.status = "active";
            phase.progress = 0.0;
            phase.elapsed_ms = 0;
        }
    }
}

async fn update_phase(
    state: &SharedDashboardState,
    key: &str,
    progress: f64,
    detail: impl Into<String>,
    elapsed: Instant,
) {
    let detail = detail.into();
    let mut state = state.write().await;
    for phase in &mut state.phases {
        if phase.key == key {
            phase.progress = progress.clamp(0.0, 1.0);
            phase.detail = detail.clone();
            phase.elapsed_ms = elapsed.elapsed().as_millis() as u64;
        }
    }
}

async fn complete_phase(state: &SharedDashboardState, key: &str, elapsed: Instant) {
    let mut state = state.write().await;
    for phase in &mut state.phases {
        if phase.key == key {
            phase.status = "complete";
            phase.progress = 1.0;
            phase.elapsed_ms = elapsed.elapsed().as_millis() as u64;
        }
    }
}

pub async fn prepare_run(state: &SharedDashboardState, config: RunConfig) -> Result<(), String> {
    let config = config.validate()?;
    let mut state = state.write().await;
    if state.status == "running" {
        return Err("an experiment is already running".to_owned());
    }
    state.status = "running";
    state.workers_online = 0;
    state.config = config;
    state.phases = fresh_phases();
    state.current_bfs = None;
    state.results = None;
    state.error = None;
    Ok(())
}

pub async fn fail_run(state: &SharedDashboardState, error: String) {
    let mut state = state.write().await;
    state.status = "failed";
    state.error = Some(error);
    state.workers_online = 0;
    if let Some(phase) = state
        .phases
        .iter_mut()
        .find(|phase| phase.status == "active")
    {
        phase.status = "error";
    }
}

pub async fn run_distributed(state: SharedDashboardState, config: RunConfig) -> Result<(), String> {
    let run_started = Instant::now();
    let cluster_config = ClusterConfig {
        nodes: config.nodes,
        degree: config.degree,
        workers: config.workers,
        seed: config.seed,
    };
    let mut workers = try_join_all((0..config.workers as usize).map(WorkerClient::spawn)).await?;
    state.write().await.workers_online = config.workers;

    begin_phase(&state, "build").await;
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
        &state,
        "build",
        1.0,
        format!(
            "{built_nodes} vertices stored across {} processes",
            config.workers
        ),
        phase_started,
    )
    .await;
    complete_phase(&state, "build", phase_started).await;

    begin_phase(&state, "rewire").await;
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
            &state,
            "rewire",
            (source_worker + 1) as f64 / workers.len() as f64,
            format!("{rewired_edges} edges rewired; endpoint updates routed"),
            phase_started,
        )
        .await;
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
    complete_phase(&state, "rewire", phase_started).await;

    begin_phase(&state, "clustering").await;
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
            &state,
            "clustering",
            clustering_seen as f64 / clustering_count as f64,
            format!("{clustering_seen} sampled vertices resolved"),
            phase_started,
        )
        .await;
    }
    let clustering_coefficient = clustering_sum / clustering_seen as f64;
    complete_phase(&state, "clustering", phase_started).await;

    begin_phase(&state, "bfs").await;
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
                &state,
                BfsView {
                    source_index: source_index as u32 + 1,
                    source_total: config.bfs_samples,
                    level: None,
                    frontier_by_worker: vec![0; workers.len()],
                    visited_nodes: visited,
                    total_nodes: config.nodes,
                    source_progress: 1.0,
                    method: "ring",
                },
                phase_started,
            )
            .await;
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
                    .map(WorkerCommand::ApplyDiscoveries)
                    .collect(),
            )
            .await?;
            for response in responses {
                let WorkerResponse::DiscoveriesApplied { accepted } = response else {
                    return Err("unexpected discovery response".to_owned());
                };
                newly_discovered += accepted;
            }
            if newly_discovered > 0 {
                total_distance += newly_discovered * u64::from(level + 1);
                total_reachable += newly_discovered;
            }

            let responses = request_all(
                &mut workers,
                (0..config.workers)
                    .map(|_| WorkerCommand::AdvanceBfs)
                    .collect(),
            )
            .await?;
            let mut frontier_by_worker = Vec::with_capacity(workers.len());
            let mut visited_this_source = 0u64;
            for response in responses {
                let WorkerResponse::BfsAdvanced { frontier, visited } = response else {
                    return Err("unexpected BFS advance response".to_owned());
                };
                frontier_by_worker.push(frontier);
                visited_this_source += visited;
            }
            let complete = frontier_by_worker.iter().sum::<u64>() == 0;
            update_bfs(
                &state,
                BfsView {
                    source_index: source_index as u32 + 1,
                    source_total: config.bfs_samples,
                    level: Some(level + 1),
                    frontier_by_worker,
                    visited_nodes: visited_this_source,
                    total_nodes: config.nodes,
                    source_progress: source_progress(visited_this_source, config.nodes, complete),
                    method: "bfs",
                },
                phase_started,
            )
            .await;
            if complete {
                vertices_visited += visited_this_source;
                break;
            }
            level += 1;
        }
    }
    complete_phase(&state, "bfs", phase_started).await;

    let average_path_length = total_distance as f64 / total_reachable as f64;
    let results = RunResults {
        average_path_length,
        clustering_coefficient,
        elapsed_ms: run_started.elapsed().as_millis() as u64,
        cross_shard_messages,
        vertices_visited,
        rewired_edges,
        clustering_samples: clustering_seen as u32,
        adjacency_entries,
    };
    let finished_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    {
        let mut dashboard = state.write().await;
        let id = dashboard.history.last().map_or(1, |record| record.id + 1);
        dashboard.history.push(RunRecord {
            id,
            finished_at_ms,
            config,
            results: results.clone(),
        });
        dashboard.status = "complete";
        dashboard.results = Some(results);
        dashboard.workers_online = 0;
    }

    join_all(workers.into_iter().map(WorkerClient::shutdown)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RunConfig, source_progress, system_worker_limit};

    #[test]
    fn progress_advances_within_a_source_and_completes_disconnected_sources() {
        assert_eq!(source_progress(1, 101, false), 0.0);
        assert_eq!(source_progress(51, 101, false), 0.5);
        assert_eq!(source_progress(101, 101, true), 1.0);
        assert_eq!(source_progress(20, 101, true), 1.0);
    }

    #[test]
    fn accepts_node_counts_above_the_previous_demo_cap() {
        let config = RunConfig {
            nodes: 5_000_001,
            workers: 1,
            ..RunConfig::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn worker_count_is_limited_by_available_parallelism() {
        let limit = system_worker_limit();
        let valid = RunConfig {
            workers: limit,
            ..RunConfig::default()
        };
        assert!(valid.validate().is_ok());

        if let Some(too_many) = limit.checked_add(1) {
            let invalid = RunConfig {
                workers: too_many,
                ..RunConfig::default()
            };
            assert!(invalid.validate().is_err());
        }
    }
}
