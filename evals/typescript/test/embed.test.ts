import { test } from "node:test";
import assert from "node:assert/strict";

import { DIM, hashEmbed } from "../src/embed";

function assertClose(actual: number, expected: number, tol = 1e-4, msg?: string) {
  assert.ok(Math.abs(actual - expected) < tol, msg ?? `expected ${actual} to be close to ${expected}`);
}

/** Asserts every nonzero bucket matches `expected` (index -> value) and every other bucket is ~0. */
function assertVector(v: number[], expected: Record<number, number>) {
  assert.equal(v.length, DIM);
  for (let i = 0; i < DIM; i++) {
    assertClose(v[i], expected[i] ?? 0, 1e-4, `bucket ${i}: got ${v[i]}, expected ${expected[i] ?? 0}`);
  }
}

test("hashEmbed('hello world') matches the known-good reference vector", () => {
  const v = hashEmbed("hello world");
  assertVector(v, { 11: 0.7071068, 115: 0.7071068 });
});

test("hashEmbed(pangram) matches the known-good reference vector", () => {
  const v = hashEmbed("the quick brown fox jumps over the lazy dog");
  assertVector(v, {
    14: 0.3015113,
    28: 0.3015113,
    47: -0.3015113,
    79: 0.3015113,
    90: 0.3015113,
    105: 0.3015113,
    111: 0.3015113,
    124: -0.6030227,
  });
});

test("hashEmbed('') is the all-zero vector", () => {
  const v = hashEmbed("");
  assert.equal(v.length, DIM);
  assert.ok(v.every((x) => x === 0));
});

test("hashEmbed is deterministic", () => {
  assert.deepEqual(hashEmbed("hello world"), hashEmbed("hello world"));
});

test("hashEmbed is case-insensitive", () => {
  assert.deepEqual(hashEmbed("Hello World"), hashEmbed("hello world"));
});

test("hashEmbed is order-invariant (bag of words)", () => {
  assert.deepEqual(hashEmbed("brown fox quick"), hashEmbed("quick fox brown"));
});

test("hashEmbed output is L2-normalized", () => {
  const v = hashEmbed("the quick brown fox jumps over the lazy dog");
  const norm = Math.sqrt(v.reduce((s, x) => s + x * x, 0));
  assertClose(norm, 1.0, 1e-5);
});
