# Rust performance audit — 2026-09-16

Compared local release builds against `91abdf5` on an Apple M1 Pro with 16 GiB
RAM, using Rust 1.98.1. Measurements are local observations, not scaling guarantees.
The compact results are in [design/performance-measurements.json](../design/performance-measurements.json).
The larger 5m/10m worker and BFS-sample comparison is in
[SCALING-BENCHMARK.md](SCALING-BENCHMARK.md).

## Findings and changes

| Path | Blocker | Change |
| --- | --- | --- |
| Core graph generation | Full edge-task and proposal arrays consume memory proportional to all edges | Derive edge tasks from indices and process at most 65,536 proposals at a time. Test membership in the original ring arithmetically, preserving the proposal snapshot and canonical commit order. Skip proposal work at `p=0`. |
| Core clustering | A binary search for every neighbor pair adds repeated searches at larger degrees | Intersect sorted adjacency lists for larger tails; retain binary search for tails of at most 16 vertices. |
| Worker runtime | Each serial worker command loop started a machine-sized Tokio runtime | Use a current-thread runtime per worker; retain the server's multithread runtime. CLI graph work uses Rayon without starting Tokio. Stdio can still use Tokio blocking I/O threads. |
| Distributed BFS | Expand, apply, and advance each required a collective request/response barrier | Combine discovery application and frontier advancement into one command; keep the global level boundary. Two barriers per level instead of three. |
| Distributed BFS traffic | Remote vertices were collected repeatedly, sorted per worker, and resent on later levels | Track already-sent remote vertices with a bitset reset for each BFS source. Remove worker-side sorting; retain coordinator deduplication across workers. |
| Intact-ring distances | Every source scanned every local vertex despite a closed-form distance function | Sum contiguous cyclic distance intervals with an arithmetic prefix formula in constant time per shard. |
| Dense rewiring correctness | The availability guard rejected a vertex with exactly one remaining non-neighbor | Fix the guard in both implementations and test the last-target case. Dense seeded graphs affected by this bug intentionally change. |

Remote discovery suppression is safe because a first discovery is delivered before
the next expansion, and owners already reject duplicates. It adds approximately
`N/8` bytes per worker when there is more than one worker. This is a memory-for-traffic
tradeoff, not a fully sharded visited structure.

Core BFS buffer reuse and alternate queue/chunk scheduling were measured but not
retained: they did not improve these workloads. The original dynamically scheduled
per-source Rayon BFS remains in place.

## Measurements

Core timing medians use seven alternating before/after runs, seed 42, `p=0.05`,
four Rayon threads, and four logical partitions. Distributed medians use five
alternating runs through the local HTTP API, with 16 BFS samples and seed 42.

| Measurement | Before | After |
| --- | ---: | ---: |
| Core clustering: N=30,000, K=100 | 150.754 ms | 100.806 ms |
| Core total measured work: N=30,000, K=100, 16 samples | 375.444 ms | 319.770 ms |
| Core total measured work: N=100,000, K=10, 64 samples | 261.124 ms | 267.563 ms |
| Distributed total: N=100,000, K=10, p=0.05, one worker | 227 ms | 210 ms |
| Distributed total: same graph parameters, four workers | 252 ms | 217 ms |
| Cross-shard items: same four-worker run | 619,349 | 600,684 |
| Distributed total: N=100,000, K=10, p=0, four workers | 17 ms | 15 ms |
| Peak RSS: core N=1,000,000, K=10, one sample (single run) | 584,171,520 bytes | 330,268,672 bytes |

The larger-degree clustering measurement improved by 33%; the four-worker run by
14%; peak RSS by 43%. The default sparse core workload did **not** show a total-time
improvement (about 2.5% slower in this sample). Core BFS itself is unchanged; timing
variation and graph allocation layout can affect its measured runtime. More workers
still do not beat one worker on this small distributed workload. Core total times
include both sequential and parallel BFS; medians of totals need not equal sums of
individual phase medians.

Reproduce core runs with each saved release binary:

```sh
./small-world-ws demo --nodes 100000 --degree 10 --samples 64 --threads 4
./small-world-ws demo --nodes 30000 --degree 100 --samples 16 --threads 4
/usr/bin/time -l ./small-world-ws demo --nodes 1000000 --samples 1 --threads 4
```

For distributed measurements, start each binary with `serve --port PORT`, POST
`{"nodes":100000,"degree":10,"probability":0.05,"bfsSamples":16,"workers":4,"seed":42}`
to `/api/run`, and read `/api/status` until completion. Vary workers between 1 and
4 and probability between 0 and 0.05. Avoid concurrent benchmark runs.

## Correctness checks

Workspace tests, core tests without the parallel feature, Clippy with warnings
denied, and formatting checks pass. Tests cover generation across the proposal
batch boundary with one and four threads, graph symmetry and edge counts,
clustering against a pairwise reference, disconnected and repeated BFS sources,
duplicate discoveries, remote-state reset, bounded mutation/query streaming under
pipe backpressure, and ring prefix sums against enumeration across all sources in
small odd/even rings and uneven shards.

Before/after distributed runs returned exactly equal path lengths, clustering
coefficients, visited counts, rewired counts, and adjacency counts for each fixed
configuration. Core CLI graph metrics also matched at displayed precision.

## Remaining scaling limits

- Rewiring authority remains serial by design: running shard rewiring concurrently
  without conflict resolution would invalidate duplicate prevention and canonical
  semantics. Endpoint updates remain streamed in bounded batches.
- Distributed clustering still enumerates neighbor pairs and processes one origin
  shard at a time. Read-only query concurrency is a possible follow-up, but requires
  multiplexed responses or a separate protocol phase to avoid pipe deadlocks.
- BFS still routes through one coordinator. It materializes each level's responses;
  discovery frames are not chunked and remain subject to the 256 MiB frame limit.
  Deduplication reduces traffic but does not remove this very-large-graph limit.
- Mutable per-vertex adjacency allocations and bincode payload allocations remain.
  CSR after rewiring could reduce traversal memory and improve locality, but needs
  separate measurements, especially on larger graphs and higher worker counts.
