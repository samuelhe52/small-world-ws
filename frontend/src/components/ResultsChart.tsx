import type { RunRecord } from "../types";

interface Point {
  p: number;
  l: number;
  c: number;
}

const WIDTH = 900;
const HEIGHT = 280;
const LEFT = 60;
const RIGHT = 24;
const TOP = 24;
const BOTTOM = 44;

function xPosition(p: number): number {
  return LEFT + p * (WIDTH - LEFT - RIGHT);
}

function yPosition(value: number, maximum: number): number {
  const usable = HEIGHT - TOP - BOTTOM;
  return TOP + usable * (1 - value / maximum);
}

function path(points: Point[], key: "l" | "c", maximum: number): string {
  return points
    .map((point, index) => {
      const x = xPosition(point.p);
      const y = yPosition(point[key], maximum);
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

export function ResultsChart({ history }: { history: RunRecord[] }) {
  const latest = history[history.length - 1];
  const comparableRuns = latest
    ? history.filter(
        (record) =>
          record.config.nodes === latest.config.nodes &&
          record.config.degree === latest.config.degree &&
          record.config.bfsSamples === latest.config.bfsSamples &&
          record.config.workers === latest.config.workers,
      )
    : [];
  const points = comparableRuns
    .map((record) => ({
      p: record.config.probability,
      l: record.results.averagePathLength,
      c: record.results.clusteringCoefficient,
    }))
    .sort((a, b) => a.p - b.p);
  const maxL = Math.max(...points.map((point) => point.l), Number.EPSILON);
  const maxC = Math.max(...points.map((point) => point.c), Number.EPSILON);

  return (
    <section className="panel chart-panel" aria-labelledby="chart-title">
      <div className="chart-heading">
        <div>
          <h2 id="chart-title">L and C vs. rewiring probability p</h2>
          <p>Completed runs with matching N, K, samples, and workers; each series is normalized.</p>
        </div>
        <div className="legend" aria-label="Chart legend">
          <span className="legend-l">L / max L</span>
          <span className="legend-c">C / max C</span>
        </div>
      </div>
      {points.length === 0 ? (
        <div className="chart-empty">Run an experiment to add the first observation.</div>
      ) : (
        <svg className="results-chart" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img">
          <title>Normalized average path length and clustering coefficient by p</title>
          {[0, 0.25, 0.5, 0.75, 1].map((tick) => {
            const y = yPosition(tick, 1);
            return (
              <g key={`y-${tick}`}>
                <line x1={LEFT} y1={y} x2={WIDTH - RIGHT} y2={y} className="grid-line" />
                <text x={LEFT - 12} y={y + 4} textAnchor="end" className="axis-text">
                  {tick.toFixed(2)}
                </text>
              </g>
            );
          })}
          {[0, 0.25, 0.5, 0.75, 1].map((tick) => {
            const x = xPosition(tick);
            return (
              <g key={`x-${tick}`}>
                <line x1={x} y1={TOP} x2={x} y2={HEIGHT - BOTTOM} className="grid-line" />
                <text x={x} y={HEIGHT - 17} textAnchor="middle" className="axis-text">
                  {tick.toFixed(2)}
                </text>
              </g>
            );
          })}
          <path d={path(points, "l", maxL)} className="line line-l" />
          <path d={path(points, "c", maxC)} className="line line-c" />
          {points.map((point, index) => (
            <g key={`${point.p}-${index}`}>
              <circle cx={xPosition(point.p)} cy={yPosition(point.l, maxL)} r="6" className="dot-l" />
              <circle cx={xPosition(point.p)} cy={yPosition(point.c, maxC)} r="3.5" className="dot-c" />
            </g>
          ))}
          <text x={WIDTH / 2} y={HEIGHT - 1} textAnchor="middle" className="axis-label">
            Rewiring probability p
          </text>
        </svg>
      )}
    </section>
  );
}
