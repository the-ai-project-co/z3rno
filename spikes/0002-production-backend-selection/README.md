# Slice 0002 — production backend evaluation spike

This directory is **evaluation code, not shipped product.** It exists to produce evidence
for `_decision_docs/0008-production-backend-selection.md` (tracked outside this repo, in
the org-level planning folders). Nothing in here is wired into `engine`/`server`/`cli`, and
none of it should be treated as a dependency by product code.

Each candidate is its own standalone Cargo crate (own `[workspace]` table, not a root
workspace member — same pattern the language bindings use) so its dependency tree never
touches the product build.

- `RUBRIC.md` — the scoring criteria (written before any candidate was touched).
- `postgres-age/` — Postgres + pgvector + Apache AGE, via `sqlx`.
- `surrealdb/` — SurrealDB, via the `surrealdb` Rust SDK.
- `neo4j-qdrant/` — Neo4j (graph) + Qdrant (vector), via `neo4rs` + `qdrant-client`.

Run any spike with its own `cargo run` inside that candidate's directory; each has a
`docker-compose.yml` for the backend(s) it needs.
