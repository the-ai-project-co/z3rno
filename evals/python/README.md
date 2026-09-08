# z3rno-evals (Python)

The Python eval harness for `z3rno`'s memory engine. It seeds the shared golden dataset (`evals/fixtures/golden_v1.json`, shared with the TypeScript and Rust harnesses) into a real `z3rno.Client`, runs every query in the dataset through `recall`, and scores the results against the dataset's expected answers.

## Run it locally

The `z3rno` package isn't published to PyPI yet, so it's built from source with `maturin` rather than installed as a normal dependency:

```sh
cd <repo root>
python3 -m venv .venv && source .venv/bin/activate

pip install maturin
maturin develop --manifest-path bindings/python/Cargo.toml

pip install -e "evals/python[dev]"

pytest evals/python
```

`maturin develop` builds the PyO3 extension module and installs it editable into the active virtualenv — rerun it after any change to `bindings/python` or `engine`.

To produce `results.json`/`report.md` without pytest's assertions:

```python
from pathlib import Path
import z3rno
from z3rno_evals.runner import load_fixture, run_eval, write_results

client = z3rno.Client(backend="embedded", path="eval.db")
fixture = load_fixture(Path("evals/fixtures/golden_v1.json"))
results = run_eval(client, fixture)
write_results(results, Path("evals/python/results"))
```

`evals/python/results/` is gitignored — it's regenerated output, not something to commit.

## Metrics

- **recall@k** — the fraction of a query's expected memory ids that appear anywhere in the top `k` results `recall()` returns. 1.0 means every expected memory was found within the budget; NaN if a query has no expected ids.
- **MRR (mean reciprocal rank)** — `1 / rank` of the first expected id found in the recalled list (1-indexed), averaged across queries; 0.0 if none of a query's expected ids were found at all, NaN if a query has no expected ids.
- **latency percentiles** — p50/p95/p99/mean wall-clock time of each `recall()` call in milliseconds, computed with the nearest-rank method over all queries in the run.
- **faithfulness** — a deterministic word-coverage heuristic (`z3rno_evals.metrics.faithfulness`): the fraction of a query's expected-answer's non-stopword tokens that also appear in the recalled memories' joined content. **This is not a real answer-quality judge** — it's a cheap, dependency-free, no-API-key regression signal ported from the pre-rewrite evals package's stub judge, good enough to catch a recall pipeline getting materially worse in CI, not a substitute for human or LLM-graded evaluation.

## The embedding

`z3rno_evals/embed.py` reimplements the naive local hashing embedding from `cli/src/embed.rs` (FNV-1a feature hashing into 128 dimensions, L2-normalized) bit-for-bit at the integer/bucket/sign level, so the Python, TypeScript, and Rust harnesses embed `golden_v1.json`'s text identically and their scores are comparable. It is **not** a semantic embedding model — see that module's docstring and the Rust source for what it is and isn't.

## Regression gate

`tests/test_golden_run.py` runs the full harness against `golden_v1.json` through a real embedded `z3rno.Client` and asserts aggregate scores stay above thresholds measured from this exact pipeline (see that file's module docstring for the observed baseline and how the thresholds were derived from it).
