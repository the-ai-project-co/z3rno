const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { Client } = require("./index.js");

async function main() {
  const dbPath = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "z3rno-ts-test-")), "z3rno.db");

  const client = await Client.connect({ backend: "embedded", path: dbPath });
  assert.strictEqual(client.tier(), "embedded");

  const stored = await client.store({
    tenantId: "t1",
    tier: "episodic",
    content: "hello",
    embedding: [0.1, 0.2, 0.3],
    metadata: { k: "v" },
    links: [],
  });
  assert.strictEqual(stored.tenantId, "t1");
  assert.strictEqual(stored.tier, "episodic");
  assert.strictEqual(stored.content, "hello");
  assert.deepStrictEqual(stored.metadata, { k: "v" });
  assert.ok(typeof stored.id === "string" && stored.id.length > 0);
  assert.ok(typeof stored.createdAt === "string" && stored.createdAt.length > 0);

  const recalled = await client.recall({ tenantId: "t1", query: [0.1, 0.2, 0.3], k: 5 });
  assert.strictEqual(recalled.length, 1);
  assert.strictEqual(recalled[0].id, stored.id);

  // Recalling for a different tenant must not see t1's memory.
  const otherTenant = await client.recall({ tenantId: "t2", query: [0.1, 0.2, 0.3], k: 5 });
  assert.strictEqual(otherTenant.length, 0);

  // Invalid tier is rejected with a clear error, not silently accepted.
  await assert.rejects(
    () =>
      client.store({
        tenantId: "t1",
        tier: "not-a-tier",
        content: "bad",
        metadata: {},
      }),
    /invalid tier/,
  );

  const events = await client.advanced.audit("t1");
  assert.strictEqual(events.length, 1);
  assert.strictEqual(events[0].operation, "store");
  assert.strictEqual(events[0].memoryId, stored.id);
  assert.strictEqual(events[0].prevHash, undefined);

  const proof = await client.forget({ tenantId: "t1", id: stored.id });
  assert.ok(proof);
  assert.ok(typeof proof.auditEventId === "string" && proof.auditEventId.length > 0);
  assert.ok(typeof proof.hash === "string" && proof.hash.length > 0);

  // Forgetting again is a no-op: null, no new audit event.
  const secondForget = await client.forget({ tenantId: "t1", id: stored.id });
  assert.strictEqual(secondForget, null);

  const eventsAfterForget = await client.advanced.audit("t1");
  assert.strictEqual(eventsAfterForget.length, 2);
  assert.strictEqual(eventsAfterForget[1].operation, "forget");
  assert.strictEqual(eventsAfterForget[1].hash, proof.hash);
  assert.strictEqual(eventsAfterForget[1].prevHash, eventsAfterForget[0].hash);

  const afterForget = await client.recall({ tenantId: "t1", query: [0.1, 0.2, 0.3], k: 5 });
  assert.strictEqual(afterForget.length, 0);

  console.log("z3rno TypeScript bindings: store -> recall -> forget -> audit OK");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
