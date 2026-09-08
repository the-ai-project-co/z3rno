/** Shape of `evals/fixtures/golden_v1.json`. See that file for the real data. */

export interface SeedRecord {
  id: string;
  tier: string;
  content: string;
  metadata: Record<string, unknown>;
}

export interface EvalItem {
  id: string;
  query: string;
  expected_seed_ids: string[];
  expected_answer: string;
  top_k: number;
  latency_budget_ms: number;
  tags: string[];
}

export interface GoldenFixture {
  version: number;
  name: string;
  description: string;
  tenant_id: string;
  seed: SeedRecord[];
  items: EvalItem[];
}
