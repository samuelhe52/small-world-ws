use super::*;
use crate::owner;
use crate::protocol::{
    BATCH_ITEM_LIMIT, ClusterConfig, MAX_FRAME_BYTES, Mutation, WorkerCommand, WorkerResponse,
    read_frame, write_frame,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

fn build_workers(nodes: u32, degree: u32, count: u32) -> Vec<Shard> {
    (0..count as usize)
        .map(|id| {
            let mut worker = Shard::new(id);
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
                    let WorkerResponse::BfsAdvanced { frontier, visited } = worker.advance_bfs()
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
        tokio::spawn(
            async move { crate::runtime::run_worker_stream(0, &mut input, &mut output).await },
        );
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
