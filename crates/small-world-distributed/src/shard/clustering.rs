use super::{Shard, flush_batches};
use crate::owner;
use crate::protocol::{BATCH_ITEM_LIMIT, EdgeQuery, WorkerResponse};
use std::collections::HashMap;
use tokio::io::AsyncWrite;

impl Shard {
    pub(super) async fn prepare_clustering<W: AsyncWrite + Unpin>(
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

    pub(super) fn resolve_edge_queries(
        &self,
        queries: Vec<EdgeQuery>,
    ) -> Result<WorkerResponse, String> {
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
}
