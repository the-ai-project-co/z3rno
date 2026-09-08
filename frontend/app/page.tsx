"use client";

import dynamic from "next/dynamic";
import { useEffect, useState } from "react";
import { fetchNeighbors, listMemories, type Memory, type Settings } from "@/lib/api";
import { loadSettings, saveSettings } from "@/lib/storage";

// react-force-graph-2d draws to a real <canvas> — no SSR-safe render path,
// so it's loaded client-side only.
const ForceGraph2D = dynamic(() => import("react-force-graph-2d"), { ssr: false });

type GraphNode = { id: string; memory: Memory };
type GraphLink = { source: string; target: string };

export default function Page() {
  const [settings, setSettings] = useState<Settings>({ serverUrl: "", token: "" });
  const [nodes, setNodes] = useState<GraphNode[]>([]);
  const [links, setLinks] = useState<GraphLink[]>([]);
  const [selected, setSelected] = useState<Memory | null>(null);
  const [status, setStatus] = useState<string>("");

  // localStorage only exists client-side — load after mount, not during
  // the initial render (which also runs server-side under Next's App
  // Router even for a "use client" page). This is exactly the "synchronize
  // with an external system" case the disabled lint rule's own guidance
  // calls out as legitimate; a lazy useState initializer would read
  // localStorage during SSR (where it doesn't exist) and mismatch on
  // hydration instead.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setSettings(loadSettings());
  }, []);

  async function loadGraph() {
    if (!settings.serverUrl || !settings.token) {
      setStatus("Set a server URL and token first.");
      return;
    }
    setStatus("Loading...");
    try {
      const { results } = await listMemories(settings);
      setNodes(results.map((memory) => ({ id: memory.id, memory })));
      setLinks([]);
      setStatus(`${results.length} memories loaded. Click a node to expand its neighbors.`);
    } catch (err) {
      setStatus(String(err));
    }
  }

  async function expandNode(id: string) {
    const node = nodes.find((n) => n.id === id);
    if (node) setSelected(node.memory);

    try {
      const { neighbors } = await fetchNeighbors(settings, id);
      setNodes((prev) => {
        const known = new Set(prev.map((n) => n.id));
        const missing = neighbors.filter((nid) => !known.has(nid));
        if (missing.length === 0) return prev;
        // A neighbor id came from the graph backend, not `/v1/memories` —
        // it may belong to a memory this tenant's list call didn't return
        // (e.g. paging, or a race with a concurrent write). Render it as a
        // stub node rather than silently dropping the edge.
        const stubs = missing.map((nid) => ({
          id: nid,
          memory: {
            id: nid,
            tenant_id: node?.memory.tenant_id ?? "",
            tier: "unknown",
            content: "(not in the loaded set — click Reload)",
            metadata: null,
            created_at: "",
          },
        }));
        return [...prev, ...stubs];
      });
      setLinks((prev) => {
        const existing = new Set(prev.map((l) => `${l.source}->${l.target}`));
        const added = neighbors
          .filter((nid) => !existing.has(`${id}->${nid}`))
          .map((nid) => ({ source: id, target: nid }));
        return [...prev, ...added];
      });
    } catch (err) {
      setStatus(String(err));
    }
  }

  return (
    <main style={{ display: "flex", height: "100vh", fontFamily: "system-ui, sans-serif" }}>
      <div style={{ flex: 1, position: "relative" }}>
        <ForceGraph2D
          graphData={{ nodes, links }}
          nodeId="id"
          // `next/dynamic` erases react-force-graph-2d's generic node type,
          // so its callbacks come through as the library's loose object
          // shape rather than `GraphNode` — the runtime object is exactly
          // what `graphData.nodes` was given, so this cast is safe.
          nodeLabel={(n: object) => (n as GraphNode).memory.content}
          onNodeClick={(n: object) => expandNode((n as GraphNode).id)}
          linkDirectionalArrowLength={4}
        />
      </div>

      <aside
        style={{
          width: 320,
          borderLeft: "1px solid #333",
          padding: 16,
          overflowY: "auto",
          display: "flex",
          flexDirection: "column",
          gap: 12,
        }}
      >
        <h1 style={{ fontSize: 16, margin: 0 }}>z3rno graph visualizer</h1>

        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          Server URL
          <input
            value={settings.serverUrl}
            onChange={(e) => setSettings((s) => ({ ...s, serverUrl: e.target.value }))}
            placeholder="http://localhost:8080"
          />
        </label>
        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          Auth token
          <input
            value={settings.token}
            onChange={(e) => setSettings((s) => ({ ...s, token: e.target.value }))}
            type="password"
            placeholder="superadmin API key or JWT"
          />
        </label>
        <button
          onClick={() => {
            saveSettings(settings);
            loadGraph();
          }}
        >
          Save & load
        </button>

        <p style={{ fontSize: 12, color: "#888" }}>{status}</p>

        {selected && (
          <div style={{ borderTop: "1px solid #333", paddingTop: 12 }}>
            <h2 style={{ fontSize: 14 }}>Selected memory</h2>
            <dl style={{ fontSize: 12 }}>
              <dt>id</dt>
              <dd>{selected.id}</dd>
              <dt>tier</dt>
              <dd>{selected.tier}</dd>
              <dt>content</dt>
              <dd>{selected.content}</dd>
              <dt>metadata</dt>
              <dd>
                <pre style={{ whiteSpace: "pre-wrap" }}>
                  {JSON.stringify(selected.metadata, null, 2)}
                </pre>
              </dd>
            </dl>
          </div>
        )}
      </aside>
    </main>
  );
}
