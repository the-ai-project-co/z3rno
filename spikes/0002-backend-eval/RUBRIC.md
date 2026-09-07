# Backend evaluation rubric

Written before any candidate is touched, per `_plan_docs/0002-production-backend-evaluation-spike.md`
sub-slice 0002.1. Applied identically to every candidate — same six criteria, same scale,
scored only after each candidate's proof-of-concept actually runs.

## Scale

Each criterion is scored **Pass / Caveat / Fail**, with one line of evidence. A candidate
with any **Fail** on a hard requirement (multi-tenant isolation) is ruled out regardless of
its other scores.

## Criteria

1. **Multi-tenant isolation guarantee** (hard requirement) — must match or exceed z3rno's
   current Postgres RLS story: rows/edges/vectors scoped per tenant such that one tenant's
   client, given only its own credentials, cannot read or write another tenant's data.
   Evidence: does the candidate have a native mechanism (RLS, per-tenant namespace/database,
   ABAC), or does isolation have to be hand-rolled in application code?

2. **Single-query vector + graph + temporal capability** — can one query answer "semantically
   similar nodes, filtered by a graph relationship, as of a point in time," or does it take
   multiple round-trips glued together in the client? Document the actual cost if it needs
   more than one query (extra latency, lost atomicity, extra code to maintain).

3. **Rust client library maturity** — is there an official or de-facto-standard Rust client?
   Evidence: crates.io download count / last-publish date, whether it wraps a stable wire
   protocol or an unstable FFI, and whether `cognee-rs` already depends on it (see
   `_research_refs/cognee-rs/docs/tools/backends.md` — reused where applicable).

4. **Operational complexity to self-host** — one binary/image vs. a cluster of services,
   backup/restore story, whether it needs a companion process (e.g. a separate vector store
   next to a graph DB).

5. **Licensing** — OSS license, and whether any feature we'd actually need sits behind a
   commercial edition.

6. **Temporal/SCD-2 versioning portability** — folded in per the plan doc's open question,
   not scored separately: does the candidate support the append-only, `valid_from`/`valid_to`
   versioning z3rno's engine already relies on, or would it have to be reimplemented as
   application-level bookkeeping on top of the candidate?

## Context found before spiking (informs interpretation, not the scores)

`_research_refs/cognee-rs/docs/tools/backends.md` shows what cognee's own Rust rewrite
actually ships, which matters because the plan doc cites "diversity like cognee" as the
motivation for this spike:

- cognee-rs's **graph** trait (`GraphDBTrait`) has two adapters: `LadybugAdapter` (an
  **embedded**, in-process graph store, the default) and `PgGraphAdapter` (Postgres, feature
  `postgres`). There is no Neo4j adapter in cognee-rs.
- cognee-rs's **vector** trait (`VectorDB`) has `LanceDbAdapter` (on-disk, default),
  `BruteForceVectorDB` (in-memory), and `PgVectorAdapter` (Postgres, feature `pgvector`).
  No Qdrant adapter in the OSS crate (a Qdrant adapter exists only in cognee's closed-source
  cloud companion).
- So "diversity like cognee" in cognee-rs concretely means **embedded-by-default +
  Postgres-as-the-one-production-adapter**, not a menu of different production databases.
  Neo4j is a real option in cognee's *Python* SDK, not in cognee-rs.

This doesn't rule Neo4j+Qdrant out of this spike — the plan doc explicitly wants us to
evaluate it on its own merits — but it does mean citing cognee-rs as precedent for it would
be inaccurate. Candidate 3 below is scored purely on the rubric, not on a cognee-rs parallel
that doesn't actually exist.
