import type { RunConfig } from "../types";

interface Props {
  config: RunConfig;
  running: boolean;
  onChange: (config: RunConfig) => void;
  onRun: () => void;
}

interface NumberFieldProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled: boolean;
  onChange: (value: number) => void;
}

function NumberField({
  label,
  value,
  min,
  max,
  step = 1,
  disabled,
  onChange,
}: NumberFieldProps) {
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </label>
  );
}

export function ConfigurationPanel({ config, running, onChange, onRun }: Props) {
  const update = <K extends keyof RunConfig>(key: K, value: RunConfig[K]) => {
    onChange({ ...config, [key]: value });
  };

  return (
    <aside className="panel configuration" aria-labelledby="configuration-title">
      <h2 id="configuration-title">Configuration</h2>
      <NumberField
        label="Nodes (N)"
        value={config.nodes}
        min={10_000}
        max={5_000_000}
        step={10_000}
        disabled={running}
        onChange={(value) => update("nodes", value)}
      />
      <NumberField
        label="Degree (K)"
        value={config.degree}
        min={2}
        max={100}
        step={2}
        disabled={running}
        onChange={(value) => update("degree", value)}
      />
      <div className="field probability-field">
        <div className="field-label-row">
          <label htmlFor="probability">Rewiring probability (p)</label>
          <input
            className="probability-number"
            type="number"
            value={config.probability}
            min={0}
            max={1}
            step={0.001}
            disabled={running}
            onChange={(event) => update("probability", Number(event.target.value))}
          />
        </div>
        <input
          id="probability"
          className="range"
          type="range"
          value={config.probability}
          min={0}
          max={1}
          step={0.001}
          disabled={running}
          onChange={(event) => update("probability", Number(event.target.value))}
        />
        <div className="range-scale" aria-hidden="true">
          <span>0</span>
          <span>0.25</span>
          <span>0.5</span>
          <span>0.75</span>
          <span>1</span>
        </div>
      </div>
      <NumberField
        label="BFS samples"
        value={config.bfsSamples}
        min={1}
        max={256}
        disabled={running}
        onChange={(value) => update("bfsSamples", value)}
      />
      <NumberField
        label="Worker processes"
        value={config.workers}
        min={2}
        max={8}
        disabled={running}
        onChange={(value) => update("workers", value)}
      />
      <button className="run-button" type="button" disabled={running} onClick={onRun}>
        {running ? <span className="spinner" aria-hidden="true" /> : null}
        {running ? "Running…" : "Run experiment"}
      </button>
      <p className="configuration-note">
        Each worker is a separate OS process and stores only its assigned node range.
      </p>
    </aside>
  );
}

