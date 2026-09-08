export type Memory = {
  id: string;
  tenant_id: string;
  tier: string;
  content: string;
  metadata: unknown;
  created_at: string;
};

export type Settings = { serverUrl: string; token: string };

class ApiError extends Error {}

async function call<T>(settings: Settings, path: string): Promise<T> {
  const url = settings.serverUrl.replace(/\/+$/, "") + path;
  const res = await fetch(url, {
    headers: { Authorization: `Bearer ${settings.token}` },
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new ApiError(`${res.status} ${res.statusText}: ${body}`);
  }
  return res.json() as Promise<T>;
}

// Seeds the graph's node set. There is no full-graph endpoint — this is
// every memory in the caller's tenant, unranked (unlike `recall`, which
// needs a query embedding this tool has no way to produce for the user).
export function listMemories(settings: Settings): Promise<{ results: Memory[]; total: number }> {
  return call(settings, "/v1/memories");
}

// One-hop neighbor ids only — z3rno has no multi-hop graph traversal at
// any layer today. Expanding further means calling this again per node.
export function fetchNeighbors(settings: Settings, id: string): Promise<{ neighbors: string[] }> {
  return call(settings, `/v1/memories/${id}/neighbors`);
}
