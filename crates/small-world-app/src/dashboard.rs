use serde::Serialize;
use small_world_distributed::{
    BfsMethod, BfsProgress, ExperimentConfig, ExperimentResult, Phase, ProgressEvent,
    system_worker_limit,
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, mpsc};

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

impl From<BfsProgress> for BfsView {
    fn from(progress: BfsProgress) -> Self {
        Self {
            source_index: progress.source_index,
            source_total: progress.source_total,
            level: progress.level,
            frontier_by_worker: progress.frontier_by_worker,
            visited_nodes: progress.visited_nodes,
            total_nodes: progress.total_nodes,
            source_progress: progress.source_progress,
            method: match progress.method {
                BfsMethod::Ring => "ring",
                BfsMethod::Traversal => "bfs",
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: u64,
    pub finished_at_ms: u64,
    pub config: ExperimentConfig,
    pub results: ExperimentResult,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardState {
    pub status: &'static str,
    pub workers_online: u32,
    pub worker_limit: u32,
    pub config: ExperimentConfig,
    pub phases: Vec<PhaseView>,
    pub current_bfs: Option<BfsView>,
    pub results: Option<ExperimentResult>,
    pub history: Vec<RunRecord>,
    pub error: Option<String>,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            status: "idle",
            workers_online: 0,
            worker_limit: system_worker_limit(),
            config: ExperimentConfig::default(),
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

fn phase_key(phase: Phase) -> &'static str {
    match phase {
        Phase::Build => "build",
        Phase::Rewire => "rewire",
        Phase::Clustering => "clustering",
        Phase::Bfs => "bfs",
    }
}

pub async fn prepare_run(
    state: &SharedDashboardState,
    config: ExperimentConfig,
) -> Result<(), String> {
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

async fn apply_progress(state: &SharedDashboardState, event: ProgressEvent) {
    let mut dashboard = state.write().await;
    match event {
        ProgressEvent::WorkersStarted(count) => dashboard.workers_online = count,
        ProgressEvent::PhaseStarted(phase) => {
            if let Some(view) = dashboard
                .phases
                .iter_mut()
                .find(|view| view.key == phase_key(phase))
            {
                view.status = "active";
                view.progress = 0.0;
                view.elapsed_ms = 0;
            }
        }
        ProgressEvent::PhaseProgress {
            phase,
            progress,
            detail,
            elapsed_ms,
        } => {
            if let Some(view) = dashboard
                .phases
                .iter_mut()
                .find(|view| view.key == phase_key(phase))
            {
                view.progress = progress;
                view.detail = detail;
                view.elapsed_ms = elapsed_ms;
            }
        }
        ProgressEvent::PhaseCompleted { phase, elapsed_ms } => {
            if let Some(view) = dashboard
                .phases
                .iter_mut()
                .find(|view| view.key == phase_key(phase))
            {
                view.status = "complete";
                view.progress = 1.0;
                view.elapsed_ms = elapsed_ms;
            }
        }
        ProgressEvent::Bfs(progress) => dashboard.current_bfs = Some(progress.into()),
    }
}

pub async fn run_experiment(state: SharedDashboardState, config: ExperimentConfig) {
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let progress_state = state.clone();
    let reducer = tokio::spawn(async move {
        while let Some(event) = progress_rx.recv().await {
            apply_progress(&progress_state, event).await;
        }
    });

    let result = small_world_distributed::run(config, &move |event| {
        let _ = progress_tx.send(event);
    })
    .await;
    let _ = reducer.await;

    match result {
        Ok(results) => {
            let finished_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
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
        Err(error) => {
            let mut dashboard = state.write().await;
            dashboard.status = "failed";
            dashboard.error = Some(error);
            dashboard.workers_online = 0;
            if let Some(phase) = dashboard
                .phases
                .iter_mut()
                .find(|phase| phase.status == "active")
            {
                phase.status = "error";
            }
        }
    }
}
