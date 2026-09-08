import * as fs from "node:fs";
import * as path from "node:path";
import type { Client, MemoryDto } from "@z3rno/sdk";

import { hashEmbed } from "./embed";
import { faithfulness, latencyPercentiles, mrr, recallAtK, type LatencyPercentiles } from "./metrics";
import type { GoldenFixture } from "./fixture";

export interface ItemResult {
  id: string;
  query: string;
  tags: string[];
  retrievedIds: string[];
  expectedIds: string[];
  recallAtK: number;
  mrr: number;
  faithfulness: number;
  latencyMs: number;
}

export interface EvalReport {
  fixtureName: string;
  tenantId: string;
  seedCount: number;
  itemCount: number;
  items: ItemResult[];
  aggregate: {
    meanRecallAtK: number;
    meanMrr: number;
    meanFaithfulness: number;
    latency: LatencyPercentiles;
  };
}

function mean(values: number[]): number {
  const finite = values.filter((v) => !Number.isNaN(v));
  if (finite.length === 0) return NaN;
  return finite.reduce((s, v) => s + v, 0) / finite.length;
}

/**
 * Seeds every `fixture.seed` record, then runs every `fixture.items` query
 * against the recalled results, scoring recall@k/MRR/faithfulness per item
 * plus aggregate latency percentiles.
 */
export async function runEval(client: Client, fixture: GoldenFixture): Promise<EvalReport> {
  const seedIdMap = new Map<string, string>(); // fixture seed id -> real store()-returned id

  for (const seed of fixture.seed) {
    const stored = await client.store({
      tenantId: fixture.tenant_id,
      tier: seed.tier,
      content: seed.content,
      embedding: hashEmbed(seed.content),
      metadata: seed.metadata,
      links: [],
    });
    seedIdMap.set(seed.id, stored.id);
  }

  const items: ItemResult[] = [];
  const latencies: number[] = [];

  for (const item of fixture.items) {
    const start = performance.now();
    const recalled: MemoryDto[] = await client.recall({
      tenantId: fixture.tenant_id,
      query: hashEmbed(item.query),
      k: item.top_k,
    });
    const latencyMs = performance.now() - start;
    latencies.push(latencyMs);

    const retrievedIds = recalled.map((m) => m.id);
    const expectedIds = item.expected_seed_ids
      .map((fixtureId) => seedIdMap.get(fixtureId))
      .filter((id): id is string => id !== undefined);
    const recalledContext = recalled.map((m) => m.content).join(" ");

    items.push({
      id: item.id,
      query: item.query,
      tags: item.tags,
      retrievedIds,
      expectedIds,
      recallAtK: recallAtK(retrievedIds, expectedIds, item.top_k),
      mrr: mrr(retrievedIds, expectedIds),
      faithfulness: faithfulness(item.expected_answer, recalledContext),
      latencyMs,
    });
  }

  return {
    fixtureName: fixture.name,
    tenantId: fixture.tenant_id,
    seedCount: fixture.seed.length,
    itemCount: fixture.items.length,
    items,
    aggregate: {
      meanRecallAtK: mean(items.map((i) => i.recallAtK)),
      meanMrr: mean(items.map((i) => i.mrr)),
      meanFaithfulness: mean(items.map((i) => i.faithfulness)),
      latency: latencyPercentiles(latencies),
    },
  };
}

export function loadFixture(fixturePath: string): GoldenFixture {
  return JSON.parse(fs.readFileSync(fixturePath, "utf-8")) as GoldenFixture;
}

function reportMarkdown(report: EvalReport): string {
  const lines: string[] = [];
  lines.push(`# z3rno TypeScript eval report — ${report.fixtureName}`);
  lines.push("");
  lines.push(`Tenant: \`${report.tenantId}\` · seeded ${report.seedCount} memories · ${report.itemCount} queries`);
  lines.push("");
  lines.push("## Aggregate scores");
  lines.push("");
  lines.push("| metric | value |");
  lines.push("| --- | --- |");
  lines.push(`| mean recall@k | ${report.aggregate.meanRecallAtK.toFixed(4)} |`);
  lines.push(`| mean MRR | ${report.aggregate.meanMrr.toFixed(4)} |`);
  lines.push(`| mean faithfulness | ${report.aggregate.meanFaithfulness.toFixed(4)} |`);
  lines.push(`| latency p50 (ms) | ${report.aggregate.latency.p50.toFixed(2)} |`);
  lines.push(`| latency p95 (ms) | ${report.aggregate.latency.p95.toFixed(2)} |`);
  lines.push(`| latency p99 (ms) | ${report.aggregate.latency.p99.toFixed(2)} |`);
  lines.push(`| latency mean (ms) | ${report.aggregate.latency.mean.toFixed(2)} |`);
  lines.push("");
  lines.push("## Per-item results");
  lines.push("");
  lines.push("| id | tags | recall@k | mrr | faithfulness | latency (ms) |");
  lines.push("| --- | --- | --- | --- | --- | --- |");
  for (const item of report.items) {
    lines.push(
      `| ${item.id} | ${item.tags.join(",")} | ${item.recallAtK.toFixed(2)} | ${item.mrr.toFixed(2)} | ${item.faithfulness.toFixed(2)} | ${item.latencyMs.toFixed(2)} |`,
    );
  }
  lines.push("");
  return lines.join("\n");
}

export function writeReport(report: EvalReport, outDir: string): void {
  fs.mkdirSync(outDir, { recursive: true });
  fs.writeFileSync(path.join(outDir, "results.json"), JSON.stringify(report, null, 2));
  fs.writeFileSync(path.join(outDir, "report.md"), reportMarkdown(report));
}
