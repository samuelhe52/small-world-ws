export type RunStatus = "idle" | "running" | "complete" | "failed";
export type PhaseStatus = "pending" | "active" | "complete" | "error";

export interface RunConfig {
  nodes: number;
  degree: number;
  probability: number;
  bfsSamples: number;
  workers: number;
  seed: number;
}

export interface PhaseView {
  key: string;
  label: string;
  status: PhaseStatus;
  progress: number;
  detail: string;
  elapsedMs: number;
}

export interface BfsView {
  sourceIndex: number;
  sourceTotal: number;
  level: number;
  frontierByWorker: number[];
}

export interface RunResults {
  averagePathLength: number;
  clusteringCoefficient: number;
  elapsedMs: number;
  crossShardMessages: number;
  verticesVisited: number;
  rewiredEdges: number;
  clusteringSamples: number;
  adjacencyEntries: number;
}

export interface RunRecord {
  id: number;
  finishedAtMs: number;
  config: RunConfig;
  results: RunResults;
}

export interface DashboardState {
  status: RunStatus;
  workersOnline: number;
  config: RunConfig;
  phases: PhaseView[];
  currentBfs: BfsView | null;
  results: RunResults | null;
  history: RunRecord[];
  error: string | null;
}

