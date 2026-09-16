use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;
// Bound the total queued items, not each owner's queue. Even the larger
// EdgeQuery frames stay below 1 MiB with bincode's fixed-width encoding.
pub const BATCH_ITEM_LIMIT: usize = 65_536;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub nodes: u32,
    pub degree: u32,
    pub workers: u32,
    pub seed: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Mutation {
    pub node: u32,
    pub other: u32,
    pub add: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EdgeQuery {
    pub origin_index: u32,
    pub a: u32,
    pub b: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerCommand {
    Build(ClusterConfig),
    Rewire { probability: f64, seed: u64 },
    ApplyMutations(Vec<Mutation>),
    GraphStats,
    PrepareClustering { vertices: Vec<u32> },
    ResolveEdgeQueries(Vec<EdgeQuery>),
    StartBfs { source: u32 },
    ExpandBfs,
    ApplyDiscoveries(Vec<u32>),
    AdvanceBfs,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerResponse {
    Built {
        local_nodes: u32,
        adjacency_entries: u64,
    },
    Rewired {
        considered: u64,
        rewired: u64,
    },
    RewireMutations {
        target: u32,
        mutations: Vec<Mutation>,
    },
    MutationsApplied {
        count: u64,
    },
    GraphStats {
        local_nodes: u32,
        adjacency_entries: u64,
    },
    ClusteringPrepared {
        degrees: Vec<u32>,
        local_counts: Vec<u64>,
    },
    ClusteringQueries {
        target: u32,
        queries: Vec<EdgeQuery>,
    },
    EdgeQueriesResolved {
        counts: Vec<(u32, u64)>,
    },
    BfsStarted,
    BfsExpanded {
        local_discovered: u64,
        discoveries_by_owner: Vec<Vec<u32>>,
    },
    DiscoveriesApplied {
        accepted: u64,
    },
    BfsAdvanced {
        frontier: u64,
        visited: u64,
    },
    Ack,
    Error(String),
}

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = bincode::serialize(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("protocol frame exceeds {MAX_FRAME_BYTES} bytes"),
        ));
    }
    writer.write_u32(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    writer.flush().await
}

pub async fn read_frame<R, T>(reader: &mut R) -> io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = match reader.read_u32().await {
        Ok(length) => length as usize,
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    };
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("incoming protocol frame is {length} bytes"),
        ));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;
    let value = bincode::deserialize(&payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_batches_and_their_replies_fit_well_below_the_frame_limit() {
        let queries = vec![
            EdgeQuery {
                origin_index: 0,
                a: 1,
                b: 2
            };
            BATCH_ITEM_LIMIT
        ];
        let mutations = vec![
            Mutation {
                node: 1,
                other: 2,
                add: true
            };
            BATCH_ITEM_LIMIT
        ];
        let sizes = [
            bincode::serialized_size(&WorkerCommand::ResolveEdgeQueries(queries.clone())).unwrap(),
            bincode::serialized_size(&WorkerResponse::ClusteringQueries { target: 1, queries })
                .unwrap(),
            bincode::serialized_size(&WorkerCommand::ApplyMutations(mutations.clone())).unwrap(),
            bincode::serialized_size(&WorkerResponse::RewireMutations {
                target: 1,
                mutations,
            })
            .unwrap(),
            bincode::serialized_size(&WorkerResponse::EdgeQueriesResolved {
                counts: (0..BATCH_ITEM_LIMIT as u32)
                    .map(|index| (index, u64::MAX))
                    .collect(),
            })
            .unwrap(),
        ];
        for size in sizes {
            assert!(size < 1024 * 1024);
            assert!(size < MAX_FRAME_BYTES as u64);
        }
    }
}
