"""Naive local hashing embedding — ported bit-for-bit (integer arithmetic
and sign) from the canonical Rust implementation in ``hash-embed/src/lib.rs``.

``z3rno.Client.store``/``recall`` take a raw embedding vector, not a text
query, so every example in this starter kit calls ``hash_embed`` to turn
its content/query text into one. This is **not** a semantic embedding
model — it's the classic feature-hashing / "hashing trick": lowercase +
whitespace-tokenize the text, hash each token into one of ``DIM`` buckets
via FNV-1a, accumulate +1/-1 into that bucket (the sign comes from a
different bit of the same hash), then L2-normalize. It exists purely so
these examples run against the embedded backend with zero external
services or API keys — see ``evals/README.md`` for the fuller rationale
and its use across the CLI, MCP server, and eval harnesses.
"""

DIM = 128
OFFSET_BASIS = 0xCBF29CE484222325
PRIME = 0x100000001B3
MASK = 0xFFFFFFFFFFFFFFFF


def fnv1a(data: bytes) -> int:
    """FNV-1a over `data`, wrapped to 64 bits at every multiply step."""
    h = OFFSET_BASIS
    for byte in data:
        h ^= byte
        h = (h * PRIME) & MASK
    return h


def hash_embed(text: str) -> list[float]:
    """Embeds `text` into a `DIM`-dimensional, L2-normalized vector."""
    v = [0.0] * DIM
    for token in text.lower().split():
        h = fnv1a(token.encode("utf-8"))
        bucket = h % DIM
        sign = 1.0 if ((h >> 32) & 1) == 0 else -1.0
        v[bucket] += sign
    norm = sum(x * x for x in v) ** 0.5
    if norm > 0:
        v = [x / norm for x in v]
    return v
