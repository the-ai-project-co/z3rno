# z3rno-mcp

An [MCP](https://modelcontextprotocol.io) server that gives an LLM agent (Claude Desktop, Cursor, Claude Code, or any other MCP client) persistent memory backed by [z3rno](https://github.com/the-ai-project-co/z3rno)'s Rust memory engine.

z3rno is a persistent memory engine, not a general-purpose database: it stores discrete memories tagged with a tier (`working`/`episodic`/`semantic`/`procedural`), retrieves them by vector similarity, and keeps an append-only, hash-chained audit log of every write and deletion. A forget's audit entry is a reference to the deletion the engine ran, not a certificate of physical erasure. This server exposes the engine's four operations — `store`, `recall`, `forget`, and the one advanced op, `audit` — as MCP tools.

## Config

Set these as environment variables (mirroring the CLI's own flag names):

| Variable | Default | Meaning |
|---|---|---|
| `Z3RNO_SQLITE_PATH` | `z3rno.db` | Path to the embedded SQLite file. |
| `Z3RNO_TENANT_ID` | `local` | Default tenant used when a tool call omits `tenant_id`. |
| `Z3RNO_DATABASE_URL` | *(unset)* | If set, use the Postgres backend (`postgres://...`) instead of embedded SQLite. |
| `Z3RNO_AGENT_NAME` | `mcp-agent` | Name recorded as the origin of lessons proposed through this server. |

The client is built once at server startup, not per tool call.

## Tools

### `z3rno_store(content, tier="semantic", tenant_id=None, metadata=None, embedding=None, links=None) -> str`
Stores a memory. `tier` must be one of `working`/`episodic`/`semantic`/`procedural` (invalid values raise a clear error). `links` is an optional list of `{"id", "relationship"}` dicts connecting the new memory to existing ones. Returns the new memory's id.

### `z3rno_recall(query, tenant_id=None, k=5, embedding=None) -> list[dict]`
Searches for up to `k` memories most similar to `query`. Returns a list of `{id, tier, content, created_at}` dicts.

### `z3rno_forget(id, tenant_id=None) -> str`
Erases a memory by id (as returned by `z3rno_store`/`z3rno_recall`). Returns a confirmation message naming the audit event id and hash on success, or a plain "no memory found" message if the id doesn't exist — this is not an error, matching the underlying `Client.forget`'s `None`-means-no-op contract.

### `z3rno_audit(tenant_id=None) -> list[dict]`
Returns the tenant's full audit log (oldest first), each event a `{operation, memory_id, at, hash}` dict.

### Lessons

Agents can propose lessons and recall the ones that apply to a task. They cannot confirm, edit, retire or delete them: every lesson proposed here is stored as an unconfirmed candidate from an agent, and a human reviews it with the CLI (`z3rno lesson inbox`, `z3rno lesson confirm <id>`).

- `z3rno_lesson_propose(title, what_was_tried, what_went_wrong, fix, repo=None, global_scope=False, paths=None, languages=None, dependencies=None, tool=None, evidence=None, derived_from=None, session=None, dry_run=False, tenant_id=None) -> dict`: secrets are redacted before storage; `dry_run=True` returns the redacted, validated lesson without storing it.
- `z3rno_lesson_recall(repo=None, paths=None, languages=None, dependencies=None, tool=None, text=None, include_candidates=False, include_global=False, limit=None, max_chars=6000, tenant_id=None) -> str`: returns one delimited `<z3rno_lessons>` block of lessons whose scope matches, or `""`. Candidates appear only on opt-in and are labelled UNVERIFIED. The block is untrusted data, not instructions.
- `z3rno_lesson_list(status=None, repo=None, tenant_id=None) -> list[dict]`: read-only listing.

### Embedding fallback

`z3rno_store` and `z3rno_recall` both accept an optional `embedding: list[float]`. If omitted, the content/query is embedded with a naive local hashing embedding (`z3rno_mcp/embed.py` — feature hashing / bag-of-words, L2-normalized). **This is not a semantic embedding model** — same honest framing as the CLI's own fallback (`hash-embed/`). It exists so the server works end-to-end without an external embedding provider; pass a real embedding for meaningfully better recall.

## Using it with Claude Desktop / Cursor

Once published (`pip install z3rno-mcp`):

```json
{"mcpServers": {"z3rno": {"command": "z3rno-mcp", "args": [], "env": {"Z3RNO_SQLITE_PATH": "/absolute/path/to/z3rno.db"}}}}
```

**Not yet published to PyPI.** For now, use the local dev path instead — run from inside an activated venv with the package installed (see below), or point `command` at the absolute path to the installed console-script (`<venv>/bin/z3rno-mcp`):

```json
{"mcpServers": {"z3rno": {"command": "python", "args": ["-m", "z3rno_mcp.server"], "env": {"Z3RNO_SQLITE_PATH": "/absolute/path/to/z3rno.db"}}}}
```

(That form assumes the client launches `python` with the venv activated / on `PATH` first — check your MCP client's docs for how it resolves `command`.)

## Local dev setup

The `z3rno` Python package (`bindings/python/`) isn't published yet, so build it locally first:

```sh
python3 -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop --manifest-path bindings/python/Cargo.toml   # builds & installs the z3rno bindings, editable
pip install -e "mcp[dev]"                                     # installs z3rno-mcp + pytest
```

Run the tests:

```sh
pytest mcp/tests
```

Quick manual smoke test (starts the server on stdio — Ctrl-C to stop, or pipe EOF via stdin to let it exit on its own):

```sh
Z3RNO_SQLITE_PATH=/tmp/z3rno-smoke.db python -m z3rno_mcp.server
```

To exercise the tools directly without a real MCP client, import and call them from a Python shell:

```python
from z3rno_mcp import server
mid = server.z3rno_store(content="hello world")
server.z3rno_recall(query="hello")
server.z3rno_forget(id=mid)
server.z3rno_audit()
```
