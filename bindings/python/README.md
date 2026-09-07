# bindings/python

Native Python bindings for `z3rno-engine`, built with [PyO3](https://pyo3.rs) and [maturin](https://www.maturin.rs). The SDK *is* the engine — `Client` calls straight into the compiled Rust core, not an HTTP client.

The bindings are sync-only: every engine call runs on one shared Tokio runtime under the hood, so there's no `asyncio` to set up. `python -c "import z3rno; ..."` is the whole story.

## Install

Not yet published to PyPI. For local development:

```sh
pip install maturin
maturin develop -m bindings/python/Cargo.toml
```

That builds the extension and installs it editable into your active virtualenv.

## Usage

```python
import z3rno

# Embedded default — opens/creates "z3rno.db" in the cwd, no external services.
client = z3rno.Client()

memory = client.store(
    tenant_id="t1",
    tier="episodic",  # "working" | "episodic" | "semantic" | "procedural"
    content="hello",
    embedding=[0.1, 0.2],   # optional — omit if this memory shouldn't be vector-searchable
    metadata={"k": "v"},    # optional, defaults to {}
    links=[],                # optional [(target_id, relationship), ...] graph edges
)
# memory is a dict: id/tenant_id/tier/content/metadata/created_at

memories = client.recall(tenant_id="t1", query=[0.1, 0.2], k=5)

proof = client.forget(tenant_id="t1", id=memory["id"])
# None if already gone, else a dict with audit_event_id/hash

print(client.tier())  # "embedded" or "postgres"
```

### Advanced

`client.advanced.audit(...)` queries the tenant's full append-only, hash-chained audit log (oldest first) — the one operation kept off the top-level `store`/`recall`/`forget` surface:

```python
events = client.advanced.audit(tenant_id="t1")
# list of dicts: id/tenant_id/operation/memory_id/at/prev_hash/hash
```

### Production backend

Pass `backend="postgres"` with a `connection_string` to run against the production Postgres + pgvector + Apache AGE backend instead of the embedded default:

```python
client = z3rno.Client(
    backend="postgres",
    connection_string="postgres://user:pass@host/db",
)
```

### Errors

A `store`/`forget` call against a nonexistent record raises `LookupError`; a storage-layer failure raises `RuntimeError`; an invalid `tier`, `backend`, or memory id string raises `ValueError` — all with the original error detail from the engine.
