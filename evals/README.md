# evals

Three independent eval harnesses — [Rust](rust/), [Python](python/), and
[TypeScript](typescript/) — that score `z3rno`'s memory engine against the
same golden dataset, one per binding, so recall quality can be checked
end-to-end through each language's real client rather than only at the
Rust engine layer.

## The shared fixture

[`fixtures/golden_v1.json`](fixtures/golden_v1.json) is the single source of
truth: 10 seed memories spanning all four tiers (`working`/`episodic`/
`semantic`/`procedural`) and 6 queries, each with the ids of the seed
memories it expects `recall` to surface. All three harnesses:

1. Seed every record in `seed` through their own client's `store`.
2. Run every query in `items` through `recall`, resolving each fixture id to
   the real id the store call returned.
3. Score the results with the same four metrics — recall@k, MRR, latency
   percentiles, faithfulness (a deterministic word-coverage stub judge, not
   an LLM judge — see each harness's own README for why).
4. Embed all text (`content` and `query`) with the identical naive local
   hashing embedding — FNV-1a feature hashing into 128 dimensions,
   L2-normalized. It's a deliberately crude, zero-dependency, zero-API-key
   bag-of-words placeholder, **not a semantic model**; it exists so every
   harness (and the CLI) works end-to-end with no external service. The
   canonical implementation is the Rust crate [`hash-embed`](../hash-embed/),
   reused directly by `cli/` and `evals/rust/`; `evals/python/` and
   `evals/typescript/` carry verified byte-for-byte ports (`embed.py`,
   `embed.ts`) — each harness's test suite asserts its port against the same
   reference vectors as the Rust original, so a query embedded in any of the
   three lands in the same vector space.

Editing the fixture changes what all three harnesses score — keep it in one
place, and re-run all three suites after touching it.

## Independently measured thresholds

Each harness measures its own baseline against this dataset through its own
client and backend, then sets pass/fail thresholds below that measured
baseline with headroom — not a single shared number. This is deliberate:
the embedded backend's vector search is HNSW-based approximate nearest
neighbor, rebuilt per search from a `HashMap`'s randomized iteration order,
so exact scores vary slightly run-to-run even with fully deterministic
embeddings (the Python harness measured this directly, running the dataset
20 times to find real bounds). The numbers also aren't comparable to the
deprecated pre-rewrite `z3rno-evals` package's acceptance bars
(recall@5≥0.80, mrr≥0.65, faithfulness≥0.90) — those were tuned for a real
semantic/graph retrieval strategy, not this naive hashing placeholder. See
each harness's own README and its test file's threshold comments for the
measured numbers.

## Running all three

```sh
# Rust — runs automatically as part of cargo test --workspace, or standalone:
cargo test -p z3rno-evals

# Python — builds the PyO3 bindings from source first (not on PyPI yet):
python3 -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop --manifest-path bindings/python/Cargo.toml
pip install -e "evals/python[dev]"
pytest evals/python

# TypeScript — builds the native addon first (not on npm yet):
cd bindings/typescript && npm install && npm run build && cd -
cd evals/typescript && npm install && npm test
```

CI runs all three independently: Rust rides the existing `test` job in
[`ci.yml`](../.github/workflows/ci.yml) (it's just another workspace
member); Python and TypeScript each have their own workflow
([`ci-evals-python.yml`](../.github/workflows/ci-evals-python.yml),
[`ci-evals-typescript.yml`](../.github/workflows/ci-evals-typescript.yml))
since neither toolchain was previously set up in CI.

## What this is not

None of these harnesses judge answer quality with an LLM, and none require
an API key — `faithfulness` is a stub word-overlap heuristic in all three,
useful for catching a real regression in CI (recall returning garbage, an
embedding change breaking matching) but not a claim about whether recalled
content actually answers the question. A real quality bar needs an actual
judge, human or LLM, layered on top of this — out of scope here.
