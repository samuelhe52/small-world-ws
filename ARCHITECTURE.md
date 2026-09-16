# Distributed architecture

## Process and storage model

The `serve` process launches `W` worker processes. It serves the HTTP API and
static frontend, coordinates experiments, and collects progress. Each worker
runs the hidden `worker --id i` command, where `i` identifies the worker.

For a graph with `N` nodes and degree `K`, worker `i` owns the half-open range

```text
[floor(N * i / W), floor(N * (i + 1) / W))
```

and stores `Vec<Vec<u32>>` adjacency only for those nodes. Neighbor IDs remain
global. The coordinator never receives the adjacency lists. Worker requests and
responses are serialized with `bincode` and sent as length-prefixed frames over
the workers' stdin/stdout pipes.

Each worker stores part of the graph in its own address space. The workers
communicate through local pipes rather than across physical hosts. Replacing
pipes with TCP would preserve the ownership and algorithm protocols described
below.

## Distributed WS construction and rewiring

Initial ring construction requires no communication. Every worker can derive
the `K/2` left and right neighbors of each locally owned node from `N` and `K`.

Rewiring uses canonical clockwise edges `(u, u+d)`, where `u` is the fixed
endpoint and `d` ranges from 1 through `K/2`. Shards take turns as the
authoritative rewiring shard:

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

The dashboard reports `C`, the average clustering coefficient. The demo
estimates `C` by sampling up to 20,000 vertices uniformly without replacement.
When `N` is 20,000 or less, it uses every vertex. For each sampled vertex `u`,
its owner enumerates all unordered neighbor pairs `(a,b)`. If `a` belongs to a
different worker, the coordinator routes the query to `owner(a)`, the only
process that stores `adj[a]`.

Target workers aggregate successful queries by sampled-vertex index. The
coordinator combines counts and computes

```text
C_u = 2 * neighbor_edges / (degree(u) * (degree(u) - 1))
```

before averaging across the sample. The UI labels `C` as sampled rather than
presenting it as an exact all-vertex result.

## Level-synchronous distributed BFS

For rewired graphs, where `p` is greater than zero, every sampled source runs a
BFS across all workers:

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
It maintains a running visited count rather than rescanning the bitmap at each level.
It never reads another worker's adjacency. The coordinator sums accepted
discoveries at level `d` to accumulate the distance contribution `d * count`.
When every worker reports an empty next frontier, that source is complete.

Sources are uniform samples without replacement. They run one at a time because
each BFS uses every worker. The expansion for each level runs concurrently in
all worker processes.

The dashboard reports `L`, the average path length. For `p=0`, workers compute
exact distances from each sampled source to their own vertices in the intact
ring: `ceil(min(|u-s|, N-|u-s|) / (K/2))`. This keeps the same sampled sources,
distance denominator, and worker ownership while avoiding one synchronization
round per ring level. Workers reject this shortcut after their adjacency has
been mutated. The dashboard labels this as exact ring distances and does not
show a simulated BFS frontier.

## Dashboard and progress

The Axum server exposes:

- `GET /api/status` - current phases, frontier sizes, results, and in-memory run
  history.
- `POST /api/run` - validate configuration and start one experiment.

The React frontend polls status every 400 ms. While a run is active, controls
are disabled, the worker count reflects live child processes, phase progress is
updated, and per-worker frontier sizes drive the activity lanes. Run history is
kept in server memory and resets when the server restarts.
During BFS, progress includes vertices reached within the current source;
disconnected sources count as complete when their frontier empties. The UI also
shows current-source progress and the active experiment's position in a p sweep.

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
