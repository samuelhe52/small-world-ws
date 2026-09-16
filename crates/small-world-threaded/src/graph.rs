use std::fmt;

#[derive(Debug, Clone)]
pub struct Graph {
    pub(crate) adjacency: Vec<Vec<usize>>,
    pub(crate) edge_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct RewireStats {
    pub considered_edges: usize,
    pub rewired_edges: usize,
    pub cross_partition_updates: usize,
    pub partitions: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PathEstimate {
    pub mean_distance: f64,
    pub reachable_pairs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    InvalidNodeCount,
    InvalidDegree,
    InvalidProbability,
    InvalidPartitionCount,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNodeCount => write!(f, "N must be at least 3"),
            Self::InvalidDegree => write!(f, "K must be even and satisfy 2 <= K < N"),
            Self::InvalidProbability => write!(f, "p must be in [0, 1]"),
            Self::InvalidPartitionCount => write!(f, "partitions must be at least 1"),
        }
    }
}

impl std::error::Error for GraphError {}

impl Graph {
    pub fn node_count(&self) -> usize {
        self.adjacency.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub fn degree(&self, node: usize) -> usize {
        self.adjacency[node].len()
    }

    pub fn neighbors(&self, node: usize) -> &[usize] {
        &self.adjacency[node]
    }

    pub fn has_edge(&self, u: usize, v: usize) -> bool {
        self.adjacency[u].binary_search(&v).is_ok()
    }

    pub fn validate(&self) -> Result<(), String> {
        let n = self.node_count();
        let mut degree_sum = 0usize;

        for (u, neighbors) in self.adjacency.iter().enumerate() {
            if neighbors.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(format!("adjacency list {u} is not strictly sorted"));
            }
            for &v in neighbors {
                if v >= n {
                    return Err(format!("edge ({u}, {v}) has an invalid endpoint"));
                }
                if u == v {
                    return Err(format!("self-loop at node {u}"));
                }
                if !self.has_edge(v, u) {
                    return Err(format!("edge ({u}, {v}) is not symmetric"));
                }
            }
            degree_sum += neighbors.len();
        }

        if !degree_sum.is_multiple_of(2) || degree_sum / 2 != self.edge_count {
            return Err("stored edge count does not match adjacency lists".to_owned());
        }
        Ok(())
    }
}
