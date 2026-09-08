# z3rno-starter-kit

Five worked examples showing how to build with z3rno. Each is a single
readable script, runnable end-to-end against a local embedded store — no
server, no LLM API key, no Docker.

| Example | What it demonstrates | Verbs used |
|---|---|---|
| [chat_memory](src/starter_kit/chat_memory.py) | Multi-turn chat that remembers a user's preferences across runs. | `store`, `recall` |
| [customer_support](src/starter_kit/customer_support.py) | Per-customer ticket isolation via one tenant per customer, plus GDPR-style erasure. | `store`, `recall`, `forget` |
| [sql_copilot](src/starter_kit/sql_copilot.py) | Schema-aware copilot mixing long-lived (`semantic`) and short-lived (`episodic`) memory. | `store`, `recall` |
| [code_memory](src/starter_kit/code_memory.py) | Symbol-level memory over a small module, linked via `calls` edges. | `store` (with `links`), `recall` |
| [research_notebook](src/starter_kit/research_notebook.py) | Related research notes recalled by content similarity, no LLM extraction needed. | `store` (with `links`), `recall` |

## Quickstart

`z3rno` isn't published to PyPI yet, so it's built from source with `maturin` rather than installed as a normal dependency:

```sh
cd <repo root>
python3 -m venv .venv && source .venv/bin/activate

pip install maturin
maturin develop --manifest-path bindings/python/Cargo.toml

pip install -e "starter-kit[dev]"

python -m starter_kit.chat_memory
python -m starter_kit.customer_support
python -m starter_kit.sql_copilot
python -m starter_kit.code_memory
python -m starter_kit.research_notebook
```

Each example creates its own SQLite file (e.g. `chat_memory.db`) next to
wherever you run it from, and accumulates state across runs — re-running
`chat_memory.py` is a continued conversation, not a fresh start.

## Repo layout

```
starter-kit/
├── pyproject.toml
├── src/starter_kit/
│   ├── _embed.py            # naive local hashing embedding — see its docstring
│   ├── chat_memory.py
│   ├── customer_support.py
│   ├── sql_copilot.py
│   ├── code_memory.py
│   └── research_notebook.py
└── tests/
    └── test_examples_smoke.py
```

No shared utilities beyond `_embed.py` (which every example needs, since
`z3rno.Client.recall` takes a raw embedding vector, not a text query) —
each example otherwise stands alone and is meant to be read top to bottom.

## What changed from the old starter kit

The old `z3rno-starter-kit` targeted a server-backed HTTP client with a
much larger surface — recall strategies (`LEXICAL`/`AUTO`/`GRAPH`/`CODE`),
per-user metadata filtering, importance/TTL pinning, and a full
ingest→distill→refine pipeline (LLM-based entity extraction) for the code
and research-notebook examples. None of that survived the Rust rewrite:
the current engine's public surface is `store`/`recall`/`forget`/
`advanced.audit()` only, `recall` is plain vector similarity with no
server-side filtering, and there's no code-graph extractor or LLM
pipeline at all.

Every example here is redesigned around that actual surface rather than
mechanically ported:

- **customer_support** replaces the old `user_id` filter with one tenant
  per customer — real backend-enforced isolation instead of an
  application-level predicate that has to be trusted to be applied
  correctly on every query.
- **code_memory** and **research_notebook** replace automatic
  extraction with hand-described symbols/notes, linked via `store`'s
  `links` parameter. Those links are written to z3rno's graph backend
  but aren't queryable through the bindings yet — see
  [z3rno#29](https://github.com/the-ai-project-co/z3rno/issues/29) — so
  both examples fall back to content-similarity recall rather than a
  graph walk.
- **research_notebook** no longer needs `OPENAI_API_KEY` or any server
  feature flags — a genuine simplification, not just a workaround,
  matching the zero-required-infrastructure story the rest of z3rno
  tells.

## Development

```sh
pytest starter-kit          # each example actually runs, not just imports —
                             # it needs no live server, unlike the old kit
ruff check starter-kit
mypy --config-file starter-kit/pyproject.toml starter-kit/src
```

## License

Apache 2.0.
