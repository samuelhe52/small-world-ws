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
      <div className="phase-meter" aria-label={`${phase.label} progress`}>
        <div style={{ width: `${phase.progress * 100}%` }} />
      </div>
      <span className="phase-percent">{Math.round(phase.progress * 100)}%</span>
      <span className="phase-time">{formatDuration(phase.elapsedMs)}</span>
    </div>
  );
}

function WorkerActivity({ bfs, workers }: { bfs: BfsView | null; workers: number }) {
  const frontiers = bfs?.frontierByWorker ?? Array.from({ length: workers }, () => 0);
  const max = Math.max(...frontiers, 1);
  return (
    <section className="bfs-status" aria-labelledby="bfs-status-title">
      <div className="bfs-status-heading">
        <h3 id="bfs-status-title">Current BFS status</h3>
        <div>
          <span>Source {bfs ? `${bfs.sourceIndex} of ${bfs.sourceTotal}` : "—"}</span>
          <i />
          <span>Level {bfs?.level ?? "—"}</span>
          <i />
          <span>{bfs ? "Exchanging frontier" : "Waiting for BFS phase"}</span>
        </div>
      </div>
      <div className="worker-grid">
        {frontiers.map((frontier, index) => {
          const ratio = frontier / max;
          return (
            <div className="worker-lane" key={index}>
              <strong>Worker {index}</strong>
              <span>{frontier.toLocaleString()} nodes in frontier</span>
              <div className="activity-bars" aria-hidden="true">
                {Array.from({ length: 11 }, (_, bar) => {
                  const profile = 0.28 + Math.abs(Math.sin((bar + 1) * (index + 2))) * 0.72;
                  return (
                    <i
                      key={bar}
                      style={{ height: `${10 + 42 * profile * Math.max(ratio, 0.08)}px` }}
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
      <WorkerActivity bfs={bfs} workers={workers} />
    </main>
  );
}

