# Backend compatibility matrix

**Version 1 — 2026-09-07.** Update this file's version whenever the set of
supported tiers, or what each guarantees, changes.

`z3rno-engine` ships two backend tiers, each a complete, matched set of
relational/vector/graph implementations. `MemoryEngine::embedded()` and
`MemoryEngine::postgres()` are the only public ways to build a
`MemoryEngine` — there is currently no way to mix backends across tiers (a
Postgres relational store with an embedded vector index, for example)
through the public API at all, so there's nothing else to list here yet.
If independent per-concern backend selection is ever built, extend this
table and add the runtime check `MemoryEngine`'s constructor doc comment
describes as the point to add it.

| Tier | Constructor | Relational | Vector | Graph | Tenant isolation |
|---|---|---|---|---|---|
| **Embedded** | `MemoryEngine::embedded()` | SQLite | in-process (`hnsw_rs`) | in-process (`petgraph`) | Application-code only — every embedded backend filters by `tenant_id` in Rust, not enforced by the storage engine itself. |
| **Postgres** | `MemoryEngine::postgres()` | Postgres, RLS | pgvector, RLS | Apache AGE, one graph per tenant | Storage-engine-enforced — Postgres RLS (`FORCE ROW LEVEL SECURITY`) on the relational/vector tables, plus explicit `WHERE tenant_id = $N` filters as defense in depth (RLS alone was confirmed live to be insufficient — a superuser connection bypasses it unconditionally, so a query relying on RLS alone silently returned every tenant's rows). Graph data isolated structurally, not by filter: each tenant gets its own AGE graph (its own Postgres schema), not one shared graph filtered by a `tenant_id` property — see `backend::postgres::graph`'s module doc for why the filter-only approach was rejected. |

## Which tier to use

- **Embedded** — local development, a single-operator deployment, or
  anywhere z3rno runs as a library inside one process with no other tenant
  sharing that process's storage. This is `pip install z3rno`'s zero-infra
  default.
- **Postgres** — any deployment where more than one tenant's data lives in
  the same storage, or where compliance/audit requirements need the
  isolation guarantee enforced below the application layer, not just
  within it.

## Operational requirements — Postgres tier

- `CREATE EXTENSION` privilege for `vector` and `age` (or a privileged role
  runs `backend::postgres::BOOTSTRAP_SQL` once — see decision-doc 0002).
- `ALTER DATABASE ... SET session_preload_libraries = 'age'` privilege, or
  the equivalent from `BOOTSTRAP_SQL`. Required because AGE's `cypher()`
  needs the library loaded into every session, and the direct way to do
  that — `LOAD 'age'` — requires Postgres *superuser*, which the app's
  runtime role should never be. `session_preload_libraries` preloads AGE
  for every session on the database automatically, regardless of which
  role connects, sidestepping that requirement for the runtime role.
- The app's **runtime** role (not just a one-time provisioning role) needs
  ongoing `CREATE` privilege on the database, plus `USAGE`/`SELECT`/
  `EXECUTE` on the `ag_catalog` schema. The per-tenant-graph design means a
  new AGE graph — its own schema — gets created the first time the runtime
  role ever sees a given tenant, for the life of the deployment, not just
  once at startup.

See `backend::postgres::provision::BOOTSTRAP_SQL` and each Postgres
backend's module doc comment for the full reasoning behind each of these.
