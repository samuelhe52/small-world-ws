import type { DashboardState, RunConfig } from "./types";

async function checked(response: Response): Promise<Response> {
  if (response.ok) return response;
  const payload = (await response.json().catch(() => null)) as
    | { error?: string }
    | null;
  throw new Error(payload?.error ?? `Request failed (${response.status})`);
}

export async function getStatus(signal?: AbortSignal): Promise<DashboardState> {
  const response = await checked(await fetch("/api/status", { signal }));
  return response.json() as Promise<DashboardState>;
}

export async function startRun(config: RunConfig): Promise<void> {
  await checked(
    await fetch("/api/run", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(config),
    }),
  );
}

