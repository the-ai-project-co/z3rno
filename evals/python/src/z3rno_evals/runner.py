"""Runs the golden-dataset eval against a live `z3rno.Client` and produces
a machine-readable `results.json` plus a human-readable `report.md`.
"""

import json
import math
import time
from pathlib import Path
from typing import Any

from .embed import hash_embed
from .metrics import LatencyPercentiles, faithfulness, latency_percentiles, mrr, recall_at_k


def load_fixture(path: Path) -> dict[str, Any]:
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def _mean_skip_nan(values: list[float]) -> float:
    clean = [v for v in values if not math.isnan(v)]
    if not clean:
        return float("nan")
    return sum(clean) / len(clean)


def run_eval(client: Any, fixture: dict[str, Any]) -> dict[str, Any]:
    """Seeds `fixture["seed"]` into `client`, runs `fixture["items"]` as
    recall queries, and returns a results dict with per-item scores and
    aggregates. `client` is a `z3rno.Client` (or anything matching its
    store/recall interface).
    """
    tenant_id = fixture["tenant_id"]

    # Seed every memory, remembering fixture id -> real store()-returned id.
    seed_id_map: dict[str, str] = {}
    for record in fixture["seed"]:
        embedding = hash_embed(record["content"])
        memory = client.store(
            tenant_id=tenant_id,
            tier=record["tier"],
            content=record["content"],
            embedding=embedding,
            metadata=record.get("metadata") or {},
        )
        seed_id_map[record["id"]] = memory["id"]

    per_item: list[dict[str, Any]] = []
    latencies_ms: list[float] = []

    for item in fixture["items"]:
        top_k = item.get("top_k", 5)
        query_vec = hash_embed(item["query"])

        start = time.perf_counter()
        retrieved = client.recall(tenant_id=tenant_id, query=query_vec, k=top_k)
        latency_ms = (time.perf_counter() - start) * 1000.0
        latencies_ms.append(latency_ms)

        retrieved_ids = [m["id"] for m in retrieved]
        expected_ids = [seed_id_map[sid] for sid in item["expected_seed_ids"]]

        recalled_context = " ".join(m["content"] for m in retrieved)

        r_at_k = recall_at_k(retrieved_ids, expected_ids, top_k)
        r_mrr = mrr(retrieved_ids, expected_ids)
        r_faith = faithfulness(item.get("expected_answer", ""), recalled_context)

        per_item.append(
            {
                "id": item["id"],
                "query": item["query"],
                "tags": item.get("tags", []),
                "recall_at_k": r_at_k,
                "mrr": r_mrr,
                "faithfulness": r_faith,
                "latency_ms": latency_ms,
                "retrieved_ids": retrieved_ids,
                "expected_ids": expected_ids,
            }
        )

    aggregates = {
        "recall_at_k_mean": _mean_skip_nan([i["recall_at_k"] for i in per_item]),
        "mrr_mean": _mean_skip_nan([i["mrr"] for i in per_item]),
        "faithfulness_mean": _mean_skip_nan([i["faithfulness"] for i in per_item]),
        "latency": _latency_dict(latency_percentiles(latencies_ms)),
        "item_count": len(per_item),
    }

    return {
        "dataset": fixture.get("name", "unknown"),
        "tenant_id": tenant_id,
        "items": per_item,
        "aggregates": aggregates,
    }


def _latency_dict(p: LatencyPercentiles) -> dict[str, float]:
    return {"p50": p.p50, "p95": p.p95, "p99": p.p99, "mean": p.mean, "count": p.count}


def write_results(results: dict[str, Any], out_dir: Path) -> tuple[Path, Path]:
    out_dir.mkdir(parents=True, exist_ok=True)

    results_path = out_dir / "results.json"
    results_path.write_text(json.dumps(results, indent=2), encoding="utf-8")

    report_path = out_dir / "report.md"
    report_path.write_text(_render_report_md(results), encoding="utf-8")

    return results_path, report_path


def _render_report_md(results: dict[str, Any]) -> str:
    agg = results["aggregates"]
    lat = agg["latency"]
    lines = [
        f"# z3rno-evals report — {results['dataset']}",
        "",
        "## Aggregates",
        "",
        f"- recall@k (mean): {agg['recall_at_k_mean']:.4f}",
        f"- MRR (mean): {agg['mrr_mean']:.4f}",
        f"- faithfulness (mean): {agg['faithfulness_mean']:.4f}",
        f"- latency p50/p95/p99 (ms): {lat['p50']:.2f} / {lat['p95']:.2f} / {lat['p99']:.2f}",
        f"- latency mean (ms): {lat['mean']:.2f}",
        f"- items: {agg['item_count']}",
        "",
        "## Per-item",
        "",
        "| id | recall@k | mrr | faithfulness | latency_ms | tags |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for item in results["items"]:
        lines.append(
            f"| {item['id']} | {item['recall_at_k']:.3f} | {item['mrr']:.3f} "
            f"| {item['faithfulness']:.3f} | {item['latency_ms']:.2f} | {', '.join(item['tags'])} |"
        )
    lines.append("")
    return "\n".join(lines)
