import type { NextConfig } from "next";

// Local dev tool only (slice 0010) — no static export, no deploy config.
// `npm run dev` against a locally running z3rno-server is the whole
// workflow for now.
const nextConfig: NextConfig = {
  // Next 16 auto-writes AGENTS.md/CLAUDE.md on `next dev` — this monorepo
  // already has its own root CLAUDE.md; don't let a second, framework-
  // generated one appear inside frontend/.
  agentRules: false,
};

export default nextConfig;
