import type { RunConfig, RunResults } from "../types";

function formatDuration(milliseconds: number): string {
  const seconds = milliseconds / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  return `${Math.floor(seconds / 60)}m ${(seconds % 60).toFixed(1)}s`;
}

function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div className="result-detail">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

interface Props {
  results: RunResults | null;
  config: RunConfig;
}

export function ResultsPanel({ results, config }: Props) {
  return (
    <aside className="panel results-panel" aria-labelledby="results-title">
      <h2 id="results-title">Results</h2>
      <div className="primary-results">
        <div>
          <span className="metric-symbol">L</span>
          <strong className="metric-value metric-l">
            {results ? results.averagePathLength.toFixed(3) : "—"}
          </strong>
          <span>Average path length</span>
        </div>
        <div>
          <span className="metric-symbol">C</span>
          <strong className="metric-value metric-c">
            {results ? results.clusteringCoefficient.toFixed(3) : "—"}
          </strong>
          <span>Sampled clustering coefficient</span>
        </div>
      </div>
      <h3>Run details</h3>
      <div className="result-details">
        <Detail label="Elapsed time" value={results ? formatDuration(results.elapsedMs) : "—"} />
        <Detail
          label="Cross-shard messages"
          value={results ? results.crossShardMessages.toLocaleString() : "—"}
        />
        <Detail
          label="Vertices visited (BFS)"
          value={results ? results.verticesVisited.toLocaleString() : "—"}
        />
        <Detail label="BFS samples" value={String(config.bfsSamples)} />
        <Detail label="Worker processes" value={String(config.workers)} />
        <Detail
          label="Graph size (N, K, p)"
          value={`${config.nodes.toLocaleString()}, ${config.degree}, ${config.probability}`}
        />
      </div>
    </aside>
  );
}

