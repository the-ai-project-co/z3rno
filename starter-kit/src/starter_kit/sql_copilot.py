"""Example — SQL copilot.

A copilot that learns your database. It remembers table/column
definitions and team decisions as long-lived *semantic* memory, and
recently-run queries as short-lived *episodic* memory — demonstrating how
tier alone (not a separate importance/TTL knob) signals durability in
z3rno's model.

Run:
    python -m starter_kit.sql_copilot
"""

from __future__ import annotations

import z3rno

from starter_kit._embed import hash_embed

TENANT_ID = "starter-kit-sql-copilot"


def main() -> None:
    client = z3rno.Client(backend="embedded", path="sql_copilot.db")

    # --- Schema facts: long-lived, semantic ----------------------------------
    schema_facts = [
        (
            "table users",
            "users(id UUID PK, email TEXT UNIQUE NOT NULL, created_at TIMESTAMPTZ)",
        ),
        (
            "table orders",
            "orders(id UUID PK, user_id UUID FK->users, total_cents BIGINT, "
            "placed_at TIMESTAMPTZ)",
        ),
        (
            "table products",
            "products(sku TEXT PK, name TEXT, price_cents BIGINT, active BOOL)",
        ),
    ]
    for label, ddl in schema_facts:
        content = f"{label}: {ddl}"
        client.store(
            tenant_id=TENANT_ID,
            tier="semantic",
            content=content,
            embedding=hash_embed(content),
            metadata={"kind": "schema", "label": label},
        )

    # --- A design decision the team made -------------------------------------
    decision = (
        "Decision (2026-03-04): all monetary fields are *_cents BIGINT, never "
        "DECIMAL. Rationale: avoid scale-mismatch bugs when joining across "
        "services."
    )
    client.store(
        tenant_id=TENANT_ID,
        tier="semantic",
        content=decision,
        embedding=hash_embed(decision),
        metadata={"kind": "decision", "topic": "money_columns"},
    )

    # --- Recently-run queries: short-lived, episodic -------------------------
    recent_queries = [
        "SELECT count(*) FROM orders WHERE placed_at > now() - interval '7 days';",
        (
            "SELECT user_id, sum(total_cents) FROM orders "
            "GROUP BY user_id ORDER BY 2 DESC LIMIT 10;"
        ),
    ]
    for q in recent_queries:
        client.store(
            tenant_id=TENANT_ID,
            tier="episodic",
            content=q,
            embedding=hash_embed(q),
            metadata={"kind": "recent_query"},
        )

    # --- Now: the user asks a question ---------------------------------------
    user_q = "What columns does the orders table have, and why are amounts in cents?"
    print(f"USER: {user_q}\n")

    results = client.recall(tenant_id=TENANT_ID, query=hash_embed(user_q), k=5)
    print(f"AGENT recalled {len(results)} memory/ies:")
    for r in results:
        kind = r["metadata"].get("kind", "?")
        snippet = r["content"][:120] + ("…" if len(r["content"]) > 120 else "")
        print(f"  [{kind:<8}] {snippet}")

    print(
        "\nThe copilot now has enough context to answer accurately — the "
        "schema row tells it the columns; the decision row tells it the why."
    )


if __name__ == "__main__":
    main()
