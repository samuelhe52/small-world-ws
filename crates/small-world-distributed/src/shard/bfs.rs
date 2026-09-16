use super::Shard;
use crate::owner;
use crate::protocol::WorkerResponse;

impl Shard {
    pub(super) fn start_bfs(&mut self, source: u32) -> Result<WorkerResponse, String> {
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

    pub(super) fn ring_distances(&self, source: u32) -> Result<WorkerResponse, String> {
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

    pub(super) fn expand_bfs(&mut self) -> Result<WorkerResponse, String> {
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

    pub(super) fn apply_discoveries(
        &mut self,
        discoveries: Vec<u32>,
    ) -> Result<WorkerResponse, String> {
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

    pub(super) fn advance_bfs(&mut self) -> WorkerResponse {
        self.frontier.clear();
        std::mem::swap(&mut self.frontier, &mut self.next_frontier);
        WorkerResponse::BfsAdvanced {
            frontier: self.frontier.len() as u64,
            visited: self.visited_count,
        }
    }
}
