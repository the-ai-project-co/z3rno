# z3rno

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

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

**Up next:** slice 0005 — server component.

## Installation

Not yet published. Target channels once the first release ships:

- `pip install z3rno` (PyPI)
- `npm install @z3rno/sdk` (npm)
- `cargo add z3rno-engine` (crates.io)
- `ghcr.io/the-ai-project-co/z3rno-server` (container image)

## License

Apache License 2.0 — see [LICENSE](LICENSE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). All commits must be signed off per [DCO.md](DCO.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.
