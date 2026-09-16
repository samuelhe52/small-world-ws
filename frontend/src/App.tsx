import { useCallback, useEffect, useRef, useState } from "react";
import { getStatus, startRun } from "./api";
import { ConfigurationPanel } from "./components/ConfigurationPanel";
import { ProgressPanel } from "./components/ProgressPanel";
import { ResultsChart } from "./components/ResultsChart";
import { ResultsPanel } from "./components/ResultsPanel";
import { RunHistory } from "./components/RunHistory";
import type { DashboardState, RunConfig, SweepProgress } from "./types";

const DEFAULT_CONFIG: RunConfig = {
  nodes: 1_000_000,
  degree: 10,
  probability: 0.05,
  bfsSamples: 32,
  workers: 4,
  seed: 42,
};

function sweepProbabilities(step: number): number[] {
  const probabilities: number[] = [];
  for (let index = 0; index * step < 1; index += 1) {
    probabilities.push(Number((index * step).toFixed(12)));
  }
  probabilities.push(1);
  return probabilities;
}

export function App() {
  const [dashboard, setDashboard] = useState<DashboardState | null>(null);
  const [config, setConfig] = useState<RunConfig>(DEFAULT_CONFIG);
  const [requestError, setRequestError] = useState<string | null>(null);
  const [sweepRunning, setSweepRunning] = useState(false);
  const [sweepProgress, setSweepProgress] = useState<SweepProgress | null>(null);
  const hydrated = useRef(false);

  const refresh = useCallback(async (signal?: AbortSignal) => {
    try {
      const next = await getStatus(signal);
      setDashboard(next);
      if (!hydrated.current) {
        setConfig(next.config);
        hydrated.current = true;
      }
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") return;
      setRequestError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    void refresh(controller.signal);
    const timer = window.setInterval(() => void refresh(), 400);
    return () => {
      controller.abort();
      window.clearInterval(timer);
    };
  }, [refresh]);

  const run = async () => {
    setRequestError(null);
    try {
      await startRun(config);
      await refresh();
    } catch (error) {
      setRequestError(error instanceof Error ? error.message : String(error));
    }
  };

  const waitForRun = async (): Promise<DashboardState> => {
    while (true) {
      const next = await getStatus();
      setDashboard(next);
      if (next.status !== "running") return next;
      await new Promise<void>((resolve) => window.setTimeout(resolve, 400));
    }
  };

  const runSweep = async (step: number) => {
    setRequestError(null);
    setSweepRunning(true);
    try {
      const probabilities = sweepProbabilities(step);
      for (const [index, probability] of probabilities.entries()) {
        setSweepProgress({ index: index + 1, total: probabilities.length, probability });
        const sweepConfig = { ...config, probability };
        setConfig(sweepConfig);
        await startRun(sweepConfig);
        const completed = await waitForRun();
        if (completed.status === "failed") {
          throw new Error(completed.error ?? `The p=${probability} experiment failed.`);
        }
      }
    } catch (error) {
      setRequestError(error instanceof Error ? error.message : String(error));
    } finally {
      setSweepRunning(false);
      setSweepProgress(null);
      await refresh();
    }
  };

  const running = dashboard?.status === "running" || sweepRunning;
  const effective = dashboard && running ? dashboard.config : config;
  const workerLimit = dashboard?.workerLimit ?? Math.max(1, navigator.hardwareConcurrency || 1);
  const error = requestError ?? dashboard?.error;

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand-group">
          <h1>Small World Lab</h1>
          <span>Distributed experiment console</span>
        </div>
        <div className="connection-status">
          <i className={dashboard?.workersOnline ? "online" : ""} />
          {dashboard?.workersOnline ?? 0} workers online
        </div>
      </header>
      {error ? <div className="error-banner" role="alert">{error}</div> : null}
      <div className="primary-grid">
        <ConfigurationPanel
          config={effective}
          workerLimit={workerLimit}
          running={running}
          onChange={setConfig}
          onRun={() => void run()}
          onRunSweep={(step) => void runSweep(step)}
          sweepProgress={sweepProgress}
          experimentProgress={dashboard?.status === "running"
            ? dashboard.phases.reduce((sum, phase) => sum + phase.progress, 0) / Math.max(dashboard.phases.length, 1)
            : 0}
        />
        <ProgressPanel
          phases={dashboard?.phases ?? []}
          bfs={dashboard?.currentBfs ?? null}
          workers={effective.workers}
        />
        <ResultsPanel results={dashboard?.results ?? null} config={effective} />
      </div>
      <div className="secondary-grid">
        <ResultsChart history={dashboard?.history ?? []} />
        <RunHistory history={dashboard?.history ?? []} />
      </div>
      <footer>
        Graph adjacency lives only inside worker processes; the coordinator routes protocol messages and stores progress metadata.
      </footer>
    </div>
  );
}
