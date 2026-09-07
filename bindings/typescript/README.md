# bindings/typescript

TypeScript/Node bindings for z3rno, built with [napi-rs](https://napi.rs) —
native compiled bindings over `z3rno-engine`, not a thin HTTP client. The SDK
*is* the engine.

The API is **async-only**: every I/O method (`store`, `recall`, `forget`,
`advanced.audit`) returns a native `Promise`, run on napi-rs's managed Tokio
runtime. `tier()` is the one exception — no I/O, so it's a plain sync getter.

## Local development

Not yet published to npm (that's a later slice). To build and test locally
from this directory:

```bash
npm install
npm run build   # napi build --platform --release — compiles the native addon
npm test        # node test.js — store -> recall -> forget -> audit against the embedded backend
```

`npm run build` regenerates `index.d.ts` and `index.js` from the Rust
`#[napi(...)]` annotations in `src/lib.rs` — don't hand-edit those two files.

## Usage

```ts
import { Client } from "@z3rno/sdk";

// Embedded backend (default): opens/creates "z3rno.db" in the cwd.
const client = await Client.connect();

const memory = await client.store({
  tenantId: "t1",
  tier: "episodic", // "working" | "episodic" | "semantic" | "procedural"
  content: "hello",
  embedding: [0.1, 0.2, 0.3], // optional — omit if this memory shouldn't be recallable by similarity search
  metadata: { source: "chat" },
  links: [], // optional graph edges: { targetId, relationship }[]
});

const memories = await client.recall({ tenantId: "t1", query: [0.1, 0.2, 0.3], k: 5 });

const proof = await client.forget({ tenantId: "t1", id: memory.id });
// proof is `{ auditEventId, hash }`, or `null` if there was nothing to forget

client.tier(); // "embedded" or "postgres" — sync, no I/O
```

## Advanced

`client.advanced` exposes operations kept out of the top-level verb surface.
Today that's just the append-only, hash-chained audit log:

```ts
const events = await client.advanced.audit("t1");
// each event: { id, tenantId, operation, memoryId, at, prevHash, hash }
// operation is "store" | "forget"; prevHash is undefined on the first event
```

## Production backend

The embedded backend (SQLite + an in-process vector index + an in-process
graph) is the zero-infra default, with no external services required. For a
production, multi-tenant deployment, connect to the Postgres + pgvector +
Apache AGE backend instead:

```ts
const client = await Client.connect({
  backend: "postgres",
  connectionString: "postgres://user:pass@host:5432/z3rno",
});
```
