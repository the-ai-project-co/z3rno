"""Regression-gated pytest run of the full eval harness against the shared
golden dataset (`evals/fixtures/golden_v1.json`), through a real
`z3rno.Client` backed by a temp SQLite path.

Threshold provenance: the embedded backend's vector search is HNSW-based
(approximate nearest neighbor — see engine/src/backend/embedded/vector.rs),
and its per-search index is rebuilt from a `HashMap`'s (randomized)
iteration order, so aggregate scores vary slightly run-to-run even though
the fixture and embeddings are fully deterministic. Thresholds below were
set from 20 observed runs of this exact dataset/pipeline, not invented:

    recall_at_k_mean : min 0.750, max 1.000, avg 0.846
    mrr_mean         : min 0.833, max 0.867, avg 0.837
    faithfulness_mean: min 0.771, max 0.979, avg 0.818
    latency p95 (ms) : min 0.47,  max 8.45,  avg well under 1ms

Each threshold below sits with real headroom under the observed minimum.
"""

import tempfile
from pathlib import Path

import pytest
import z3rno

from z3rno_evals.runner import load_fixture, run_eval

FIXTURE_PATH = Path(__file__).resolve().parents[3] / "evals" / "fixtures" / "golden_v1.json"

# See module docstring for how these were derived.
RECALL_AT_K_MIN = 0.6
MRR_MIN = 0.7
FAITHFULNESS_MIN = 0.65
LATENCY_P95_MS_MAX = 500.0  # generous ceiling; observed max was ~8.5ms


@pytest.fixture
def client(tmp_path):
    db_path = tmp_path / "eval.db"
    return z3rno.Client(backend="embedded", path=str(db_path))


@pytest.fixture
def fixture():
    return load_fixture(FIXTURE_PATH)


def test_fixture_loads():
    fx = load_fixture(FIXTURE_PATH)
    assert fx["name"] == "golden_v1"
    assert len(fx["seed"]) > 0
    assert len(fx["items"]) > 0


def test_golden_run_meets_regression_thresholds(client, fixture):
    results = run_eval(client, fixture)
    agg = results["aggregates"]

    assert agg["recall_at_k_mean"] >= RECALL_AT_K_MIN, agg
    assert agg["mrr_mean"] >= MRR_MIN, agg
    assert agg["faithfulness_mean"] >= FAITHFULNESS_MIN, agg
    assert agg["latency"]["p95"] <= LATENCY_P95_MS_MAX, agg
    assert agg["item_count"] == len(fixture["items"])


def test_forget_removes_memory_from_recall(client, fixture):
    tenant_id = fixture["tenant_id"]
    from z3rno_evals.embed import hash_embed

    # Seed just the one record this case cares about.
    target = next(r for r in fixture["seed"] if r["id"] == "m-004")
    memory = client.store(
        tenant_id=tenant_id,
        tier=target["tier"],
        content=target["content"],
        embedding=hash_embed(target["content"]),
        metadata=target.get("metadata") or {},
    )

    query = "How do I deploy to staging?"
    before = client.recall(tenant_id=tenant_id, query=hash_embed(query), k=5)
    assert any(m["id"] == memory["id"] for m in before)

    proof = client.forget(tenant_id=tenant_id, id=memory["id"])
    assert proof is not None
    assert "audit_event_id" in proof

    after = client.recall(tenant_id=tenant_id, query=hash_embed(query), k=5)
    assert all(m["id"] != memory["id"] for m in after)

    # Forgetting again is a no-op, not an error.
    assert client.forget(tenant_id=tenant_id, id=memory["id"]) is None


def test_write_results_produces_report_files(client, fixture, tmp_path):
    from z3rno_evals.runner import write_results

    results = run_eval(client, fixture)
    out_dir = tmp_path / "results"
    results_path, report_path = write_results(results, out_dir)

    assert results_path.exists()
    assert report_path.exists()
    assert "recall@k" in report_path.read_text()
