"""Naive local hashing embedding — ported bit-for-bit (integer arithmetic
and sign) from the canonical Rust implementation in ``cli/src/embed.rs``.

This is **not** a semantic embedding model. It's the classic feature-hashing
/ "hashing trick": lowercase + whitespace-tokenize the text, hash each token
into one of ``DIM`` buckets via FNV-1a, accumulate +1/-1 into that bucket
(the sign comes from a different bit of the same hash), then L2-normalize.
It exists purely so the Python, TypeScript, and Rust eval harnesses embed
`evals/fixtures/golden_v1.json` identically and their scores are comparable
across languages — see that Rust module's doc comment for the full
rationale. Floating-point rounding may differ by ~1e-6 from Rust's f32 path;
the token->bucket assignment and sign are exact integer/bitwise arithmetic
and must match exactly.
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
