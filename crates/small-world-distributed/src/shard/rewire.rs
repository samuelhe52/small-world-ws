use super::{Shard, flush_batches};
use crate::owner;
use crate::protocol::{BATCH_ITEM_LIMIT, Mutation, WorkerResponse};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use tokio::io::AsyncWrite;

impl Shard {
    fn splitmix64(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub(super) fn choose_target(
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

    pub(super) async fn rewire<W: AsyncWrite + Unpin>(
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

    pub(super) fn apply_one_mutation(&mut self, mutation: Mutation) -> Result<(), String> {
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

    pub(super) fn apply_mutations(
        &mut self,
        mutations: Vec<Mutation>,
    ) -> Result<WorkerResponse, String> {
        let count = mutations.len() as u64;
        for mutation in mutations {
            self.apply_one_mutation(mutation)?;
        }
        Ok(WorkerResponse::MutationsApplied { count })
    }
}
