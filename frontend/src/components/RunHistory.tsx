import type { RunRecord } from "../types";

function duration(milliseconds: number): string {
  const seconds = milliseconds / 1000;
  return seconds < 60 ? `${seconds.toFixed(1)}s` : `${Math.floor(seconds / 60)}m`;
}

export function RunHistory({ history }: { history: RunRecord[] }) {
  const rows = [...history].reverse().slice(0, 6);
  return (
    <section className="panel history-panel" aria-labelledby="history-title">
      <h2 id="history-title">Run history</h2>
      <div className="history-scroll">
        <table>
          <thead>
            <tr>
              <th>#</th>
              <th>Date &amp; time</th>
              <th>N</th>
              <th>K</th>
              <th>p</th>
              <th>L</th>
              <th>C</th>
              <th>Time</th>
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 ? (
              <tr>
                <td colSpan={8} className="empty-row">No completed runs yet.</td>
              </tr>
            ) : (
              rows.map((record) => (
                <tr key={record.id}>
                  <td>{record.id}</td>
                  <td>{new Date(record.finishedAtMs).toLocaleString([], { dateStyle: "short", timeStyle: "short" })}</td>
                  <td>{record.config.nodes >= 1_000_000 ? `${record.config.nodes / 1_000_000}M` : `${record.config.nodes / 1000}K`}</td>
                  <td>{record.config.degree}</td>
                  <td>{record.config.probability}</td>
                  <td>{record.results.averagePathLength.toFixed(2)}</td>
                  <td>{record.results.clusteringCoefficient.toFixed(3)}</td>
                  <td>{duration(record.results.elapsedMs)}</td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}
