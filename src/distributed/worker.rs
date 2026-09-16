use super::owner;
use super::protocol::{
    BATCH_ITEM_LIMIT, ClusterConfig, EdgeQuery, Mutation, WorkerCommand, WorkerResponse,
    read_frame, write_frame,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite, stdin, stdout};

async fn flush_batches<W, T>(
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

pub struct Worker {
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

impl Worker {
    pub fn new(id: usize) -> Self {
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
        if neighbors.len() + 1 >= nodes as usize {
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

    async fn rewire<W: AsyncWrite + Unpin>(
        &mut self,
        probability: f64,
        seed: u64,
        output: &mut W,
    ) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        if probability == 0.0 {
            return Ok(WorkerResponse::Rewired {
                considered: u64::from(self.end - self.start) * u64::from(config.degree / 2),
                rewired: 0,
            });
        }
        let mut mutations_by_owner = vec![Vec::new(); config.workers as usize];
        let half = config.degree / 2;
        let mut considered = 0u64;
        let mut rewired = 0u64;
        let mut queued = 0usize;

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
                        queued += 1;
                        if queued == BATCH_ITEM_LIMIT {
                            flush_batches(output, &mut mutations_by_owner, |target, mutations| {
                                WorkerResponse::RewireMutations { target, mutations }
                            })
                            .await?;
                            queued = 0;
                        }
                    }
                }
                rewired += 1;
                self.ring_intact = false;
            }
        }

        flush_batches(output, &mut mutations_by_owner, |target, mutations| {
            WorkerResponse::RewireMutations { target, mutations }
        })
        .await?;
        Ok(WorkerResponse::Rewired {
            considered,
            rewired,
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
        self.ring_intact = false;
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

    async fn prepare_clustering<W: AsyncWrite + Unpin>(
        &self,
        vertices: Vec<u32>,
        output: &mut W,
    ) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        let mut degrees = Vec::with_capacity(vertices.len());
        let mut queries_by_owner = vec![Vec::new(); config.workers as usize];
        let mut local_counts = Vec::with_capacity(vertices.len());
        let mut queued = 0usize;

        for (origin_index, u) in vertices.into_iter().enumerate() {
            let neighbors = &self.adjacency[self.local_index(u)?];
            degrees.push(neighbors.len() as u32);
            let mut local_count = 0u64;
            for i in 0..neighbors.len() {
                for &b in &neighbors[i + 1..] {
                    let a = neighbors[i];
                    let target = owner(a, config.nodes, config.workers);
                    if target == self.id {
                        local_count += u64::from(self.contains_edge(a, b)?);
                        continue;
                    }
                    queries_by_owner[target].push(EdgeQuery {
                        origin_index: origin_index as u32,
                        a,
                        b,
                    });
                    queued += 1;
                    if queued == BATCH_ITEM_LIMIT {
                        flush_batches(output, &mut queries_by_owner, |target, queries| {
                            WorkerResponse::ClusteringQueries { target, queries }
                        })
                        .await?;
                        queued = 0;
                    }
                }
            }
            local_counts.push(local_count);
        }
        flush_batches(output, &mut queries_by_owner, |target, queries| {
            WorkerResponse::ClusteringQueries { target, queries }
        })
        .await?;
        Ok(WorkerResponse::ClusteringPrepared {
            degrees,
            local_counts,
        })
    }

    fn resolve_edge_queries(&self, queries: Vec<EdgeQuery>) -> Result<WorkerResponse, String> {
        let mut counts = HashMap::<u32, u64>::new();
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
        // One bit per global vertex: send a remote discovery only once per
        // source, rather than once per incident edge and again at later levels.
        self.remote_seen.resize(
            if config.workers > 1 {
                (config.nodes as usize).div_ceil(64)
            } else {
                0
            },
            0,
        );
        self.remote_seen.fill(0);
        self.visited_count = 0;
        self.frontier.clear();
        self.next_frontier.clear();
        if owner(source, config.nodes, config.workers) == self.id {
            let index = self.local_index(source)?;
            self.visited[index] = true;
            self.visited_count = 1;
            self.frontier.push(source);
        }
        Ok(WorkerResponse::BfsStarted)
    }

    fn ring_distances(&self, source: u32) -> Result<WorkerResponse, String> {
        let config = self.config()?;
        if !self.ring_intact || source >= config.nodes || config.degree < 2 {
            return Err("ring distances require an intact ring and a valid source".to_owned());
        }
        let half = u64::from(config.degree / 2);
        let nodes = u64::from(config.nodes);
        // Sum ceil(d / half) over a contiguous interval of cyclic distances.
        // Each shard answers in constant time, independent of its node count.
        let ascending = |distance: u64| {
            let q = distance / half;
            let r = distance % half;
            half * q * (q + 1) / 2 + r * (q + 1)
        };
        let total = 2 * ascending((nodes - 1) / 2)
            + if nodes.is_multiple_of(2) {
                (nodes / 2).div_ceil(half)
            } else {
                0
            };
        let prefix = |end: u64| {
            if end == 0 {
                0
            } else if end <= nodes / 2 + 1 {
                ascending(end - 1)
            } else {
                total - ascending(nodes - end)
            }
        };
        let start = (u64::from(self.start) + nodes - u64::from(source)) % nodes;
        let end = start + u64::from(self.end - self.start);
        let distance_sum = if end <= nodes {
            prefix(end) - prefix(start)
        } else {
            total - prefix(start) + prefix(end - nodes)
        };
        let reachable =
            u64::from(self.end - self.start) - u64::from(source >= self.start && source < self.end);
        Ok(WorkerResponse::RingDistances {
            distance_sum,
            reachable,
            visited: u64::from(self.end - self.start),
        })
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
                    let word = &mut self.remote_seen[v as usize / 64];
                    let mask = 1u64 << (v % 64);
                    if *word & mask == 0 {
                        *word |= mask;
                        discoveries_by_owner[target].push(v);
                    }
                }
            }
        }
        self.visited_count += local_discovered;
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
        self.visited_count += accepted;
        Ok(WorkerResponse::DiscoveriesApplied { accepted })
    }

    fn advance_bfs(&mut self) -> WorkerResponse {
        self.frontier.clear();
        std::mem::swap(&mut self.frontier, &mut self.next_frontier);
        WorkerResponse::BfsAdvanced {
            frontier: self.frontier.len() as u64,
            visited: self.visited_count,
        }
    }

    async fn handle<W: AsyncWrite + Unpin>(
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

pub async fn run_worker(id: usize) -> io::Result<()> {
    run_worker_stream(id, &mut stdin(), &mut stdout()).await
}

async fn run_worker_stream<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    id: usize,
    input: &mut R,
    output: &mut W,
) -> io::Result<()> {
    let mut worker = Worker::new(id);

    while let Some(command) = read_frame::<_, WorkerCommand>(input).await? {
        let shutdown = matches!(command, WorkerCommand::Shutdown);
        let response = worker
            .handle(command, output)
            .await
            .unwrap_or_else(WorkerResponse::Error);
        write_frame(output, &response).await?;
        if shutdown {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::protocol::MAX_FRAME_BYTES;
    use super::*;

    fn build_workers(nodes: u32, degree: u32, count: u32) -> Vec<Worker> {
        (0..count as usize)
            .map(|id| {
                let mut worker = Worker::new(id);
                worker
                    .build(ClusterConfig {
                        nodes,
                        degree,
                        workers: count,
                        seed: 42,
                    })
                    .unwrap();
                worker
            })
            .collect()
    }

    fn assert_batch_frame(response: &WorkerResponse, items: usize) {
        assert!((1..=BATCH_ITEM_LIMIT).contains(&items));
        let size = bincode::serialized_size(response).unwrap() as usize;
        assert!(size < 1024 * 1024);
        assert!(size < MAX_FRAME_BYTES);
    }

    #[test]
    fn ring_prefix_matches_enumeration_for_every_shard_and_source() {
        for nodes in 3..40 {
            for degree in (2..nodes).step_by(2) {
                for worker in build_workers(nodes, degree, 4) {
                    for source in 0..nodes {
                        let expected: u64 = (worker.start..worker.end)
                            .map(|v| {
                                let d = v.abs_diff(source);
                                u64::from(d.min(nodes - d).div_ceil(degree / 2))
                            })
                            .sum();
                        let WorkerResponse::RingDistances { distance_sum, .. } =
                            worker.ring_distances(source).unwrap()
                        else {
                            panic!()
                        };
                        assert_eq!(distance_sum, expected);
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn fused_level_keeps_duplicate_discoveries_and_remote_state_correct() {
        let mut workers = build_workers(20, 4, 2);
        let worker = &mut workers[0];
        for _ in 0..2 {
            worker.start_bfs(0).unwrap();
            let WorkerResponse::BfsExpanded {
                discoveries_by_owner,
                ..
            } = worker.expand_bfs().unwrap()
            else {
                panic!()
            };
            assert_eq!(discoveries_by_owner[1].len(), 2);
            let WorkerResponse::BfsExpanded {
                discoveries_by_owner,
                ..
            } = worker.expand_bfs().unwrap()
            else {
                panic!()
            };
            assert!(discoveries_by_owner.iter().all(Vec::is_empty));
            let response = worker
                .handle(
                    WorkerCommand::FinishBfsLevel(vec![1, 3, 3]),
                    &mut tokio::io::sink(),
                )
                .await
                .unwrap();
            assert!(matches!(
                response,
                WorkerResponse::BfsLevelFinished {
                    accepted: 1,
                    frontier: 3,
                    visited: 4
                }
            ));
        }
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        assert_eq!(worker.choose_target(&mut rng, 0, 1, &[1, 2], 4), Some(3));
    }

    #[test]
    fn ring_distances_match_bfs_and_visited_counts_reset_between_sources() {
        for (nodes, degree, count) in [(19, 2, 3), (20, 4, 3), (31, 10, 4), (103, 100, 4)] {
            let mut workers = build_workers(nodes, degree, count);
            for source in [0, nodes / 2, nodes - 1] {
                let expected_distance: u64 = workers
                    .iter()
                    .map(|worker| {
                        let WorkerResponse::RingDistances {
                            distance_sum,
                            reachable,
                            visited,
                        } = worker.ring_distances(source).unwrap()
                        else {
                            panic!("expected ring distances")
                        };
                        assert_eq!(visited, u64::from(worker.end - worker.start));
                        assert_eq!(
                            reachable,
                            visited - u64::from(source >= worker.start && source < worker.end)
                        );
                        distance_sum
                    })
                    .sum();
                for worker in &mut workers {
                    worker.start_bfs(source).unwrap();
                }
                let mut distance_sum = 0u64;
                for level in 1..=nodes {
                    let mut routed = vec![Vec::new(); count as usize];
                    let mut discovered = 0u64;
                    for worker in &mut workers {
                        let WorkerResponse::BfsExpanded {
                            local_discovered,
                            discoveries_by_owner,
                        } = worker.expand_bfs().unwrap()
                        else {
                            panic!("expected expansion")
                        };
                        discovered += local_discovered;
                        for (target, discoveries) in discoveries_by_owner.into_iter().enumerate() {
                            routed[target].extend(discoveries);
                        }
                    }
                    for (worker, mut discoveries) in workers.iter_mut().zip(routed) {
                        // Duplicated remote messages must not inflate the counter.
                        discoveries.extend(discoveries.clone());
                        let WorkerResponse::DiscoveriesApplied { accepted } =
                            worker.apply_discoveries(discoveries).unwrap()
                        else {
                            panic!("expected discoveries")
                        };
                        discovered += accepted;
                    }
                    distance_sum += discovered * u64::from(level);
                    let mut frontier_total = 0u64;
                    let mut visited_total = 0u64;
                    for worker in &mut workers {
                        let WorkerResponse::BfsAdvanced { frontier, visited } =
                            worker.advance_bfs()
                        else {
                            panic!("expected frontier")
                        };
                        assert_eq!(
                            visited,
                            worker.visited.iter().filter(|&&value| value).count() as u64
                        );
                        frontier_total += frontier;
                        visited_total += visited;
                    }
                    if frontier_total == 0 {
                        assert_eq!(visited_total, u64::from(nodes));
                        break;
                    }
                }
                assert_eq!(distance_sum, expected_distance);
            }
        }
    }

    #[test]
    fn ring_distances_reject_mutated_graphs() {
        let mut workers = build_workers(20, 4, 1);
        workers[0]
            .apply_one_mutation(Mutation {
                node: 0,
                other: 1,
                add: false,
            })
            .unwrap();
        assert!(workers[0].ring_distances(0).is_err());
    }

    #[tokio::test]
    async fn rewiring_streams_bounded_mutations_and_preserves_graph() {
        let nodes = 4096;
        let degree = 100;
        let mut workers = build_workers(nodes, degree, 3);
        let mut batches = 0;
        let mut routed = 0u64;
        let mut total_rewired = 0u64;
        for origin in 0..workers.len() {
            let mut source = workers.remove(origin);
            let expected_considered = u64::from(source.end - source.start) * u64::from(degree / 2);
            // A small pipe forces backpressure while the consumer routes batches.
            let (mut output, mut input) = tokio::io::duplex(4096);
            let produce = async {
                let summary = source.rewire(1.0, 42, &mut output).await.unwrap();
                write_frame(&mut output, &summary).await.unwrap();
            };
            let consume = async {
                loop {
                    let response = read_frame::<_, WorkerResponse>(&mut input)
                        .await
                        .unwrap()
                        .unwrap();
                    match &response {
                        WorkerResponse::RewireMutations { target, mutations } => {
                            assert_ne!(*target as usize, origin);
                            assert_batch_frame(&response, mutations.len());
                            batches += 1;
                            routed += mutations.len() as u64;
                            let index = *target as usize - usize::from(*target as usize > origin);
                            workers[index].apply_mutations(mutations.clone()).unwrap();
                        }
                        WorkerResponse::Rewired {
                            considered,
                            rewired,
                        } => {
                            assert_eq!(*considered, expected_considered);
                            total_rewired += rewired;
                            break;
                        }
                        _ => panic!("unexpected response: {response:?}"),
                    }
                }
            };
            tokio::join!(produce, consume);
            workers.insert(origin, source);
        }
        assert!(routed > BATCH_ITEM_LIMIT as u64);
        assert!(batches > workers.len());
        assert_eq!(total_rewired, u64::from(nodes) * u64::from(degree / 2));
        let mut entries = 0u64;
        for worker in &workers {
            for (index, neighbors) in worker.adjacency.iter().enumerate() {
                let u = worker.start + index as u32;
                assert!(neighbors.windows(2).all(|pair| pair[0] < pair[1]));
                entries += neighbors.len() as u64;
                for &v in neighbors {
                    assert_ne!(u, v);
                    assert!(
                        workers[owner(v, nodes, workers.len() as u32)]
                            .contains_edge(v, u)
                            .unwrap()
                    );
                }
            }
        }
        assert_eq!(entries, u64::from(nodes) * u64::from(degree));
    }

    #[tokio::test]
    async fn clustering_aggregates_bounded_query_batches_across_owners() {
        let workers = build_workers(512, 100, 3);
        let mut batches = 0;
        let mut remote_queries = 0;
        for source in &workers {
            let vertices: Vec<_> = (source.start..source.end).collect();
            let mut totals = vec![0u64; vertices.len()];
            let (mut output, mut input) = tokio::io::duplex(4096);
            let produce = async {
                let summary = source
                    .prepare_clustering(vertices, &mut output)
                    .await
                    .unwrap();
                write_frame(&mut output, &summary).await.unwrap();
            };
            let consume = async {
                loop {
                    let response = read_frame::<_, WorkerResponse>(&mut input)
                        .await
                        .unwrap()
                        .unwrap();
                    match &response {
                        WorkerResponse::ClusteringQueries { target, queries } => {
                            assert_ne!(*target as usize, source.id);
                            assert_batch_frame(&response, queries.len());
                            batches += 1;
                            remote_queries += queries.len();
                            let WorkerResponse::EdgeQueriesResolved { counts } = workers
                                [*target as usize]
                                .resolve_edge_queries(queries.clone())
                                .unwrap()
                            else {
                                panic!("unexpected resolution response")
                            };
                            for (index, count) in counts {
                                totals[index as usize] += count;
                            }
                        }
                        WorkerResponse::ClusteringPrepared {
                            degrees,
                            local_counts,
                        } => {
                            assert_eq!(degrees, &vec![100; totals.len()]);
                            for (total, local) in totals.iter_mut().zip(local_counts) {
                                *total += local;
                            }
                            break;
                        }
                        _ => panic!("unexpected response: {response:?}"),
                    }
                }
            };
            tokio::join!(produce, consume);
            // Ring-lattice K=100 has 3*K*(K-2)/8 neighbor-neighbor links.
            assert!(totals.iter().all(|&count| count == 3675));
        }
        assert!(remote_queries > BATCH_ITEM_LIMIT);
        assert!(batches > workers.len());
    }

    #[tokio::test]
    async fn reported_single_worker_clustering_run_fits_and_keeps_stream_open() {
        let (client, server) = tokio::io::duplex(4096);
        let (mut input, mut output) = tokio::io::split(server);
        let worker =
            tokio::spawn(async move { run_worker_stream(0, &mut input, &mut output).await });
        let (mut input, mut output) = tokio::io::split(client);
        write_frame(
            &mut output,
            &WorkerCommand::Build(ClusterConfig {
                nodes: 10_000,
                degree: 100,
                workers: 1,
                seed: 42,
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame::<_, WorkerResponse>(&mut input).await.unwrap(),
            Some(WorkerResponse::Built { .. })
        ));
        write_frame(
            &mut output,
            &WorkerCommand::PrepareClustering {
                vertices: (0..10_000).collect(),
            },
        )
        .await
        .unwrap();
        // All 49.5 million pairs are resolved locally: only a small summary is sent.
        let response = read_frame::<_, WorkerResponse>(&mut input)
            .await
            .unwrap()
            .unwrap();
        assert!(bincode::serialized_size(&response).unwrap() < MAX_FRAME_BYTES as u64);
        let WorkerResponse::ClusteringPrepared {
            degrees,
            local_counts,
        } = response
        else {
            panic!("expected summary without local query frames")
        };
        assert_eq!(degrees, vec![100; 10_000]);
        assert_eq!(local_counts, vec![3675; 10_000]);
        write_frame(&mut output, &WorkerCommand::GraphStats)
            .await
            .unwrap();
        assert!(matches!(
            read_frame::<_, WorkerResponse>(&mut input).await.unwrap(),
            Some(WorkerResponse::GraphStats {
                adjacency_entries: 1_000_000,
                ..
            })
        ));
        write_frame(&mut output, &WorkerCommand::Shutdown)
            .await
            .unwrap();
        assert!(matches!(
            read_frame::<_, WorkerResponse>(&mut input).await.unwrap(),
            Some(WorkerResponse::Ack)
        ));
        worker.await.unwrap().unwrap();
    }
}
