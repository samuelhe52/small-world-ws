use serde::{Deserialize, Serialize};

pub fn system_worker_limit() -> u32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(u32::MAX as usize) as u32)
        .unwrap_or(1)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentConfig {
    pub nodes: u32,
    pub degree: u32,
    pub probability: f64,
    pub bfs_samples: u32,
    pub workers: u32,
    pub seed: u64,
}

impl Default for ExperimentConfig {
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

impl ExperimentConfig {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Build,
    Rewire,
    Clustering,
    Bfs,
}

#[derive(Debug, Clone)]
pub struct BfsProgress {
    pub source_index: u32,
    pub source_total: u32,
    pub level: Option<u32>,
    pub frontier_by_worker: Vec<u64>,
    pub visited_nodes: u64,
    pub total_nodes: u32,
    pub source_progress: f64,
    pub method: BfsMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BfsMethod {
    Ring,
    Traversal,
}

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    WorkersStarted(u32),
    PhaseStarted(Phase),
    PhaseProgress {
        phase: Phase,
        progress: f64,
        detail: String,
        elapsed_ms: u64,
    },
    PhaseCompleted {
        phase: Phase,
        elapsed_ms: u64,
    },
    Bfs(BfsProgress),
}

pub trait ProgressReporter: Send + Sync {
    fn report(&self, event: ProgressEvent);
}

impl<F> ProgressReporter for F
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    fn report(&self, event: ProgressEvent) {
        self(event);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentResult {
    pub average_path_length: f64,
    pub clustering_coefficient: f64,
    pub elapsed_ms: u64,
    pub cross_shard_messages: u64,
    pub vertices_visited: u64,
    pub rewired_edges: u64,
    pub clustering_samples: u32,
    pub adjacency_entries: u64,
}
