"""Pure scoring functions for the eval harness. No store/recall calls here —
each function takes plain lists/strings in and a score out, so they're unit
testable on their own (see ``tests/test_metrics.py``).
"""

import math
import re
from dataclasses import dataclass


def recall_at_k(retrieved_ids: list[str], expected_ids: list[str], k: int) -> float:
    """Fraction of `expected_ids` present in `retrieved_ids[:k]`.

    NaN if `expected_ids` is empty (nothing to have recalled).
    """
    if not expected_ids:
        return float("nan")
    top_k = set(retrieved_ids[:k])
    hits = sum(1 for eid in expected_ids if eid in top_k)
    return hits / len(expected_ids)


def mrr(retrieved_ids: list[str], expected_ids: list[str]) -> float:
    """Reciprocal rank (1-indexed) of the first expected id found anywhere
    in `retrieved_ids`.

    NaN if `expected_ids` is empty, 0.0 if none of them appear at all.
    """
    if not expected_ids:
        return float("nan")
    expected = set(expected_ids)
    for i, rid in enumerate(retrieved_ids, start=1):
        if rid in expected:
            return 1.0 / i
    return 0.0


@dataclass
class LatencyPercentiles:
    p50: float
    p95: float
    p99: float
    mean: float
    count: int


def latency_percentiles(latencies_ms: list[float]) -> LatencyPercentiles:
    """Nearest-rank percentiles over `latencies_ms`.

    idx = clamp(ceil(p * n) - 1, 0, n - 1) into the ascending-sorted list,
    for p in {0.50, 0.95, 0.99}. Empty input -> all-zero, count 0.
    """
    n = len(latencies_ms)
    if n == 0:
        return LatencyPercentiles(p50=0.0, p95=0.0, p99=0.0, mean=0.0, count=0)

    ordered = sorted(latencies_ms)

    def pct(p: float) -> float:
        idx = max(0, min(n - 1, math.ceil(p * n) - 1))
        return ordered[idx]

    return LatencyPercentiles(
        p50=pct(0.50),
        p95=pct(0.95),
        p99=pct(0.99),
        mean=sum(ordered) / n,
        count=n,
    )


_STOP_WORDS = {
    "the", "a", "an", "is", "are", "was", "were", "to", "of",
    "in", "on", "at", "and", "or", "for",
}
_PUNCT_RE = re.compile(r"[^\w\s]")


def faithfulness(expected_answer: str, recalled_context: str) -> float:
    """Deterministic word-coverage stub judge (no LLM/API key) — ported from
    the pre-rewrite Python evals package's StubJudge. NOT a real
    answer-quality judge, just a regression-detection heuristic good enough
    for CI: fraction of `expected_answer`'s non-stopword tokens that appear
    anywhere in `recalled_context`.
    """
    if not expected_answer or not expected_answer.strip():
        return float("nan")
    clean_expected = _PUNCT_RE.sub(" ", expected_answer.casefold())
    expected_tokens = {w for w in clean_expected.split() if w and w not in _STOP_WORDS}
    if not expected_tokens:
        return float("nan")
    ctx = _PUNCT_RE.sub(" ", recalled_context.casefold())
    ctx_tokens = set(ctx.split())
    hits = sum(1 for t in expected_tokens if t in ctx_tokens)
    return hits / len(expected_tokens)
