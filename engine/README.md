# z3rno-engine

The core memory engine implements `store`, `recall`, and `forget`, with audit
and graph traversal under `advanced()`. `MemoryEngine::embedded` assembles the
SQLite backends for an in-process SDK; `MemoryEngine::postgres` assembles
Postgres, pgvector, and Apache AGE for shared deployments. The bindings and
server use the same engine API.

The relational backend owns atomic audit-chain append. SQLite locks a write
transaction; Postgres serializes appends per tenant inside a transaction.
Neither path scans the full audit log for every write. Recall fetches its
candidate records through `get_many`; the Postgres strategy uses one query
for the candidate set while preserving vector ranking in the engine.

The three storage concerns remain separate backends. A `store` or `forget`
is not one transaction across relational, vector, and graph storage; callers
must not treat a successful relational write followed by a later backend
error as an all-or-nothing operation. The embedded tier is intended for
local use; shared multi-replica deployments need the Postgres tier.
