# Distributed architecture

## Process and storage model

The `serve` process has three responsibilities: HTTP API/static frontend,
experiment coordination, and progress aggregation. It launches `W` instances of
the same executable using the hidden `worker --id i` command.

For `N` nodes, worker `i` owns the half-open range

```text
[floor(N * i / W), floor(N * (i + 1) / W))
```

and stores `Vec<Vec<u32>>` adjacency only for those nodes. Neighbor IDs remain
global. The coordinator never receives the adjacency lists. Worker requests and
responses are serialized with `bincode` and sent as length-prefixed frames over
the workers' stdin/stdout pipes.

This is local multi-process distribution: storage is genuinely sharded across
isolated address spaces, while transport is local IPC rather than multiple
physical hosts. Replacing pipes with TCP does not change the ownership or
algorithm protocols described below.

## Distributed WS construction and rewiring

Initial ring construction requires no communication. Every worker can derive
the `K/2` left and right neighbors of each locally owned node from `N` and `K`.

Rewiring uses the standard canonical clockwise edges `(u, u+d)` for
`d=1..K/2`. Shards take turns as the authoritative rewiring shard:

1. The active worker processes every canonical edge whose fixed endpoint `u`
   it owns.
2. It checks duplicates against the complete local adjacency list of `u`.
3. It updates `u` immediately.
4. If the old or new opposite endpoint is remote, it returns a remove/add
   mutation addressed to that endpoint's owner.
5. The coordinator routes those mutations and waits for acknowledgements before
   activating the next shard.

Serial shard authority is intentional. It makes duplicate prevention and edge
symmetry deterministic without distributed locks or rollback. Workers still
perform graph storage and endpoint mutation independently. After rewiring, the
coordinator requests only adjacency-entry counts and verifies the global
invariant `sum(degrees) = N*K`.

## Distributed clustering coefficient

At large `N`, the demo samples up to 20,000 vertices uniformly without
replacement. For each sampled vertex `u`, its owner enumerates all unordered
neighbor pairs `(a,b)`. Checking whether `(a,b)` exists is routed to `owner(a)`,
the only process that stores `adj[a]`.

Target workers aggregate successful queries by sampled-vertex index. The
coordinator combines counts and computes

```text
C_u = 2 * neighbor_edges / (degree(u) * (degree(u) - 1))
```

before averaging across the sample. The UI labels `C` as sampled rather than
presenting it as an exact all-vertex result.

## Level-synchronous distributed BFS

Every sampled source runs one collective BFS across all workers:

```text
start source
    ↓
expand local frontiers in all workers
    ↓
group remote discoveries by target owner
    ↓
route, deduplicate, and accept unvisited vertices
    ↓
global frontier/termination check
    ↺ next level
```

Each worker owns its local visited bitmap, current frontier, and next frontier.
It never reads another worker's adjacency. The coordinator sums accepted
discoveries at level `d` to accumulate the distance contribution `d * count`.
When every worker reports an empty next frontier, that source is complete.

Sources are uniform samples without replacement. They execute sequentially
because every BFS uses the full worker cluster; the expensive expansion inside
each level runs concurrently in all worker processes.

## Dashboard and progress

The Axum server exposes:

- `GET /api/status` - current phases, frontier sizes, results, and in-memory run
  history.
- `POST /api/run` - validate configuration and start one experiment.

The React frontend polls status every 400 ms. While a run is active, controls
are disabled, the worker count reflects live child processes, phase progress is
updated, and per-worker frontier sizes drive the activity lanes. Run history is
kept in server memory and resets when the server restarts.

## Reference run

One release-mode run on the Apple M1 Pro development host used four workers,
`N=1,000,000`, `K=10`, `p=0.05`, 32 BFS sources, and 20,000 clustering samples:

| Result | Value |
| --- | ---: |
| Adjacency entries | 10,000,000 |
| Rewired edges | 250,848 |
| Cross-shard protocol items | 11,501,939 |
| BFS vertices visited | 32,000,000 |
| Average path length `L` | 11.758442 |
| Sampled clustering `C` | 0.573552 |
| End-to-end elapsed time | 2.038 s |

The timing is a local reference, not a portable performance guarantee.

## Deliberate limitations

- Workers run on one host and use pipes, so there is no network-failure model.
- The coordinator is a single routing and synchronization point.
- Run history is not persisted.
- Clustering is sampled above 20,000 nodes.
- Rewiring shards are authoritative one at a time; endpoint storage is
  distributed, but the rewiring decision phase is not concurrently committed.

These boundaries keep the demo understandable while retaining the essential
properties the earlier Rayon implementation lacked: isolated shard storage,
explicit cross-shard messages, and collective distributed BFS.

