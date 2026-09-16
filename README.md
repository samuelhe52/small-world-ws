# Small World Lab

Small World Lab is a browser-based demo of a genuinely partitioned
Watts-Strogatz experiment. A Rust coordinator launches separate worker
processes. Each worker stores adjacency lists only for its assigned node range;
the coordinator stores progress metadata but no graph adjacency.

The dashboard lets you tune `N`, `K`, rewiring probability `p`, BFS sample
count, and worker-process count. It displays live construction, rewiring,
clustering, and distributed BFS progress, followed by `L`, `C`, run details,
and a history chart.

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

See [ARCHITECTURE.md](ARCHITECTURE.md) for protocol and correctness details.

## Verification

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cd frontend && npm run build
```

The earlier shared-memory Rayon CLI remains available as a comparison baseline:

```bash
cargo run --release -- demo
cargo run --release -- accuracy --nodes 5000 --samples 100
```
