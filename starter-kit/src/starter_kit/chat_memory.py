"""Example — Chat agent memory.

A multi-turn chat that remembers a user's preferences across runs.
Demonstrates the two verbs most agent-memory use cases actually need:
``store`` to write a turn, ``recall`` to retrieve relevant context for the
next one. Re-run the script and the agent picks up where it left off — the
on-disk SQLite file this creates next to wherever you run it from
(``chat_memory.db``) accumulates state across runs, so re-runs are
conversations, not resets.

Run:
    python -m starter_kit.chat_memory
"""

from __future__ import annotations

import z3rno

from starter_kit._embed import hash_embed

TENANT_ID = "starter-kit-chat-agent"


def main() -> None:
    client = z3rno.Client(backend="embedded", path="chat_memory.db")

    # --- Turn 1: user shares a preference -----------------------------------
    preference = "I prefer dark mode and weekly digest emails."
    print(f"USER: {preference}")
    client.store(
        tenant_id=TENANT_ID,
        tier="semantic",  # stable preferences live in semantic memory
        content=preference,
        embedding=hash_embed(preference),
        metadata={"role": "user", "kind": "preference"},
    )
    print("AGENT: Got it — dark mode + weekly digests.\n")

    # --- Turn 2: user asks something the agent should answer from memory ----
    question = "Hey, can you remind me what email cadence I picked?"
    print(f"USER: {question}")
    results = client.recall(tenant_id=TENANT_ID, query=hash_embed(question), k=3)
    if results:
        top = results[0]
        print(f"AGENT: Based on what you told me — `{top['content']}`\n")
    else:
        print(
            "AGENT: I don't have anything in memory yet — try running this "
            "script twice in a row.\n"
        )

    # --- Turn 3: write the new exchange so memory keeps growing -------------
    client.store(
        tenant_id=TENANT_ID,
        tier="episodic",
        content=f"User asked: {question}. Agent answered from semantic memory.",
        embedding=hash_embed(question),
        metadata={"role": "agent", "turn": "3"},
    )
    print("Run this script again — turn 2 will recall what you just stored.")


if __name__ == "__main__":
    main()
