import { useEffect, useRef, useState } from "react";
import type { RunConfig } from "../types";

interface Props {
  config: RunConfig;
  workerLimit: number;
  running: boolean;
  onChange: (config: RunConfig) => void;
  onRun: () => void;
}

interface NumberFieldProps {
  label: string;
  value: number;
  min: number;
  max?: number;
  step?: number;
  className?: string;
  id?: string;
  disabled: boolean;
  onChange: (value: number) => void;
}

function DraftNumberInput({
  value,
  min,
  max,
  step = 1,
  className,
  id,
  disabled,
  onChange,
}: Omit<NumberFieldProps, "label">) {
  const [draft, setDraft] = useState(String(value));
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (document.activeElement !== inputRef.current) {
      setDraft(String(value));
    }
  }, [value]);

  return (
    <input
      ref={inputRef}
      id={id}
      className={className}
      type="number"
      value={draft}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      onChange={(event) => {
        const next = event.target.value;
        setDraft(next);
        if (next === "") return;
        const parsed = Number(next);
        if (Number.isFinite(parsed)) onChange(parsed);
      }}
      onBlur={() => setDraft(String(value))}
    />
  );
}

function NumberField({ label, ...inputProps }: NumberFieldProps) {
  return (
    <label className="field">
      <span>{label}</span>
      <DraftNumberInput {...inputProps} />
    </label>
  );
}

export function ConfigurationPanel({ config, workerLimit, running, onChange, onRun }: Props) {
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
          <label htmlFor="probability-number">Rewiring probability (p)</label>
          <DraftNumberInput
            id="probability-number"
            className="probability-number"
            value={config.probability}
            min={0}
            max={1}
            step={0.001}
            disabled={running}
            onChange={(value) => update("probability", value)}
          />
        </div>
        <input
          id="probability"
          aria-label="Rewiring probability slider"
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
        min={1}
        max={workerLimit}
        disabled={running}
        onChange={(value) => update("workers", value)}
      />
      <button className="run-button" type="button" disabled={running} onClick={onRun}>
        {running ? <span className="spinner" aria-hidden="true" /> : null}
        {running ? "Running…" : "Run experiment"}
      </button>
      <p className="configuration-note">
        Each worker is a separate OS process and stores only its assigned node range. This system
        allows up to {workerLimit} worker {workerLimit === 1 ? "process" : "processes"}.
      </p>
    </aside>
  );
}
