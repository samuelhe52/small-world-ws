mod bfs;
mod clustering;
mod rewire;

use crate::protocol::{ClusterConfig, WorkerCommand, WorkerResponse, write_frame};
use tokio::io::AsyncWrite;

pub(super) async fn flush_batches<W, T>(
    output: &mut W,
    batches: &mut [Vec<T>],
    response: impl Fn(u32, Vec<T>) -> WorkerResponse,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    for (target, batch) in batches.iter_mut().enumerate() {
        if !batch.is_empty() {
            write_frame(output, &response(target as u32, std::mem::take(batch)))
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub(crate) struct Shard {
    id: usize,
    config: Option<ClusterConfig>,
    start: u32,
    end: u32,
    adjacency: Vec<Vec<u32>>,
    visited: Vec<bool>,
    visited_count: u64,
    remote_seen: Vec<u64>,
    ring_intact: bool,
    frontier: Vec<u32>,
    next_frontier: Vec<u32>,
}

impl Shard {
    pub(crate) fn new(id: usize) -> Self {
        Self {
            id,
            config: None,
            start: 0,
            end: 0,
            adjacency: Vec::new(),
            visited: Vec::new(),
            visited_count: 0,
            remote_seen: Vec::new(),
            ring_intact: false,
            frontier: Vec::new(),
            next_frontier: Vec::new(),
        }
    }

    fn config(&self) -> Result<ClusterConfig, String> {
        self.config
            .ok_or_else(|| "worker has not built a shard".to_owned())
    }

    fn local_index(&self, node: u32) -> Result<usize, String> {
        if node < self.start || node >= self.end {
            return Err(format!(
                "worker {} does not own node {node} (owns {}..{})",
                self.id, self.start, self.end
            ));
        }
        Ok((node - self.start) as usize)
    }

    fn contains_edge(&self, a: u32, b: u32) -> Result<bool, String> {
        let index = self.local_index(a)?;
        Ok(self.adjacency[index].binary_search(&b).is_ok())
    }

    fn insert_neighbor(neighbors: &mut Vec<u32>, node: u32) -> bool {
        match neighbors.binary_search(&node) {
            Ok(_) => false,
            Err(position) => {
                neighbors.insert(position, node);
                true
            }
        }
    }

    fn remove_neighbor(neighbors: &mut Vec<u32>, node: u32) -> bool {
        match neighbors.binary_search(&node) {
            Ok(position) => {
                neighbors.remove(position);
                true
            }
            Err(_) => false,
        }
    }

    fn build(&mut self, config: ClusterConfig) -> Result<WorkerResponse, String> {
        if config.workers == 0 || self.id >= config.workers as usize {
            return Err("invalid worker count or worker id".to_owned());
        }
        let start = (u64::from(config.nodes) * self.id as u64 / u64::from(config.workers)) as u32;
        let end =
            (u64::from(config.nodes) * (self.id + 1) as u64 / u64::from(config.workers)) as u32;
        let half = config.degree / 2;
        let nodes = u64::from(config.nodes);
        let mut adjacency = Vec::with_capacity((end - start) as usize);
        for u in start..end {
            let mut neighbors = Vec::with_capacity(config.degree as usize);
            for offset in 1..=half {
                neighbors.push(((u64::from(u) + u64::from(offset)) % nodes) as u32);
                neighbors.push(((u64::from(u) + nodes - u64::from(offset)) % nodes) as u32);
            }
            neighbors.sort_unstable();
            neighbors.dedup();
            adjacency.push(neighbors);
        }
        let entries = adjacency
            .iter()
            .map(|neighbors| neighbors.len() as u64)
            .sum();
        self.config = Some(config);
        self.start = start;
        self.end = end;
        self.visited = vec![false; adjacency.len()];
        self.visited_count = 0;
        self.ring_intact = true;
        self.adjacency = adjacency;
        self.frontier.clear();
        self.next_frontier.clear();
        Ok(WorkerResponse::Built {
            local_nodes: end - start,
            adjacency_entries: entries,
        })
    }

    fn graph_stats(&self) -> WorkerResponse {
        WorkerResponse::GraphStats {
            local_nodes: self.end - self.start,
            adjacency_entries: self
                .adjacency
                .iter()
                .map(|neighbors| neighbors.len() as u64)
                .sum(),
        }
    }

    pub(crate) async fn handle<W: AsyncWrite + Unpin>(
        &mut self,
        command: WorkerCommand,
        output: &mut W,
    ) -> Result<WorkerResponse, String> {
        match command {
            WorkerCommand::Build(config) => self.build(config),
            WorkerCommand::Rewire { probability, seed } => {
                self.rewire(probability, seed, output).await
            }
            WorkerCommand::ApplyMutations(mutations) => self.apply_mutations(mutations),
            WorkerCommand::GraphStats => Ok(self.graph_stats()),
            WorkerCommand::PrepareClustering { vertices } => {
                self.prepare_clustering(vertices, output).await
            }
            WorkerCommand::ResolveEdgeQueries(queries) => self.resolve_edge_queries(queries),
            WorkerCommand::StartBfs { source } => self.start_bfs(source),
            WorkerCommand::RingDistances { source } => self.ring_distances(source),
            WorkerCommand::ExpandBfs => self.expand_bfs(),
            WorkerCommand::ApplyDiscoveries(discoveries) => self.apply_discoveries(discoveries),
            WorkerCommand::AdvanceBfs => Ok(self.advance_bfs()),
            WorkerCommand::FinishBfsLevel(discoveries) => {
                let WorkerResponse::DiscoveriesApplied { accepted } =
                    self.apply_discoveries(discoveries)?
                else {
                    unreachable!()
                };
                let WorkerResponse::BfsAdvanced { frontier, visited } = self.advance_bfs() else {
                    unreachable!()
                };
                Ok(WorkerResponse::BfsLevelFinished {
                    accepted,
                    frontier,
                    visited,
                })
            }
            WorkerCommand::Shutdown => Ok(WorkerResponse::Ack),
        }
    }
}

#[cfg(test)]
mod tests;
