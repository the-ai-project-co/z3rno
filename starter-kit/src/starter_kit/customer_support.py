"""Example — Customer support memory, scoped per customer.

Each customer's tickets live in their *own tenant* (``support-{user_id}``),
so a support agent can recall one customer's history without ever being
able to see another's — real backend-enforced isolation, the same
guarantee z3rno's multi-tenant Postgres backend gives production
deployments, not an application-level filter that has to be trusted to be
applied correctly on every query. ``forget`` demonstrates GDPR-style
right-to-be-forgotten, with an auditable proof of erasure.

Note on scope: ``z3rno.Client.recall`` has no server-side metadata filter —
it's pure top-k vector similarity within a tenant. The old starter kit's
``recall(strategy="LEXICAL", user_id=...)`` doesn't have an equivalent here;
tenant-per-customer is the honest replacement for the isolation half of
that call, and plain similarity search stands in for the keyword-matching
half.

Run:
    python -m starter_kit.customer_support
"""

from __future__ import annotations

import z3rno

from starter_kit._embed import hash_embed

ALICE_TENANT = "support-alice"
BOB_TENANT = "support-bob"


def _store_ticket(
    client: z3rno.Client, tenant_id: str, ticket_id: str, body: str
) -> None:
    client.store(
        tenant_id=tenant_id,
        tier="episodic",
        content=body,
        embedding=hash_embed(body),
        metadata={"kind": "support_ticket", "ticket_id": ticket_id},
    )


def main() -> None:
    client = z3rno.Client(backend="embedded", path="customer_support.db")

    # --- Seed a few tickets across two customers -----------------------------
    _store_ticket(
        client, ALICE_TENANT, "T-001", "Dashboard was slow this morning around 9am UTC."
    )
    _store_ticket(
        client, ALICE_TENANT, "T-002", "Export-to-CSV timed out for the Q3 report."
    )
    _store_ticket(
        client, BOB_TENANT, "T-003", "Pricing page is showing the wrong tier on mobile."
    )

    # --- Alice writes in again ------------------------------------------------
    incoming = "I'm having issues with the dashboard again."
    print(f"ALICE: {incoming}")
    results = client.recall(tenant_id=ALICE_TENANT, query=hash_embed(incoming), k=3)
    print(f"\nFound {len(results)} matching ticket(s) for Alice:")
    for r in results:
        ticket = r["metadata"].get("ticket_id", "?")
        print(f"  [{ticket}] {r['content']}")

    # --- Sanity check: Alice's tenant can't surface Bob's ticket, ever -------
    # recall() has no relevance threshold — it always returns up to k results
    # from within the tenant, so Alice's own tickets still come back even for
    # an unrelated query. What matters is that T-003 (Bob's) can never be
    # among them: it doesn't exist in Alice's tenant at all.
    cross_check = client.recall(
        tenant_id=ALICE_TENANT, query=hash_embed("pricing tier mobile"), k=3
    )
    cross_check_tickets = [r["metadata"].get("ticket_id", "?") for r in cross_check]
    print(
        f"\nAlice asking about 'pricing tier mobile' → {cross_check_tickets} "
        "(never T-003 — Bob's ticket lives in a different tenant entirely, "
        "not just filtered out of a shared one)."
    )

    # --- GDPR: Bob exercises his right to be forgotten ------------------------
    print("\nBob exercises right-to-be-forgotten...")
    bob_history = client.recall(tenant_id=BOB_TENANT, query=hash_embed("*"), k=10)
    for r in bob_history:
        proof = client.forget(tenant_id=BOB_TENANT, id=r["id"])
        ticket = r["metadata"].get("ticket_id", "?")
        print(f"  Removed {ticket} — audit proof: {proof['hash'][:12]}...")


if __name__ == "__main__":
    main()
