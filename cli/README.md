# z3rno-cli

The standalone `z3rno` CLI binary (crate name `z3rno-cli`, binary name
`z3rno`), distributed via crates.io and npm. Four subcommands: `init`,
`serve`, `store`, `recall` — a real, useful tool against a local embedded
store on its own, not just a wrapper around the HTTP API.

```
cargo run --bin z3rno -- <init|serve|store|recall> ...
# or, once installed: z3rno <init|serve|store|recall> ...
```

Every command accepts `--tenant`; when omitted it defaults to `"local"`
everywhere. `init`/`store`/`recall` all default `--path` to `z3rno.db` in
the current directory, so running them back to back with no flags composes
into one local store:

```
z3rno init
z3rno store "the user prefers dark mode"
z3rno recall "dark mode"
```

## `z3rno init`

Bootstraps a local embedded store: opens `MemoryEngine::embedded(--path)`
once, which creates the SQLite file and schema if they don't exist yet, and
prints a confirmation.

```
z3rno init [--path z3rno.db] [--tenant local]
```

## `z3rno serve`

Runs the real z3rno HTTP API server (the `z3rno-server` crate, used here as
a library — see its own `run`/`AppState`) locally, against either the
embedded backend or Postgres.

```
z3rno serve
  [--listen-addr 0.0.0.0:8080]
  [--database-url postgres://...]      # omit to use --sqlite-path instead
  [--sqlite-path z3rno.db]
  [--cache-sqlite-path z3rno-cache.db]
  [--jwt-secret <secret>]
  [--superadmin-api-key <key>]
```

Every flag also reads from the matching `Z3RNO_*` environment variable
(`Z3RNO_LISTEN_ADDR`, `Z3RNO_DATABASE_URL`, `Z3RNO_SQLITE_PATH`,
`Z3RNO_CACHE_SQLITE_PATH`, `Z3RNO_JWT_SECRET`, `Z3RNO_SUPERADMIN_API_KEY`).

**`--jwt-secret` is optional here**, unlike the standalone `z3rno-server`
binary (which refuses to start without one). Omit it and `z3rno serve`
generates a random secret for that run and prints a warning to stderr. This
is a local/dev convenience, not a production path — every session and
token issued that run stops validating the moment the process restarts.
Pass `--jwt-secret` (or set `Z3RNO_JWT_SECRET`) for anything that needs to
survive a restart.

## `z3rno store`

Stores a memory against a local embedded store.

```
z3rno store <content>
  [--tenant local]
  [--path z3rno.db]
  [--tier semantic]      # working | episodic | semantic | procedural
  [--embedding 0.1,0.2,...]
  [--metadata '{"key":"value"}']
```

Prints the created memory's id on success. See **Embeddings** below for
what happens when `--embedding` is omitted.

## `z3rno recall`

Recalls up to `--k` memories most similar to `<query>`.

```
z3rno recall <query>
  [--tenant local]
  [--path z3rno.db]
  [--k 5]
  [--embedding 0.1,0.2,...]
```

Prints one line per result: id, tier, and the (possibly truncated)
content. Prints `No memories found.` if nothing matches.

## Embeddings: the naive local hashing default

`store` and `recall` both need a vector to do anything useful, and z3rno
doesn't require an API key or an embedding service to be minimally useful.
When `--embedding` is omitted, both commands embed the text themselves with
a small **feature-hashing** ("hashing trick") function in `src/embed.rs`:
lowercase + whitespace-tokenize, hash each token into one of 128 buckets,
accumulate +1/-1 per occurrence, L2-normalize. It's a deterministic
bag-of-words vector — text sharing more words scores higher — nothing
more.

**This is not a semantic embedding model.** It doesn't know synonyms, word
order, or meaning; it's a local placeholder that makes `store`/`recall`
work end to end with zero external services. For real semantic recall,
supply real embeddings (from an LLM provider) via `--embedding`, or use the
SDKs/server API, which accept a plain `Vec<f32>` and don't care how it was
produced.

`store` and `recall` must use embeddings from the *same* space to find
each other — the naive hashing function is deterministic and used
identically by both, so the default composes correctly on its own; mixing
naive-hashing and real-model embeddings for the same tenant will not.

## A known limitation: recall across separate CLI invocations

`z3rno-engine`'s embedded vector index (`EmbeddedVectorBackend`) lives
entirely in process memory — it is not persisted to the SQLite file, only
the relational memory record is. Tracked as
[z3rno#21](https://github.com/the-ai-project-co/z3rno/issues/21); the same
class of gap already tracked for the embedded graph backend as
[z3rno#8](https://github.com/the-ai-project-co/z3rno/issues/8).

Practically: each `z3rno store` / `z3rno recall` invocation opens a fresh
`MemoryEngine::embedded(path)`, so a `store` in one process and a `recall`
in a later, separate process will not find that memory via similarity
search yet, even though `z3rno recall` runs correctly against whatever was
stored earlier **in the same process** — e.g. through one long-running
`z3rno serve` session, or the SDK bindings holding a single engine
instance. Fixing this requires the embedded vector backend to persist and
reload its index, which is engine-crate work outside this CLI sub-slice's
scope; flagging it here rather than leaving it a silent surprise.
