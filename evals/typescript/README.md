# evals/typescript

The TypeScript eval harness for z3rno. Seeds the shared golden dataset
(`evals/fixtures/golden_v1.json`) into a real `@z3rno/sdk` `Client`, runs
each fixture query through `recall()`, and scores the results — recall@k,
MRR, a heuristic faithfulness score, and latency percentiles. Plain
TypeScript compiled with `tsc`, tested with Node's built-in test runner
(`node --test`) — no test framework, no build framework.

Two other harnesses (Python, Rust) score the same fixture in parallel. All
three embed text with the identical naive hashing embedding
(`src/embed.ts`, ported bit-for-bit from `hash-embed/src/lib.rs`) so scores
are comparable across languages.

## Build and run locally

`@z3rno/sdk` (`bindings/typescript`) is not published to npm yet, so this
package depends on it via a local `file:` path and needs its native addon
built first:

```bash
# 1. Build the native addon this package depends on.
cd bindings/typescript
npm install
npm run build

# 2. Build and test the eval harness.
cd ../../evals/typescript
npm install
npm test          # tsc, then node --test against dist/test/*.test.js
```

`npm test` writes `results/results.json` and `results/report.md` (both
gitignored — regenerated output, not checked in) and asserts real
regression thresholds on the aggregate scores; see the constants and
measured-baseline comment at the top of `test/golden-run.test.ts`.

## Metrics

- **recall@k** (`src/metrics.ts`): the fraction of a query's expected
  memory ids that appear anywhere in the top-`k` recalled results. `NaN`
  when a query has no expected ids (nothing to measure).
- **MRR** (mean reciprocal rank): `1 / rank` of the first expected id found
  in the recalled results (1-indexed), or `0` if none of them show up at
  all. `NaN` when a query has no expected ids.
- **latency percentiles**: p50/p95/p99/mean over every query's `recall()`
  wall-clock time, computed with the nearest-rank method.
- **faithfulness**: a deterministic word-coverage heuristic, **not a real
  answer-quality judge** — no LLM call, no API key. It lowercases and
  strips punctuation from the expected answer and the recalled context,
  drops a short stop-word list from the expected side only, and reports
  what fraction of the remaining expected words appear anywhere in the
  recalled context. Good enough to catch a recall regression in CI; not a
  substitute for real judged evaluation.

## Not a semantic embedding

`src/embed.ts`'s `hashEmbed` is feature hashing (FNV-1a into 128 buckets,
sign from a separate hash bit, L2-normalized) — a crude bag-of-words
similarity with no notion of synonyms, word order, or meaning. It exists so
this harness (and its Python/Rust siblings) work with zero external
services and zero API keys. The 64-bit FNV-1a arithmetic is done in
`BigInt` since JS's `number` can't represent it exactly past 2^53 — the
token-to-bucket assignment and sign must match the Rust/Python
implementations exactly for cross-language scores to be comparable.
