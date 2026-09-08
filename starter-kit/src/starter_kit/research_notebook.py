"""Example — Research notebook.

Stores three short research notes as *semantic* memory, linking each one
to the notes it mentions (via ``store``'s ``links`` parameter — see
``code_memory.py``'s docstring for why those links aren't queryable yet),
then answers a question about them by content-similarity recall.

The old starter kit's version of this example ran the full Forge pipeline
(ingest -> distill -> refine) — an LLM extracted entities and
relationships from raw text, and required ``OPENAI_API_KEY`` plus three
feature flags on the server just to run. That pipeline doesn't exist in
z3rno's current engine at all (distill/refine were never rebuilt after
the Rust rewrite). The upside: this version needs no LLM, no API key, and
no server — ``pip install z3rno`` and it runs, matching the zero-required-
infrastructure story the rest of z3rno tells.

Run:
    python -m starter_kit.research_notebook
"""

from __future__ import annotations

import z3rno

from starter_kit._embed import hash_embed

TENANT_ID = "starter-kit-research-notebook"

# Listed mentioned-before-mentioning so `links` can point at an id that
# already exists — Babbage and Somerville first (no outgoing links),
# Ada last (links to both).
NOTES = [
    (
        "babbage-bio",
        "Charles Babbage designed the Analytical Engine, a mechanical "
        "general-purpose computer. He collaborated extensively with Ada "
        "Lovelace, whose translation of Menabrea's paper included a method "
        "for computing Bernoulli numbers.",
        [],
    ),
    (
        "somerville-bio",
        "Mary Somerville was a Scottish scientist and writer. She tutored "
        "Ada Lovelace in mathematics and championed women's education. She "
        "corresponded with Charles Babbage about scientific computation.",
        [],
    ),
    (
        "ada-bio",
        "Ada Lovelace (1815-1852) was an English mathematician. She worked "
        "with Charles Babbage on the Analytical Engine and is widely "
        "credited with writing the first computer program. Her mentor was "
        "Mary Somerville.",
        ["babbage-bio", "somerville-bio"],
    ),
]


def main() -> None:
    client = z3rno.Client(backend="embedded", path="research_notebook.db")

    print("Storing 3 research notes...")
    ids_by_note: dict[str, str] = {}
    for note_id, body, mentions in NOTES:
        links = [(ids_by_note[m], "mentions") for m in mentions]
        memory = client.store(
            tenant_id=TENANT_ID,
            tier="semantic",
            content=body,
            embedding=hash_embed(body),
            metadata={"note_id": note_id},
            links=links,
        )
        ids_by_note[note_id] = memory["id"]
        print(f"  {note_id} -> {memory['id']}")

    print("\nWho did Ada Lovelace work with?")
    results = client.recall(
        tenant_id=TENANT_ID, query=hash_embed("Who did Ada Lovelace work with?"), k=5
    )
    print(f"  {len(results)} hit(s):")
    for r in results:
        snippet = r["content"][:140] + ("…" if len(r["content"]) > 140 else "")
        print(f"  - [{r['metadata'].get('note_id', '?')}] {snippet}")

    print(
        "\nThree plain-text notes, recalled by similarity alone — no LLM "
        "extraction, no server, no API key."
    )


if __name__ == "__main__":
    main()
