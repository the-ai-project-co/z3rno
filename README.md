# z3rno

[![License](https://img.shields.io/github/license/the-ai-project-co/z3rno)](https://opensource.org/licenses/Apache-2.0)
[![Latest tag](https://img.shields.io/github/v/tag/the-ai-project-co/z3rno)](https://github.com/the-ai-project-co/z3rno/tags)
[![Downloads](https://img.shields.io/github/downloads/the-ai-project-co/z3rno/total)](https://github.com/the-ai-project-co/z3rno/releases)
[![Commits](https://img.shields.io/github/commit-activity/t/the-ai-project-co/z3rno)](https://github.com/the-ai-project-co/z3rno/commits/main)
[![Contributors](https://img.shields.io/github/contributors/the-ai-project-co/z3rno)](https://github.com/the-ai-project-co/z3rno/graphs/contributors)
[![Stars](https://img.shields.io/github/stars/the-ai-project-co/z3rno)](https://github.com/the-ai-project-co/z3rno/stargazers)
[![Forks](https://img.shields.io/github/forks/the-ai-project-co/z3rno)](https://github.com/the-ai-project-co/z3rno/forks)

[![npm version](https://img.shields.io/npm/v/%40z3rno%2Fsdk)](https://www.npmjs.com/package/@z3rno/sdk)
[![npm downloads](https://img.shields.io/npm/dm/%40z3rno%2Fsdk)](https://www.npmjs.com/package/@z3rno/sdk)
[![PyPI version](https://img.shields.io/pypi/v/z3rno)](https://pypi.org/project/z3rno/)
[![PyPI downloads](https://img.shields.io/pypi/dm/z3rno)](https://pypi.org/project/z3rno/)
[![crates.io version](https://img.shields.io/crates/v/z3rno-engine)](https://crates.io/crates/z3rno-engine)
[![crates.io downloads](https://img.shields.io/crates/d/z3rno-engine)](https://crates.io/crates/z3rno-engine)

**z3rno is an open-source memory engine for AI agents** — a Rust core with native Python and TypeScript bindings, embeddable with zero required infrastructure by default, with a pluggable production backend and an optional server for multi-tenant deployments.

> This repository is under active ground-up construction. It replaces the previous z3rno stack (now archived) with a single Rust monorepo. Expect rapid, breaking iteration until the v1.0.0 launch.

## Why a rewrite

The previous z3rno stack shipped as six separately-published Python repos, hard-wired to a single Postgres deployment. Direct feedback from real users — including a request to be embeddable and infrastructure-light by default — drove a full rewrite: one monorepo, a Rust core for speed, native bindings instead of thin HTTP clients, and a memory engine that runs embedded out of the box while staying pluggable to a production backend when you need one.

## What it does

z3rno gives an agent persistent memory across sessions through four operations:

| Verb | What it does |
|---|---|
| `store` | Write a memory (raw content or structured data) into the engine. |
| `recall` | Retrieve relevant memories for a query, across configurable strategies. |
| `forget` | Remove a memory, with an auditable proof of erasure. |
| `audit` | Query the append-only, hash-chained history of what changed and when. |

Memory is organized across four tiers — working, episodic, semantic, and procedural — and can be queried as vectors, as a knowledge graph, or both.

## Architecture at a glance

- **Core engine** — Rust, embeddable by default (SQLite + an embedded vector index + an embedded graph store), no required external services.
- **Bindings** — native compiled bindings for Python (PyO3) and TypeScript (napi-rs); the SDK *is* the engine, not a thin client.
- **Server** — an optional Axum HTTP server for production, multi-tenant, or multi-language deployments.
- **CLI** — a standalone binary, distributed via both crates.io and npm.
- **Production backend** — Postgres + pgvector + Apache AGE, chosen after a spike comparing it against SurrealDB and Neo4j+Qdrant on multi-tenant isolation, operational complexity, and licensing; see `spikes/0002-backend-eval/`. Fully implemented against the same traits the embedded backend uses — see `engine/COMPATIBILITY.md`.

Full architecture rationale lives in this org's internal decision and plan documents.

## Status

Pre-v1.0.0. Every unit of work ships as its own pull request against `main`; follow progress via the issues and PRs in this repo.

**Shipped so far:**

- **Slice 0001 — Monorepo bootstrap.** Cargo workspace (`engine`/`server`/`cli`), Python (PyO3) and TypeScript (napi-rs) binding scaffolds, CI (fmt/clippy/test).
- **Slice 0002 — Production backend evaluation spike.** Working Rust proof-of-concepts against real, dockerized Postgres+pgvector+AGE, SurrealDB, and Neo4j+Qdrant instances, scored against a shared rubric. Decided: Postgres + pgvector + Apache AGE is the production backend slice 0004 builds against.
- **Slice 0003 — Core engine + embedded backend.** A real, working `MemoryEngine`: `store`/`recall`/`forget` plus an append-only, hash-chained `audit` log, against SQLite (relational), an embedded vector index (`hnsw_rs`), and an embedded graph (`petgraph`) — the zero-infra default. Covers all four memory tiers end-to-end. Known gap: the embedded graph has no persistence across a restart yet ([#8](https://github.com/the-ai-project-co/z3rno/issues/8)).
- **Slice 0004 — Production backend.** `MemoryEngine::postgres()` — Postgres + pgvector + Apache AGE against the same traits the embedded backend implements. Resolves the AGE/RLS isolation gap slice 0002 flagged (one AGE graph per tenant, not a shared graph filtered by a property). Compatibility enforcement done by making an unsupported backend combo unconstructable through the public API — see `engine/COMPATIBILITY.md`.
- **Slice 0005 — Server component.** The `z3rno-server` crate: an Axum HTTP API in front of the engine — `/v1/memories` (+`/recall`, `/forget`), `/v1/audit`, `/v1/sessions`, JWT + API-key auth, superadmin cross-tenant budget admin, and real two-tier health checks + Prometheus metrics + an opt-in OpenTelemetry bridge. Closes a real gap: the old Python server's readiness check was a confirmed no-op that always reported healthy; this one runs genuine backend probes. Pluggable cache backend (SQLite default, Redis/Valkey opt-in) — no external service required to run the full test suite.
- **Slice 0006 — Language bindings.** Real, tested `z3rno` (PyO3) and `@z3rno/sdk` (napi-rs) packages over `z3rno-engine` directly — no server/HTTP dependency, embedded SQLite by default, a `postgres` backend opt-in. Python ships a sync API (a shared Tokio runtime under the hood); TypeScript ships an async, `Promise`-based API — no sync/async duality in either language, matching each ecosystem's own idioms. `store`/`recall`/`forget` top-level, `client.advanced.audit(...)` for the audit trail. Real multi-platform release pipeline: maturin wheels + napi-rs addons across Linux/macOS/Windows (x86_64/ARM64), publishing to PyPI (trusted publishing) and npm (`--provenance`) on a tagged release.
- **Slice 0007 — CLI, crates.io, and release distribution.** A real `z3rno-cli` binary (`init`/`serve`/`store`/`recall`/`forget`) usable standalone against a local embedded store, with a naive local hashing embedding so `store`/`recall` work with zero external services — clearly documented as a placeholder, not a semantic model. Distributed via crates.io (`z3rno-engine`, `z3rno-server`, `z3rno-cli`, with per-crate SBOMs) and npm (`z3rno`, platform-binary pattern, `--provenance`). Real multi-arch `ghcr.io/the-ai-project-co/z3rno/server` image (SLSA build provenance + SBOM), refusing to start without an explicit `Z3RNO_JWT_SECRET` rather than baking in an insecure default. Known gap found and tracked: the embedded vector backend, like the graph backend ([#8](https://github.com/the-ai-project-co/z3rno/issues/8)), doesn't persist across a process restart yet ([#21](https://github.com/the-ai-project-co/z3rno/issues/21)).
- **Slice 0008 — MCP server + evals harness.** A Python MCP server (`mcp/`, `FastMCP` over stdio) exposing `store`/`recall`/`forget`/`audit` as tools against a real `z3rno` client, plus three independent eval harnesses (Rust, Python, TypeScript) scoring recall@k, MRR, faithfulness, and latency against a shared golden dataset (`evals/fixtures/golden_v1.json`) through each language's real bindings. A canonical naive local hashing embedding (`hash-embed/`) is shared by the Rust harness and ported byte-for-byte to Python/TypeScript so scores are comparable across languages; each harness sets its own regression thresholds from its own measured baseline.
- **Slice 0009 — Starter kit.** `z3rno-starter-kit` (`starter-kit/`), five worked examples over the real bindings surface — chat memory, customer support, a SQL copilot, code memory, and a research notebook — redesigned around the actual engine (no recall strategies, no `ingest`/`distill`/`refine` pipeline) rather than mechanically ported from the pre-rewrite version. `customer_support` uses one tenant per customer for real backend-enforced isolation; `code_memory`/`research_notebook` link related memories via `store`'s `links` parameter (written to the graph backend, not yet queryable through the bindings — [#29](https://github.com/the-ai-project-co/z3rno/issues/29)) and fall back to content-similarity recall. Every example runs against a local embedded store with zero external services, so CI runs them end to end rather than just import-gating them.
- **Slice 0010 — New website + docs site.** Closed the graph-read gap `code_memory`/`research_notebook` flagged: `Engine::list_memories`/`neighbors` plus `GET /v1/memories` and `GET /v1/memories/{id}/neighbors` server routes, thin wrappers over already-tested backend methods. New in-monorepo `frontend/` Next.js app — a graph visualizer (`react-force-graph-2d`) that seeds from `list_memories` and expands neighbors on click, the successor to the old stack's `/graph` page. On the website side: a `/docs` section (install/sdk/cli/mcp/architecture) covering every publish channel and binding, replacing scattered per-package READMEs as the install source of truth. Known gap found via live end-to-end testing, not code review: no self-service token-issuing path yet ([#35](https://github.com/the-ai-project-co/z3rno/issues/35)).

**Up next:** slice 0011 — v1 launch sequencing and dependency graph.

## Installation

Not yet published. Target channels once the first release ships:

- `pip install z3rno` (PyPI)
- `npm install @z3rno/sdk` (npm)
- `cargo add z3rno-engine` (crates.io)
- `ghcr.io/the-ai-project-co/z3rno/server` (container image)

## License

Apache License 2.0 — see [LICENSE](LICENSE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All commits must be signed off per [DCO.md](DCO.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.
