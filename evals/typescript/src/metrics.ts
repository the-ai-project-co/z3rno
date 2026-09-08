/** Pure scoring functions shared by the eval runner. No SDK/IO dependency. */

/** Fraction of `expectedIds` present in `retrievedIds.slice(0, k)`. NaN if `expectedIds` is empty. */
export function recallAtK(retrievedIds: string[], expectedIds: string[], k: number): number {
  if (expectedIds.length === 0) return NaN;
  const top = new Set(retrievedIds.slice(0, k));
  const hits = expectedIds.filter((id) => top.has(id)).length;
  return hits / expectedIds.length;
}

/**
 * Reciprocal rank (1-indexed) of the first `expectedIds` member found in
 * `retrievedIds`. NaN if `expectedIds` is empty, 0.0 if none is found.
 */
export function mrr(retrievedIds: string[], expectedIds: string[]): number {
  if (expectedIds.length === 0) return NaN;
  const expected = new Set(expectedIds);
  for (let i = 0; i < retrievedIds.length; i++) {
    if (expected.has(retrievedIds[i])) return 1 / (i + 1);
  }
  return 0.0;
}

export interface LatencyPercentiles {
  p50: number;
  p95: number;
  p99: number;
  mean: number;
  count: number;
}

/** Nearest-rank percentiles. Empty input -> all zero, count 0. */
export function latencyPercentiles(latenciesMs: number[]): LatencyPercentiles {
  const n = latenciesMs.length;
  if (n === 0) return { p50: 0, p95: 0, p99: 0, mean: 0, count: 0 };
  const sorted = [...latenciesMs].sort((a, b) => a - b);
  const clamp = (i: number) => Math.min(Math.max(i, 0), n - 1);
  const nearestRank = (p: number) => sorted[clamp(Math.ceil(p * n) - 1)];
  const mean = sorted.reduce((s, x) => s + x, 0) / n;
  return {
    p50: nearestRank(0.5),
    p95: nearestRank(0.95),
    p99: nearestRank(0.99),
    mean,
    count: n,
  };
}

const STOP_WORDS = new Set([
  "the",
  "a",
  "an",
  "is",
  "are",
  "was",
  "were",
  "to",
  "of",
  "in",
  "on",
  "at",
  "and",
  "or",
  "for",
]);

/**
 * Deterministic word-coverage stub judge (no LLM/API key) — ported from the
 * pre-rewrite Python evals package's StubJudge. NOT a real answer-quality
 * judge, just a regression-detection heuristic good enough for CI.
 */
export function faithfulness(expectedAnswer: string, recalledContext: string): number {
  if (expectedAnswer.trim().length === 0) return NaN;

  const clean = (s: string) => s.replace(/[^\w\s]/g, " ").toLowerCase();

  const expectedTokens = new Set(
    clean(expectedAnswer)
      .split(/\s+/)
      .filter((t) => t.length > 0 && !STOP_WORDS.has(t)),
  );
  if (expectedTokens.size === 0) return NaN;

  const ctxTokens = new Set(
    clean(recalledContext)
      .split(/\s+/)
      .filter((t) => t.length > 0),
  );

  let hits = 0;
  for (const t of expectedTokens) {
    if (ctxTokens.has(t)) hits++;
  }
  return hits / expectedTokens.size;
}
