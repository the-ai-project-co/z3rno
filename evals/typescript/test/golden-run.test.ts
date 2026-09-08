import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { Client } from "@z3rno/sdk";

import { hashEmbed } from "../src/embed";
import { loadFixture, runEval, writeReport } from "../src/runner";

// __dirname at runtime is dist/test/ (compiled output) -> up to dist/ -> up
// to evals/typescript/ -> up to evals/ -> fixtures/golden_v1.json.
const FIXTURE_PATH = path.join(__dirname, "..", "..", "..", "fixtures", "golden_v1.json");
const RESULTS_DIR = path.join(__dirname, "..", "..", "results");

async function freshClient(): Promise<Client> {
  const dbPath = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "z3rno-ts-eval-")), "z3rno.db");
  return Client.connect({ backend: "embedded", path: dbPath });
}

// Baseline measured by actually running this suite against golden_v1.json
// with the naive hashing embedding (2026-09-08, embedded SQLite backend,
// 10 seeded memories, 6 queries):
//   meanRecallAtK    = 0.8333 (5/6 queries hit; q-006 misses entirely —
//                       the naive bag-of-words hash just doesn't connect
//                       "bug ... performance" to the m-008 wording)
//   meanMrr          = 0.8333 (same 5/6 pattern)
//   meanFaithfulness = 0.7917 (q-005 partial at 0.75, q-006 at 0)
//   latency p50/p95/p99 ~ 0.16-1.5ms (in-process embedded backend, tiny dataset)
// Thresholds below are set with headroom under that observed baseline, not
// invented — rerun this suite and update both the comment and the constants
// if the dataset or the embedding algorithm changes.
const MIN_MEAN_RECALL_AT_K = 0.7;
const MIN_MEAN_MRR = 0.7;
const MIN_MEAN_FAITHFULNESS = 0.65;
const MAX_P95_LATENCY_MS = 500; // generous vs the ~1.5ms observed; matches the fixture's own latency_budget_ms

test("golden_v1 eval run meets regression thresholds", async () => {
  const client = await freshClient();
  const fixture = loadFixture(FIXTURE_PATH);

  const report = await runEval(client, fixture);
  writeReport(report, RESULTS_DIR);

  assert.ok(
    report.aggregate.meanRecallAtK >= MIN_MEAN_RECALL_AT_K,
    `meanRecallAtK ${report.aggregate.meanRecallAtK} below threshold ${MIN_MEAN_RECALL_AT_K}`,
  );
  assert.ok(
    report.aggregate.meanMrr >= MIN_MEAN_MRR,
    `meanMrr ${report.aggregate.meanMrr} below threshold ${MIN_MEAN_MRR}`,
  );
  assert.ok(
    report.aggregate.meanFaithfulness >= MIN_MEAN_FAITHFULNESS,
    `meanFaithfulness ${report.aggregate.meanFaithfulness} below threshold ${MIN_MEAN_FAITHFULNESS}`,
  );
  assert.ok(
    report.aggregate.latency.p95 <= MAX_P95_LATENCY_MS,
    `p95 latency ${report.aggregate.latency.p95}ms above ceiling ${MAX_P95_LATENCY_MS}ms`,
  );
});

test("forget removes a seeded memory from recall (regression case)", async () => {
  const client = await freshClient();
  const fixture = loadFixture(FIXTURE_PATH);

  // Seed just the record targeted by q-001 ("m-001" -> dark mode preferences).
  const seed = fixture.seed.find((s) => s.id === "m-001")!;
  const item = fixture.items.find((i) => i.id === "q-001")!;

  const stored = await client.store({
    tenantId: fixture.tenant_id,
    tier: seed.tier,
    content: seed.content,
    embedding: hashEmbed(seed.content),
    metadata: seed.metadata,
    links: [],
  });

  const before = await client.recall({
    tenantId: fixture.tenant_id,
    query: hashEmbed(item.query),
    k: item.top_k,
  });
  assert.ok(
    before.some((m) => m.id === stored.id),
    "expected the seeded memory to be recallable before forgetting",
  );

  const proof = await client.forget({ tenantId: fixture.tenant_id, id: stored.id });
  assert.ok(proof, "forget() should return a proof for a memory that exists");

  const after = await client.recall({
    tenantId: fixture.tenant_id,
    query: hashEmbed(item.query),
    k: item.top_k,
  });
  assert.ok(
    !after.some((m) => m.id === stored.id),
    "forgotten memory must not be recallable any more",
  );
});
