# Small World Lab

Small World Lab is a browser demo of a partitioned Watts-Strogatz experiment. A
Rust coordinator launches separate worker processes. Each worker stores
adjacency lists only for its assigned node range. The coordinator routes work
without holding graph adjacency, while the app adapter stores dashboard state.

The workspace separates its two execution models. `small-world-threaded` owns
the in-process Rayon implementation, while `small-world-distributed` owns the
multiprocess engine, shard algorithms, protocol, and worker runtime. The
`small-world-app` crate adapts distributed progress to the dashboard and HTTP
API, and `small-world-cli` provides the executable entry points.

The dashboard lets you tune `N` (the number of nodes), `K` (the degree of each
node), `p` (the rewiring probability), the number of BFS samples, and the
number of worker processes. It displays live progress for construction,
rewiring, clustering, and distributed BFS. It then shows `L` (average path
length), `C` (clustering coefficient), run details, and a history chart.

## Start the dashboard

```bash
./scripts/run_web.sh
```

Then open [http://127.0.0.1:8080](http://127.0.0.1:8080).

The script installs/builds the React frontend and starts the release-mode Rust
server. To run the two steps manually:

```bash
cd frontend
npm install
npm run build
cd ..
cargo run --release -- serve
```

The default workload is `N=1,000,000`, `K=10`, `p=0.05`, 32 distributed BFS
sources, and up to four worker processes. Node count has no artificial UI or
application maximum (the transport uses 32-bit node IDs); worker count is
limited to the processor parallelism reported by the host operating system.

## Workspace layout

| Crate | Responsibility |
| --- | --- |
| `small-world-threaded` | Complete in-process graph, Rayon generation, clustering, and sampled BFS |
| `small-world-distributed` | Application-independent experiment API, coordinator, IPC protocol, and isolated shard algorithms |
| `small-world-app` | Dashboard state adapter, Axum API, and static frontend hosting |
| `small-world-cli` | `demo`, `accuracy`, `serve`, and internal `worker` commands |

The distributed engine reports typed progress events and returns an experiment
result. It has no dependency on the web server or dashboard state.

## What is actually distributed?

- Every worker is a separate OS process with an isolated address space.
- Worker `i` owns one contiguous node-ID range and only that range's adjacency
  lists.
- The coordinator and browser server never hold a graph replica.
- Rewiring sends remote endpoint additions/removals to the owning process.
- Clustering sends neighbor-edge existence queries to the process that owns the
  queried adjacency list.
- Every sampled BFS is level-synchronous: workers expand local frontiers,
  return remote discoveries, receive routed discoveries, and advance together.
- Messages use a length-prefixed `bincode` protocol over child-process pipes.

`C` is sampled over up to 20,000 uniformly chosen vertices so million-node runs
remain interactive. `L` is sampled using the configurable number of uniformly
chosen source vertices. Both samples are without replacement and deterministic
for the selected seed.

See [the distributed architecture](docs/ARCHITECTURE.md) for protocol and
correctness details. See [the performance audit](docs/PERFORMANCE.md) and the
[large-scale benchmark](docs/SCALING-BENCHMARK.md) for measurements.

## Verification

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd frontend && npm run build
```

The earlier shared-memory Rayon CLI remains available as a comparison baseline:

```bash
cargo run --release -- demo
cargo run --release -- accuracy --nodes 5000 --samples 100
```
