<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/brand/z3rno-wordmark-transparent-dark.svg">
    <img src=".github/brand/z3rno-wordmark-transparent-light.svg" alt="z3rno" width="280">
  </picture>
</p>

<p align="center">
  <b>An open-source memory engine for AI agents</b> — a Rust core with native Python and TypeScript bindings, embeddable with zero required infrastructure by default.
</p>

<p align="center">
  <a href="https://the-ai-project-co.github.io/z3rno-website/">Website</a> ·
  <a href="https://the-ai-project-co.github.io/z3rno-website/docs">Docs</a> ·
  <a href="https://the-ai-project-co.github.io/z3rno-website/progress">Progress log</a> ·
  <a href="https://github.com/the-ai-project-co/z3rno/issues">Issues</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

<p align="center">
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/github/license/the-ai-project-co/z3rno" alt="License"></a>
  <a href="https://github.com/the-ai-project-co/z3rno/tags"><img src="https://img.shields.io/github/v/tag/the-ai-project-co/z3rno" alt="Latest tag"></a>
  <a href="https://github.com/the-ai-project-co/z3rno/commits/main"><img src="https://img.shields.io/github/commit-activity/t/the-ai-project-co/z3rno" alt="Commits"></a>
  <a href="https://github.com/the-ai-project-co/z3rno/graphs/contributors"><img src="https://img.shields.io/github/contributors/the-ai-project-co/z3rno" alt="Contributors"></a>
  <a href="https://github.com/the-ai-project-co/z3rno/stargazers"><img src="https://img.shields.io/github/stars/the-ai-project-co/z3rno" alt="Stars"></a>
  <a href="https://github.com/the-ai-project-co/z3rno/forks"><img src="https://img.shields.io/github/forks/the-ai-project-co/z3rno" alt="Forks"></a>
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@z3rno/cli"><img src="https://img.shields.io/npm/v/%40z3rno%2Fcli?label=npm%20%40z3rno%2Fcli" alt="npm CLI version"></a>
  <a href="https://www.npmjs.com/package/@z3rno/sdk"><img src="https://img.shields.io/npm/v/%40z3rno%2Fsdk?label=npm%20%40z3rno%2Fsdk" alt="npm SDK version"></a>
  <a href="https://pypi.org/project/z3rno/"><img src="https://img.shields.io/pypi/v/z3rno?label=pypi%20z3rno" alt="PyPI version"></a>
  <a href="https://crates.io/crates/z3rno-engine"><img src="https://img.shields.io/crates/v/z3rno-engine?label=crates.io%20z3rno-engine" alt="crates.io version"></a>
  <a href="#crates">4 crates published →</a>
</p>

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

## Where to start

| I want to... | Go here |
|---|---|
| Try it from the command line, zero setup | [CLI](#cli) below, or [`cli/README.md`](cli/README.md) |
| Add memory to a Python agent | [Python](#python) below, or [`bindings/python/README.md`](bindings/python/README.md) |
| Add memory to a Node/TypeScript agent | [TypeScript](#typescript) below, or [`bindings/typescript/README.md`](bindings/typescript/README.md) |
| Embed the engine directly in a Rust binary | [Rust](#rust) below |
| Run a production, multi-tenant HTTP deployment | [Docker](#docker) below, or [`server/README.md`](server/README.md) |
| Give Claude Desktop / Cursor / Claude Code persistent memory | [`mcp/README.md`](mcp/README.md) |
| See worked, runnable examples | [`starter-kit/`](starter-kit/) — 5 scripts, zero external services |
| Understand the tier/backend architecture in depth | [Architecture at a glance](#architecture-at-a-glance) below, or [docs/architecture](https://the-ai-project-co.github.io/z3rno-website/docs/architecture) |

## Quickstart

Every surface below talks to the same embedded default: a local SQLite file plus an in-process vector index and graph, no external services required. All four accept the same shape — a `tenant_id`, a `tier` (`working`/`episodic`/`semantic`/`procedural`), and `content` — because the SDKs and CLI are native bindings over the engine, not thin HTTP clients.

### CLI

```sh
npm install -g @z3rno/cli
# or: cargo install z3rno-cli

z3rno init
z3rno store "the user prefers dark mode"
z3rno recall "dark mode"
```

### Python

```sh
pip install z3rno
```

```python
import z3rno

client = z3rno.Client()  # opens/creates z3rno.db in the cwd

memory = client.store(tenant_id="t1", tier="episodic", content="the user prefers dark mode")
client.recall(tenant_id="t1", query=[0.1, 0.2], k=5)
client.forget(tenant_id="t1", id=memory["id"])
```

### TypeScript

```sh
npm install @z3rno/sdk
```

```ts
import { Client } from "@z3rno/sdk";

const client = await Client.connect(); // opens/creates z3rno.db in the cwd

const memory = await client.store({ tenantId: "t1", tier: "episodic", content: "the user prefers dark mode" });
await client.recall({ tenantId: "t1", query: [0.1, 0.2, 0.3], k: 5 });
await client.forget({ tenantId: "t1", id: memory.id });
```

### Rust

```sh
cargo add z3rno-engine
```

```rust
use z3rno_engine::{MemoryEngine, Tier};

let engine = MemoryEngine::embedded("z3rno.db")?;

let memory = engine
    .store("t1", Tier::Episodic, "the user prefers dark mode".into(), None, serde_json::json!({}), vec![])
    .await?;
engine.recall("t1", vec![0.1, 0.2], 5).await?;
engine.forget("t1", memory.id).await?;
```

### Docker

The server is a production, multi-tenant deployment of the same engine over an HTTP API — it refuses to start without an explicit JWT secret rather than falling back to an insecure default:

```sh
docker run -e Z3RNO_JWT_SECRET=<a-real-random-secret> \
  -p 8080:8080 ghcr.io/the-ai-project-co/z3rno/server
```

Every quickstart above uses the embedded, zero-infra default. For a production Postgres + pgvector + Apache AGE backend instead, see the `backend`/`connection_string` options in [`bindings/python/README.md`](bindings/python/README.md#production-backend) and [`bindings/typescript/README.md`](bindings/typescript/README.md#production-backend).

## Architecture at a glance

- **Core engine** — Rust, embeddable by default (SQLite + an embedded vector index + an embedded graph store), no required external services.
- **Bindings** — native compiled bindings for Python (PyO3) and TypeScript (napi-rs); the SDK *is* the engine, not a thin client.
- **Server** — an optional Axum HTTP server for production, multi-tenant, or multi-language deployments.
- **CLI** — a standalone binary, distributed via both crates.io and npm.
- **Production backend** — Postgres + pgvector + Apache AGE, chosen after a spike comparing it against SurrealDB and Neo4j+Qdrant on multi-tenant isolation, operational complexity, and licensing; see `spikes/0002-production-backend-selection/`. Fully implemented against the same traits the embedded backend uses — see `engine/COMPATIBILITY.md`.

Full architecture rationale lives in this org's internal decision and plan documents.

## Installation

<a id="crates"></a>

| Channel | Command |
|---|---|
| pip (SDK) | `pip install z3rno` |
| pip (MCP server) | `pip install z3rno-mcp` |
| pip (evals harness) | `pip install z3rno-evals` |
| npm (SDK) | `npm install @z3rno/sdk` |
| npm (CLI) | `npm install -g @z3rno/cli` |
| Docker | `docker pull ghcr.io/the-ai-project-co/z3rno/server` |

**crates.io** — four crates, published independently:

| Crate | What it is |
|---|---|
| [![z3rno-engine](https://img.shields.io/crates/v/z3rno-engine?label=z3rno-engine)](https://crates.io/crates/z3rno-engine) [![downloads](https://img.shields.io/crates/d/z3rno-engine?label=downloads)](https://crates.io/crates/z3rno-engine) | The core memory engine — `cargo add z3rno-engine` |
| [![z3rno-server](https://img.shields.io/crates/v/z3rno-server?label=z3rno-server)](https://crates.io/crates/z3rno-server) [![downloads](https://img.shields.io/crates/d/z3rno-server?label=downloads)](https://crates.io/crates/z3rno-server) | The Axum HTTP server — `cargo add z3rno-server`, or use the [Docker image](#docker) |
| [![z3rno-cli](https://img.shields.io/crates/v/z3rno-cli?label=z3rno-cli)](https://crates.io/crates/z3rno-cli) [![downloads](https://img.shields.io/crates/d/z3rno-cli?label=downloads)](https://crates.io/crates/z3rno-cli) | The `z3rno` binary — `cargo install z3rno-cli` |
| [![z3rno-hash-embed](https://img.shields.io/crates/v/z3rno-hash-embed?label=z3rno-hash-embed)](https://crates.io/crates/z3rno-hash-embed) [![downloads](https://img.shields.io/crates/d/z3rno-hash-embed?label=downloads)](https://crates.io/crates/z3rno-hash-embed) | The naive local hashing embedding shared by the CLI and eval harnesses | 

## Status

Pre-v1.0.0. Every unit of work ships as its own pull request against `main`; follow progress via the issues and PRs in this repo, or the [progress log](https://the-ai-project-co.github.io/z3rno-website/progress).

<details>
<summary><b>Shipped so far</b> — slices 0001 through 0010</summary>

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

</details>

**Up next:** slice 0011 — v1 launch sequencing and dependency graph.

## License

Apache License 2.0 — see [LICENSE](LICENSE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All commits must be signed off per [DCO.md](DCO.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.
