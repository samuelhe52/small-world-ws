use super::owner;
use super::protocol::{
    ClusterConfig, EdgeQuery, Mutation, WorkerCommand, WorkerResponse, read_frame, write_frame,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;
use std::io;
use tokio::io::{stdin, stdout};

pub struct Worker {
    id: usize,
    config: Option<ClusterConfig>,
    start: u32,
    end: u32,
    adjacency: Vec<Vec<u32>>,
    visited: Vec<bool>,
    frontier: Vec<u32>,
    next_frontier: Vec<u32>,
}

impl Worker {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            config: None,
            start: 0,
            end: 0,
            adjacency: Vec::new(),
            visited: Vec::new(),
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
        self.adjacency = adjacency;
        self.frontier.clear();
        self.next_frontier.clear();
        Ok(WorkerResponse::Built {
            local_nodes: end - start,
            adjacency_entries: entries,
        })
    }

    fn splitmix64(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn choose_target(
        &self,
        rng: &mut ChaCha8Rng,
        u: u32,
        old_v: u32,
        neighbors: &[u32],
        nodes: u32,
    ) -> Option<u32> {
        if neighbors.len() + 2 >= nodes as usize {
            return None;
        }
        for _ in 0..64 {
            let candidate = rng.gen_range(0..nodes);
            if candidate != u && candidate != old_v && neighbors.binary_search(&candidate).is_err()
            {
                return Some(candidate);
            }
        }
        (0..nodes).find(|candidate| {
            *candidate != u && *candidate != old_v && neighbors.binary_search(candidate).is_err()
        })
    }

    fn rewire(&mut self, probability: f64, seed: u64) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        let mut mutations_by_owner = vec![Vec::new(); config.workers as usize];
        let half = config.degree / 2;
        let mut considered = 0u64;
        let mut rewired = 0u64;

        for u in self.start..self.end {
            for offset in 1..=half {
                let v = ((u64::from(u) + u64::from(offset)) % u64::from(config.nodes)) as u32;
                let edge_index = u64::from(u) * u64::from(half) + u64::from(offset - 1);
                let mut rng = ChaCha8Rng::seed_from_u64(Self::splitmix64(seed ^ edge_index));
                considered += 1;
                if !rng.gen_bool(probability) {
                    continue;
                }

                let u_index = self.local_index(u)?;
                let Some(w) =
                    self.choose_target(&mut rng, u, v, &self.adjacency[u_index], config.nodes)
                else {
                    continue;
                };
                if !Self::remove_neighbor(&mut self.adjacency[u_index], v)
                    || !Self::insert_neighbor(&mut self.adjacency[u_index], w)
                {
                    return Err(format!(
                        "rewiring invariant failed for ({u}, {v}) -> ({u}, {w})"
                    ));
                }

                for mutation in [
                    Mutation {
                        node: v,
                        other: u,
                        add: false,
                    },
                    Mutation {
                        node: w,
                        other: u,
                        add: true,
                    },
                ] {
                    let target = owner(mutation.node, config.nodes, config.workers);
                    if target == self.id {
                        self.apply_one_mutation(mutation)?;
                    } else {
                        mutations_by_owner[target].push(mutation);
                    }
                }
                rewired += 1;
            }
        }

        Ok(WorkerResponse::Rewired {
            considered,
            rewired,
            mutations_by_owner,
        })
    }

    fn apply_one_mutation(&mut self, mutation: Mutation) -> Result<(), String> {
        let index = self.local_index(mutation.node)?;
        let changed = if mutation.add {
            Self::insert_neighbor(&mut self.adjacency[index], mutation.other)
        } else {
            Self::remove_neighbor(&mut self.adjacency[index], mutation.other)
        };
        if !changed {
            return Err(format!(
                "mutation was not applicable: node={}, other={}, add={}",
                mutation.node, mutation.other, mutation.add
            ));
        }
        Ok(())
    }

    fn apply_mutations(&mut self, mutations: Vec<Mutation>) -> Result<WorkerResponse, String> {
        let count = mutations.len() as u64;
        for mutation in mutations {
            self.apply_one_mutation(mutation)?;
        }
        Ok(WorkerResponse::MutationsApplied { count })
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

    fn prepare_clustering(&self, vertices: Vec<u32>) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        let mut degrees = Vec::with_capacity(vertices.len());
        let mut queries_by_owner = vec![Vec::new(); config.workers as usize];

        for (origin_index, u) in vertices.into_iter().enumerate() {
            let neighbors = &self.adjacency[self.local_index(u)?];
            degrees.push(neighbors.len() as u32);
            for i in 0..neighbors.len() {
                for &b in &neighbors[i + 1..] {
                    let a = neighbors[i];
                    queries_by_owner[owner(a, config.nodes, config.workers)].push(EdgeQuery {
                        origin_index: origin_index as u32,
                        a,
                        b,
                    });
                }
            }
        }
        Ok(WorkerResponse::ClusteringPrepared {
            degrees,
            queries_by_owner,
        })
    }

    fn resolve_edge_queries(&self, queries: Vec<EdgeQuery>) -> Result<WorkerResponse, String> {
        let mut counts = HashMap::<u32, u32>::new();
        for query in queries {
            if self.contains_edge(query.a, query.b)? {
                *counts.entry(query.origin_index).or_default() += 1;
            }
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort_unstable_by_key(|entry| entry.0);
        Ok(WorkerResponse::EdgeQueriesResolved { counts })
    }

    fn start_bfs(&mut self, source: u32) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        self.visited.fill(false);
        self.frontier.clear();
        self.next_frontier.clear();
        if owner(source, config.nodes, config.workers) == self.id {
            let index = self.local_index(source)?;
            self.visited[index] = true;
            self.frontier.push(source);
        }
        Ok(WorkerResponse::BfsStarted)
    }

    fn expand_bfs(&mut self) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        let mut discoveries_by_owner = vec![Vec::new(); config.workers as usize];
        let mut local_discovered = 0u64;

        for &u in &self.frontier {
            let u_index = self.local_index(u)?;
            for &v in &self.adjacency[u_index] {
                let target = owner(v, config.nodes, config.workers);
                if target == self.id {
                    let index = (v - self.start) as usize;
                    if !self.visited[index] {
                        self.visited[index] = true;
                        self.next_frontier.push(v);
                        local_discovered += 1;
                    }
                } else {
                    discoveries_by_owner[target].push(v);
                }
            }
        }
        for discoveries in &mut discoveries_by_owner {
            discoveries.sort_unstable();
            discoveries.dedup();
        }
        Ok(WorkerResponse::BfsExpanded {
            local_discovered,
            discoveries_by_owner,
        })
    }

    fn apply_discoveries(&mut self, discoveries: Vec<u32>) -> Result<WorkerResponse, String> {
        let mut accepted = 0u64;
        for node in discoveries {
            let index = self.local_index(node)?;
            if !self.visited[index] {
                self.visited[index] = true;
                self.next_frontier.push(node);
                accepted += 1;
            }
        }
        Ok(WorkerResponse::DiscoveriesApplied { accepted })
    }

    fn advance_bfs(&mut self) -> WorkerResponse {
        self.frontier.clear();
        std::mem::swap(&mut self.frontier, &mut self.next_frontier);
        WorkerResponse::BfsAdvanced {
            frontier: self.frontier.len() as u64,
            visited: self.visited.iter().filter(|visited| **visited).count() as u64,
        }
    }

    fn handle(&mut self, command: WorkerCommand) -> Result<WorkerResponse, String> {
        match command {
            WorkerCommand::Build(config) => self.build(config),
            WorkerCommand::Rewire { probability, seed } => self.rewire(probability, seed),
            WorkerCommand::ApplyMutations(mutations) => self.apply_mutations(mutations),
            WorkerCommand::GraphStats => Ok(self.graph_stats()),
            WorkerCommand::PrepareClustering { vertices } => self.prepare_clustering(vertices),
            WorkerCommand::ResolveEdgeQueries(queries) => self.resolve_edge_queries(queries),
            WorkerCommand::StartBfs { source } => self.start_bfs(source),
            WorkerCommand::ExpandBfs => self.expand_bfs(),
            WorkerCommand::ApplyDiscoveries(discoveries) => self.apply_discoveries(discoveries),
            WorkerCommand::AdvanceBfs => Ok(self.advance_bfs()),
            WorkerCommand::Shutdown => Ok(WorkerResponse::Ack),
        }
    }
}

pub async fn run_worker(id: usize) -> io::Result<()> {
    let mut worker = Worker::new(id);
    let mut input = stdin();
    let mut output = stdout();

    while let Some(command) = read_frame::<_, WorkerCommand>(&mut input).await? {
        let shutdown = matches!(command, WorkerCommand::Shutdown);
        let response = worker.handle(command).unwrap_or_else(WorkerResponse::Error);
        write_frame(&mut output, &response).await?;
        if shutdown {
            break;
        }
    }
    Ok(())
}
