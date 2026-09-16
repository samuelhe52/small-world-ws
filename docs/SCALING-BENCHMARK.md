# Large-scale worker and BFS benchmark

These measurements compare the shared-memory Rayon path with the isolated
worker-process path from 100,000 through 10 million nodes. They also measure
the effect of increasing the sampled BFS count from 16 to 64 at the larger
scales.

The runs were completed on September 16, 2026, on an Apple M1 Pro host with
16 GiB of RAM and eight logical CPUs. They used release builds, `K=10`,
`p=0.05`, seed `42`, and eight logical graph partitions. The 5m/10m ladder
used one run per concurrency setting. The earlier small-scale medians used the
repetition counts stated in their section. The values are local observations,
not scaling guarantees.

## Results

The in-process command reports measured work for graph generation, parallel
clustering, a sequential sampled-BFS validation, and parallel sampled BFS. The
worker command reports the complete distributed API run, including process
startup, coordination, rewiring, clustering, and BFS. Because the commands do
not measure identical phases, compare the tables as implementation profiles,
not as a strict end-to-end replacement benchmark.

### Earlier small-scale ladder

These runs used the same `K=10`, `p=0.05`, seed `42`, eight logical
partitions, and 16 BFS samples. The in-process values are medians of three
runs, and the worker values are medians of two runs. Times are seconds. The
in-process columns are Rayon thread counts, and the worker columns are
worker-process counts.

|     Nodes | In-process 1 | In-process 4 | In-process 6 | In-process 8 | Workers 1 | Workers 4 | Workers 6 | Workers 8 |
| --------: | -----------: | -----------: | -----------: | -----------: | --------: | --------: | --------: | --------: |
|   100,000 |        0.175 |        0.059 |        0.049 |        0.045 |     0.189 |     0.210 |     0.224 |     0.252 |
|   500,000 |        1.131 |        0.382 |        0.309 |        0.294 |     1.086 |     0.756 |     0.761 |     0.792 |
| 1,000,000 |        2.382 |        0.811 |        0.680 |        0.616 |     2.355 |     1.472 |     1.441 |     1.519 |

The in-process medians were 174.536, 58.634, 48.691, and 44.962 ms at
100,000 nodes; 1,130.829, 382.235, 309.422, and 294.268 ms at 500,000
nodes; and 2,381.884, 810.845, 680.263, and 615.669 ms at 1,000,000 nodes.
The worker medians were collected through the local HTTP API: 189, 210, 224,
and 252 ms; 1,086, 756, 761, and 792 ms; and 2,355, 1,472, 1,441, and 1,519
ms, respectively.

### In-process Rayon path

Times are seconds. Columns are Rayon thread counts.

|      Nodes | BFS samples |       1 |      4 |      6 |      8 |
| ---------: | ----------: | ------: | -----: | -----: | -----: |
|  5,000,000 |          16 |  20.711 | 11.446 | 10.823 | 11.491 |
|  5,000,000 |          64 |  61.418 | 37.635 | 51.067 | 35.107 |
| 10,000,000 |          16 |  42.811 | 29.211 | 24.484 | 24.620 |
| 10,000,000 |          64 | 119.379 | 68.336 | 68.717 | 70.431 |

The 10-million-node, one-thread run takes 42.811 seconds of measured work with
16 samples and 119.379 seconds with 64 samples. Wall time was approximately
44.8 and 121.4 seconds, respectively. Peak RSS was about 2.8 GiB for the
16-sample run and 2.8 GiB for the 64-sample run.

### Isolated worker-process path

Times are seconds. Columns are worker-process counts.

|      Nodes | BFS samples |      1 |      4 |      6 |      8 |
| ---------: | ----------: | -----: | -----: | -----: | -----: |
|  5,000,000 |          16 | 14.648 |  7.474 |  7.083 |  7.145 |
|  5,000,000 |          64 | 38.173 | 15.006 | 13.427 | 13.470 |
| 10,000,000 |          16 | 28.841 | 15.494 | 14.612 | 14.572 |
| 10,000,000 |          64 | 81.694 | 29.155 | 24.815 | 25.648 |

Cross-shard protocol items increased with worker count. At 10 million nodes,
the 16-sample runs sent 56.3 million, 62.8 million, and 66.1 million items
with 4, 6, and 8 workers. The 64-sample runs sent 219.6 million, 245.0
million, and 257.8 million items.

## Interpretation

Six workers were the fastest distributed setting in the large runs. Eight
workers did not improve the result: at 10 million nodes and 64 samples, six
workers took 24.815 seconds and eight took 25.648 seconds.

**The worker totals should not be compared directly with the in-process totals as
if they represented the same work.** The in-process command includes a
sequential BFS validation and computes clustering across the full graph. The
distributed path samples clustering at up to 20,000 vertices and does not run
the sequential validation pass. For example, at 10 million nodes and 16
samples, the 8-thread in-process run took 24.620 seconds in total, but its
generation, clustering, and parallel-BFS phases sum to 9.546 seconds before
the sequential validation is included. The corresponding 8-worker distributed
run took 14.572 seconds.

The 64-sample axis increases the BFS work by roughly four times. The distributed
path benefits from reusing its built graph and rewiring cost, but its
coordinator and cross-shard traffic remain visible. The benchmark therefore
supports shared-memory Rayon for equivalent read-heavy work and six isolated
workers when the distributed ownership model is required.

## Reproduction

Build the release binary before running the in-process commands:

```sh
cargo build --release
```

Run one in-process measurement with the requested node count, BFS sample count,
and thread count:

```sh
target/release/small-world-ws demo \
  --nodes 10000000 \
  --degree 10 \
  --probability 0.05 \
  --samples 64 \
  --threads 8 \
  --partitions 8 \
  --seed 42
```

For the worker path, build the frontend, start the server, and POST a JSON
configuration to `/api/run`. The `workers` field accepts the process count and
`bfsSamples` selects the BFS axis:

```json
{
  "nodes": 10000000,
  "degree": 10,
  "probability": 0.05,
  "bfsSamples": 64,
  "workers": 6,
  "seed": 42
}
```

Poll `/api/status` until its `status` is `complete`, then read
`results.elapsedMs`, `results.crossShardMessages`, and the phase timings.
