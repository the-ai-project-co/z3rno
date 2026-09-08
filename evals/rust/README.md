# z3rno-evals (Rust)

The Rust golden-dataset regression harness for `z3rno-engine` (slice 0008.4).
It runs the shared fixture at `evals/fixtures/golden_v1.json` end-to-end
through a real `MemoryEngine::embedded(...)` — seeding every record with
`store`, scoring every query with `recall`, and exercising `forget` — and
asserts the results stay above empirically-measured thresholds.

## Running it

```
cargo test -p z3rno-evals
```

To see the actual aggregate scores (useful after editing the fixture or
touching retrieval):

```
cargo test -p z3rno-evals -- --nocapture
```

Because `evals/rust` is a normal Cargo workspace member, this also runs
automatically as part of `cargo test --workspace` — the existing CI `test`
job in `.github/workflows/ci.yml` already invokes that, so this harness runs
on every PR with no extra workflow wiring.

## Metrics

- **recall@k** — the fraction of an item's expected memory ids that show up
  anywhere in the top-`k` results `recall` returned. `1.0` means every
  expected memory was found, `0.0` means none were.
- **MRR (mean reciprocal rank)** — `1 / rank` of the first expected memory
  found in the results (rank 1-indexed), averaged across items. Rewards
  finding the right memory near the top of the list, not just anywhere in it.
- **latency percentiles** — p50/p95/p99/mean wall-clock time (ms) of each
  `recall` call, computed with the nearest-rank method.
- **faithfulness** — a deterministic word-coverage heuristic: strip
  stopwords from an item's `expected_answer`, then check what fraction of
  the remaining words also appear in the memories `recall` actually
  returned.

## On "faithfulness"

This is **not** an LLM-judged score — it's a crude, dependency-free
word-overlap stub judge, the same "stub judge" framing the old pre-rewrite
evals package used: good enough to catch a real regression in CI (recall
starts returning garbage, an embedding change breaks matching, etc.), not a
claim about whether the recalled content actually answers the question. A
real quality bar for answer faithfulness needs an actual judge (LLM or
human), not this.

## Thresholds

The pass/fail floors in `tests/golden_run.rs` are empirically derived from
running this exact 10-seed/6-query dataset through the naive hashing
embedding (`z3rno-hash-embed`) — not copied from anywhere else, since this
retrieval mechanism (crude feature hashing, not a semantic model) has no
comparable baseline elsewhere in the repo. See the comment next to the
threshold constants for the measured numbers and the headroom built in.
