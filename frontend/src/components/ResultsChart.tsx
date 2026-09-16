import type { RunRecord } from "../types";

interface Point {
  p: number;
  l: number;
  c: number;
}

const WIDTH = 900;
const HEIGHT = 280;
const LEFT = 80;
const RIGHT = 80;
const TOP = 32;
const BOTTOM = 44;

function axisScale(points: Point[], key: "l" | "c") {
  const maximum = points.reduce((largest, point) => Math.max(largest, point[key]), 0);
  const roughStep = (maximum || 1) / 4;
  const magnitude = 10 ** Math.floor(Math.log10(roughStep));
  const fraction = roughStep / magnitude;
  const multiple = [1, 2, 2.5, 5, 10].find((candidate) => candidate >= fraction) ?? 10;
  const step = multiple * magnitude;
  return { maximum: step * 4, step };
}

function tickLabel(value: number, step: number): string {
  if (value !== 0 && Math.abs(value) < 0.0001) return value.toExponential(1);
  const decimals = Math.max(0, 1 - Math.floor(Math.log10(step)));
  return value.toLocaleString(undefined, { maximumFractionDigits: Math.min(10, decimals) });
}

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
  const scaleL = axisScale(points, "l");
  const scaleC = axisScale(points, "c");

  return (
    <section className="panel chart-panel" aria-labelledby="chart-title">
      <div className="chart-heading">
        <div>
          <h2 id="chart-title">L and C vs. rewiring probability p</h2>
          <p>Completed runs with matching N, K, samples, and workers. Actual values; axes scale automatically from zero.</p>
        </div>
        <div className="legend" aria-label="Chart legend">
          <span className="legend-l">L (left axis)</span>
          <span className="legend-c">C (right axis)</span>
        </div>
      </div>
      {points.length === 0 ? (
        <div className="chart-empty">Run an experiment to add the first observation.</div>
      ) : (
        <div className="chart-scroll">
          <svg className="results-chart" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-labelledby="plot-title plot-description">
            <title id="plot-title">Average path length and clustering coefficient by p</title>
            <desc id="plot-description">Actual average path length L on the blue left axis and clustering coefficient C on the orange right axis. Both axes start at zero and scale independently to the completed runs.</desc>
            <text x={LEFT} y={14} textAnchor="end" className="axis-label axis-l">L</text>
            <text x={WIDTH - RIGHT} y={14} textAnchor="start" className="axis-label axis-c">C</text>
            {[0, 0.25, 0.5, 0.75, 1].map((tick) => {
              const y = yPosition(tick, 1);
              return (
                <g key={`y-${tick}`}>
                  <line x1={LEFT} y1={y} x2={WIDTH - RIGHT} y2={y} className="grid-line" />
                  <text x={LEFT - 12} y={y + 4} textAnchor="end" className="axis-text axis-l">
                    {tickLabel(tick * scaleL.maximum, scaleL.step)}
                  </text>
                  <text x={WIDTH - RIGHT + 12} y={y + 4} textAnchor="start" className="axis-text axis-c">
                    {tickLabel(tick * scaleC.maximum, scaleC.step)}
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
            <path d={path(points, "l", scaleL.maximum)} className="line line-l" />
            <path d={path(points, "c", scaleC.maximum)} className="line line-c" />
            {points.map((point, index) => (
              <g key={`${point.p}-${index}`}>
                <circle cx={xPosition(point.p)} cy={yPosition(point.l, scaleL.maximum)} r="6" className="dot-l">
                  <title>{`p = ${point.p}, L = ${point.l}`}</title>
                </circle>
                <circle cx={xPosition(point.p)} cy={yPosition(point.c, scaleC.maximum)} r="3.5" className="dot-c">
                  <title>{`p = ${point.p}, C = ${point.c}`}</title>
                </circle>
              </g>
            ))}
            <text x={WIDTH / 2} y={HEIGHT - 1} textAnchor="middle" className="axis-label">
              Rewiring probability p
            </text>
          </svg>
        </div>
      )}
    </section>
  );
}
