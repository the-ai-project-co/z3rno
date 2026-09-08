"""Example — Code memory.

Stores a small module's functions and classes as individual *procedural*
memories, each carrying a qualified name and a one-line description, and
recalls the one most relevant to a natural-language question about the
code — e.g. "how do I place an order?" surfaces ``OrderService.place``
over ``OrderService.refund``, by content similarity alone.

Each symbol is also linked to the symbols it calls via ``store``'s
``links`` parameter (listed in dependency order below so a callee's id
always exists before its caller is stored), recording real graph edges in
z3rno's graph backend — but today's ``recall`` is vector-similarity only
and doesn't walk them yet, so this example can't yet answer "what does X
call?" the way a graph query would. See z3rno#29.

Run:
    python -m starter_kit.code_memory
"""

from __future__ import annotations

from typing import TypedDict

import z3rno

from starter_kit._embed import hash_embed

TENANT_ID = "starter-kit-code-memory"


class Symbol(TypedDict):
    qualified_name: str
    kind: str
    description: str
    calls: list[str]


# A handful of symbols from a small OrderService module — realistic but
# compact, hand-described rather than parsed (z3rno has no source-code
# extractor; see the module docstring). Listed callees-before-callers so
# `links` can point at an id that already exists.
SYMBOLS: list[Symbol] = [
    {
        "qualified_name": "OrderService",
        "kind": "class",
        "description": "OrderService — books, ships, and refunds orders.",
        "calls": [],
    },
    {
        "qualified_name": "OrderService._mint_id",
        "kind": "function",
        "description": "OrderService._mint_id() — generates a new order id.",
        "calls": [],
    },
    {
        "qualified_name": "OrderService._charge",
        "kind": "function",
        "description": (
            "OrderService._charge(order_id, refund=False) — talks to the "
            "payment provider."
        ),
        "calls": [],
    },
    {
        "qualified_name": "OrderService.place",
        "kind": "function",
        "description": (
            "OrderService.place(user_id, items) — books a new order, charges "
            "the user, returns the order id."
        ),
        "calls": ["OrderService._mint_id", "OrderService._charge"],
    },
    {
        "qualified_name": "OrderService.refund",
        "kind": "function",
        "description": (
            "OrderService.refund(order_id) — reverses a charge for a placed order."
        ),
        "calls": ["OrderService._charge"],
    },
]


def main() -> None:
    client = z3rno.Client(backend="embedded", path="code_memory.db")

    print("Storing symbols from OrderService...")
    ids_by_name: dict[str, str] = {}
    for sym in SYMBOLS:
        links = [(ids_by_name[callee], "calls") for callee in sym["calls"]]
        memory = client.store(
            tenant_id=TENANT_ID,
            tier="procedural",
            content=sym["description"],
            embedding=hash_embed(sym["description"]),
            metadata={"kind": sym["kind"], "qualified_name": sym["qualified_name"]},
            links=links,
        )
        ids_by_name[sym["qualified_name"]] = memory["id"]

    print("\nHow do I place an order?")
    results = client.recall(
        tenant_id=TENANT_ID, query=hash_embed("how do I place an order"), k=3
    )
    for r in results:
        meta = r["metadata"]
        print(f"  [{meta.get('kind', '?'):<8}] {meta.get('qualified_name', '?')}")

    print(
        "\nThe CODE-strategy graph walk the old starter kit had (surfacing a "
        "symbol's callees) isn't available through the bindings yet — this "
        "shows content-similarity recall over manually described symbols "
        "instead."
    )


if __name__ == "__main__":
    main()
