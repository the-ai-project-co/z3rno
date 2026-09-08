"""MCP server exposing z3rno's persistent memory engine to LLM agents.

z3rno is a persistent memory engine, not a general-purpose database: it
stores discrete memories tagged with a tier (working/episodic/semantic/
procedural), retrieves them by vector similarity, and keeps an append-only,
hash-chained audit log of every store/forget so erasure is provable. This
server wraps the four operations the engine actually has — store, recall,
forget, and the one advanced op, audit — as MCP tools so an LLM agent can
give itself persistent memory across a conversation or across sessions.

Config (env vars, mirroring the CLI's own flag names):
  Z3RNO_SQLITE_PATH   embedded SQLite file path (default "z3rno.db")
  Z3RNO_TENANT_ID     default tenant for tool calls that omit tenant_id
                      (default "local")
  Z3RNO_DATABASE_URL  if set, use the Postgres backend instead of the
                      embedded SQLite default (a "postgres://..." URL)
"""

import os

import z3rno
from mcp.server.fastmcp import FastMCP

from z3rno_mcp.embed import hash_embed

VALID_TIERS = ("working", "episodic", "semantic", "procedural")

mcp = FastMCP(
    "z3rno",
    instructions=(
        "z3rno is a persistent memory engine for LLM agents. It is not a "
        "general database — it stores discrete memories (tagged working / "
        "episodic / semantic / procedural), retrieves them by vector "
        "similarity, and keeps a tamper-evident audit log of every write "
        "and deletion. Use z3rno_store to save something worth "
        "remembering, z3rno_recall to search for relevant past memories, "
        "z3rno_forget to erase one, and z3rno_audit to inspect the audit "
        "trail."
    ),
)

_client: z3rno.Client | None = None


def _get_client() -> z3rno.Client:
    """Builds the shared z3rno.Client once, on first use, and reuses it for
    every subsequent tool call."""
    global _client
    if _client is None:
        database_url = os.environ.get("Z3RNO_DATABASE_URL")
        if database_url:
            _client = z3rno.Client(backend="postgres", connection_string=database_url)
        else:
            path = os.environ.get("Z3RNO_SQLITE_PATH", "z3rno.db")
            _client = z3rno.Client(backend="embedded", path=path)
    return _client


def _default_tenant() -> str:
    return os.environ.get("Z3RNO_TENANT_ID", "local")


@mcp.tool()
def z3rno_store(
    content: str,
    tier: str = "semantic",
    tenant_id: str | None = None,
    metadata: dict | None = None,
    embedding: list[float] | None = None,
) -> str:
    """Save a memory into z3rno's persistent store.

    Use this whenever there's a fact, preference, decision, or piece of
    context worth remembering beyond the current turn — not for scratch
    work or anything already visible in the conversation.

    `tier` classifies the memory's expected lifespan and role:
      - "working": short-lived, task-scoped context.
      - "episodic": a specific event or interaction ("user asked X on Y").
      - "semantic": a durable fact or preference (the default).
      - "procedural": a how-to / rule the agent should follow going forward.

    If `embedding` is omitted, the content is embedded with a naive local
    hashing embedding (bag-of-words, no real semantics) so recall still
    works without an external embedding model — pass a real embedding for
    better recall quality.

    Returns the id of the newly created memory (a string), which can later
    be passed to z3rno_forget.
    """
    if tier not in VALID_TIERS:
        raise ValueError(f"invalid tier {tier!r}: expected one of {VALID_TIERS}")
    vector = embedding if embedding is not None else hash_embed(content)
    memory = _get_client().store(
        tenant_id=tenant_id or _default_tenant(),
        tier=tier,
        content=content,
        embedding=vector,
        metadata=metadata,
    )
    return memory["id"]


@mcp.tool()
def z3rno_recall(
    query: str,
    tenant_id: str | None = None,
    k: int = 5,
    embedding: list[float] | None = None,
) -> list[dict]:
    """Search z3rno's persistent store for memories relevant to `query`.

    Call this before answering when past context (facts about the user,
    prior decisions, established preferences) might change the answer —
    it's how the agent retrieves what it previously stored with
    z3rno_store.

    If `embedding` is omitted, `query` is embedded with the same naive
    local hashing embedding used as the store-time fallback, so it lands
    in the same vector space as memories stored without a real embedding.
    Recall quality against memories stored with real embeddings will be
    poor unless a matching real embedding is supplied here too.

    Returns up to `k` memories, most similar first, as a list of
    {id, tier, content, created_at} dicts.
    """
    vector = embedding if embedding is not None else hash_embed(query)
    memories = _get_client().recall(
        tenant_id=tenant_id or _default_tenant(),
        query=vector,
        k=k,
    )
    return [
        {
            "id": m["id"],
            "tier": m["tier"],
            "content": m["content"],
            "created_at": m["created_at"],
        }
        for m in memories
    ]


@mcp.tool()
def z3rno_forget(id: str, tenant_id: str | None = None) -> str:
    """Permanently erase a memory from z3rno's persistent store.

    `id` is a memory id as returned by z3rno_store or z3rno_recall. Use
    this when the user asks to forget something, retracts a fact, or a
    stored memory is confirmed stale or wrong.

    Erasure is proven, not silent: on success this returns a confirmation
    naming the audit event id and hash that record the deletion. If no
    memory with that id exists (already forgotten, or never existed), this
    is reported as a plain "no memory found" message, not an error.
    """
    proof = _get_client().forget(tenant_id=tenant_id or _default_tenant(), id=id)
    if proof is None:
        return f"no memory found with id {id!r} — nothing to forget"
    return (
        f"forgot memory {id!r} "
        f"(audit event {proof['audit_event_id']}, hash {proof['hash']})"
    )


@mcp.tool()
def z3rno_audit(tenant_id: str | None = None) -> list[dict]:
    """Return the tenant's full audit log from z3rno's memory engine.

    The audit log is an append-only, hash-chained record of every store
    and forget operation for the tenant, oldest first — use this to prove
    what was remembered or erased, and when, rather than to look up
    memory content (use z3rno_recall for that).

    Returns a list of events, each a dict with operation, memory_id, at
    (timestamp), and hash (this event's position in the hash chain).
    """
    events = _get_client().advanced.audit(tenant_id=tenant_id or _default_tenant())
    return [
        {
            "operation": e["operation"],
            "memory_id": e["memory_id"],
            "at": e["at"],
            "hash": e["hash"],
        }
        for e in events
    ]


def main() -> None:
    _get_client()  # build once, at startup, rather than on the first tool call
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
