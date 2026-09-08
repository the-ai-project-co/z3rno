"""Naive local hashing embedding — a fallback for tool calls that don't
supply a real ``embedding``.

This is **not** a semantic embedding model. It's the classic feature-hashing
/ "hashing trick": lowercase + whitespace-tokenize the text, hash each token
into one of ``DIM`` buckets, accumulate +1/-1 into that bucket, then
L2-normalize. Two pieces of text that share more words end up with a higher
dot product — a crude bag-of-words similarity, nothing more. It has no
notion of synonyms, word order, or meaning.

It exists so ``z3rno_store``/``z3rno_recall`` work end-to-end even when the
calling LLM doesn't pass a real embedding vector. For anything beyond
kicking the tires, pass real embeddings from an LLM provider via the tools'
``embedding`` argument.

This is a byte-for-byte Python port of the same algorithm implemented in
Rust at ``hash-embed/src/lib.rs`` (also reused by the CLI and the eval
harnesses), so a query embedded here lands in the same vector space as
memories embedded by the CLI or SDKs using their own hash_embed.
"""

DIM = 128
OFFSET_BASIS = 0xCBF29CE484222325
PRIME = 0x100000001B3
MASK = 0xFFFFFFFFFFFFFFFF


def fnv1a(data: bytes) -> int:
    h = OFFSET_BASIS
    for byte in data:
        h ^= byte
        h = (h * PRIME) & MASK
    return h


def hash_embed(text: str) -> list[float]:
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
