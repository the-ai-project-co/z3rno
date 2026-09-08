import { test } from "node:test";
import assert from "node:assert/strict";

import { faithfulness, latencyPercentiles, mrr, recallAtK } from "../src/metrics";

test("recallAtK: normal case, hand-computed", () => {
  // 2 of 3 expected ids are in the top 3 retrieved.
  const retrieved = ["a", "b", "c", "d"];
  const expected = ["b", "d", "z"];
  assert.equal(recallAtK(retrieved, expected, 3), 1 / 3); // top-3 = [a,b,c] -> only "b" hits
  assert.equal(recallAtK(retrieved, expected, 4), 2 / 3); // top-4 = [a,b,c,d] -> "b","d" hit
});

test("recallAtK: NaN when expectedIds is empty", () => {
  assert.ok(Number.isNaN(recallAtK(["a", "b"], [], 5)));
});

test("recallAtK: zero-hit case", () => {
  assert.equal(recallAtK(["a", "b"], ["x", "y"], 5), 0);
});

test("mrr: normal case, hand-computed", () => {
  // first expected match ("c") is at 1-indexed rank 3 -> reciprocal rank 1/3
  assert.equal(mrr(["a", "b", "c", "d"], ["c", "d"]), 1 / 3);
});

test("mrr: NaN when expectedIds is empty", () => {
  assert.ok(Number.isNaN(mrr(["a", "b"], [])));
});

test("mrr: 0.0 when none found", () => {
  assert.equal(mrr(["a", "b"], ["z"]), 0.0);
});

test("latencyPercentiles: normal case, hand-computed (nearest-rank, n=10)", () => {
  const latencies = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
  const p = latencyPercentiles(latencies);
  // p50: ceil(0.5*10)-1 = 4 -> sorted[4] = 50
  assert.equal(p.p50, 50);
  // p95: ceil(0.95*10)-1 = 9 -> sorted[9] = 100
  assert.equal(p.p95, 100);
  // p99: ceil(0.99*10)-1 = 9 -> sorted[9] = 100
  assert.equal(p.p99, 100);
  assert.equal(p.mean, 55);
  assert.equal(p.count, 10);
});

test("latencyPercentiles: empty input -> all zero, count 0", () => {
  const p = latencyPercentiles([]);
  assert.deepEqual(p, { p50: 0, p95: 0, p99: 0, mean: 0, count: 0 });
});

test("faithfulness: normal case, hand-computed", () => {
  // expected tokens (stop words "the"/"and" stripped): {user, prefers, dark,
  // mode, weekly, digest, emails} = 7 tokens. Context keeps all its words
  // (no stop-word filtering on that side, per spec).
  const expected = "The user prefers dark mode and weekly digest emails.";
  const context = "The user prefers dark mode and daily summary notifications.";
  // hits: user, prefers, dark, mode -> 4 of 7
  assert.equal(faithfulness(expected, context), 4 / 7);
});

test("faithfulness: NaN when expectedAnswer is empty/whitespace", () => {
  assert.ok(Number.isNaN(faithfulness("", "some context")));
  assert.ok(Number.isNaN(faithfulness("   ", "some context")));
});

test("faithfulness: NaN when expectedAnswer is only stop words", () => {
  assert.ok(Number.isNaN(faithfulness("the a an is", "some context")));
});

test("faithfulness: zero-hit case", () => {
  assert.equal(faithfulness("dark mode preferences", "completely unrelated text"), 0);
});
