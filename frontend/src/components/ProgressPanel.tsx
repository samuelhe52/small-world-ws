import type { BfsView, PhaseView } from "../types";

function formatDuration(milliseconds: number): string {
  if (milliseconds < 1000) return `${milliseconds} ms`;
  return `${(milliseconds / 1000).toFixed(1)} s`;
}

function PhaseRow({ phase, index }: { phase: PhaseView; index: number }) {
  return (
    <div className={`phase-row phase-${phase.status}`}>
      <div className="phase-index" aria-hidden="true">
        {phase.status === "complete" ? "✓" : index + 1}
      </div>
      <div className="phase-copy">
        <strong>{phase.label}</strong>
        <span>{phase.detail}</span>
      </div>
      <div
        className="phase-meter"
        role="progressbar"
        aria-label={`${phase.label} progress`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={phase.progress * 100}
      >
        <div style={{ width: `${phase.progress * 100}%` }} />
      </div>
      <div className="phase-meta">
        <span className="phase-percent">{Math.round(phase.progress * 100)}%</span>
        <span className="phase-time">{formatDuration(phase.elapsedMs)}</span>
      </div>
    </div>
  );
}

function WorkerActivity({ bfs, workers, active }: {
  bfs: BfsView | null;
  workers: number;
  active: boolean;
}) {
  const frontiers = bfs?.frontierByWorker ?? Array.from({ length: workers }, () => 0);
  const max = Math.max(...frontiers, 1);
  let status = "Waiting for BFS phase";
  if (bfs?.method === "ring") status = "Exact ring distances";
  else if (bfs) status = active ? "Exchanging frontier" : "Complete";
  return (
    <section className="bfs-status" aria-labelledby="bfs-status-title">
      <div className="bfs-status-heading">
        <h3 id="bfs-status-title">Current BFS status</h3>
        <div>
          <span>Source {bfs ? `${bfs.sourceIndex} of ${bfs.sourceTotal}` : "—"}</span>
          <i />
          <span>Level {bfs?.level ?? "—"}</span>
          <i />
          <span>{status}</span>
        </div>
      </div>
      {bfs ? (
        <div className="source-progress">
          <div>
            <span>Current source: {bfs.visitedNodes.toLocaleString()} / {bfs.totalNodes.toLocaleString()} vertices reached</span>
            <strong>{Math.round(bfs.sourceProgress * 100)}%</strong>
          </div>
          <progress aria-label="Current BFS source progress" max={1} value={bfs.sourceProgress} />
        </div>
      ) : null}
      <p className="worker-chart-explanation">
        Each lane is one worker. Its blue bars show the size of that worker’s current BFS frontier
        relative to the busiest worker at this level; they indicate live traversal activity, not a
        history, CPU utilization, or total work completed. The p=0 baseline uses exact ring distances
        and has no BFS frontier.
      </p>
      <div className="worker-grid">
        {frontiers.map((frontier, index) => {
          const ratio = frontier / max;
          return (
            <div className="worker-lane" key={index}>
              <strong>Worker {index}</strong>
              <span>{frontier.toLocaleString()} nodes in frontier</span>
              <div className="activity-bars" aria-hidden="true">
                {Array.from({ length: 11 }, (_, bar) => {
                  return (
                    <i
                      key={bar}
                      style={{ height: `${52 * ratio}px` }}
                    />
                  );
                })}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

interface Props {
  phases: PhaseView[];
  bfs: BfsView | null;
  workers: number;
}

export function ProgressPanel({ phases, bfs, workers }: Props) {
  return (
    <main className="panel progress-panel" aria-labelledby="progress-title">
      <h2 id="progress-title">Experiment progress</h2>
      <div className="phase-list">
        {phases.map((phase, index) => (
          <PhaseRow key={phase.key} phase={phase} index={index} />
        ))}
      </div>
      <WorkerActivity
        bfs={bfs}
        workers={workers}
        active={phases.some((phase) => phase.key === "bfs" && phase.status === "active")}
      />
    </main>
  );
}
