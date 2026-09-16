use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

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
        mutations_by_owner: Vec<Vec<Mutation>>,
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
        queries_by_owner: Vec<Vec<EdgeQuery>>,
    },
    EdgeQueriesResolved {
        counts: Vec<(u32, u32)>,
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
