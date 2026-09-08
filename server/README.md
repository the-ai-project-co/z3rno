# z3rno-server

The real Axum HTTP API for z3rno — `store`/`recall`/`forget`/`audit` over
a `MemoryEngine` (embedded or Postgres+pgvector+AGE), plus sessions, admin
budget overrides, and observability. Not a scaffold — this is the same
server shipped in the `ghcr.io/the-ai-project-co/z3rno/server` image.

## Running it

Easiest local path is via the CLI, which wraps this crate as a library and
generates a throwaway JWT secret for you:

```
z3rno serve --superadmin-api-key <any-string-you-pick>
```

Or run the standalone binary/image directly — `Z3RNO_JWT_SECRET` is
required here, with no insecure default:

```
docker run -e Z3RNO_JWT_SECRET=<a-real-random-secret> \
  -p 8080:8080 ghcr.io/the-ai-project-co/z3rno/server
```

See `cli/README.md`'s `z3rno serve` section for the full flag/env-var list
(listen address, Postgres vs. embedded backend, cache path, superadmin key).

## Auth

Every route except `/v1/limits` and `/v1/health*` requires an
`Authorization: Bearer <token>` header — a JWT signed with
`Z3RNO_JWT_SECRET`, or a pre-registered API key (looked up via the cache
backend as `tenant_id|role`). Roles gate individual routes (e.g. `recall`,
`GET /v1/memories`, and `GET /v1/memories/{id}/neighbors` need
`Admin`/`Write`/`Read`; `store`/`forget` need `Admin`/`Write`).

**`--superadmin-api-key`/`Z3RNO_SUPERADMIN_API_KEY` is narrower than it
sounds**: it grants `Role::Superadmin`, which only the admin budget-override
routes accept — it does *not* satisfy the `Admin`/`Write`/`Read` roles the
memory/session/audit routes check, and it carries no tenant (superadmin acts
across tenants, not within one). There is also no route or CLI command that
issues a JWT or registers an API key — for local dev against `/v1/memories*`
today, hand-craft an HS256 JWT (`{"sub": "...", "org_id": "<tenant>",
"role": "admin", "exp": <unix-ts>}`, signed with whatever you passed as
`--jwt-secret`) with any JWT library. This is a real, undocumented-until-now
gap, filed as [z3rno#35](https://github.com/the-ai-project-co/z3rno/issues/35).

## Routes

- `POST /v1/memories`, `GET /v1/memories`, `POST /v1/memories/recall`,
  `POST /v1/memories/forget`, `GET /v1/memories/{id}/neighbors` — the
  memory-engine surface, including the two list/neighbor routes the
  in-monorepo `frontend/` graph visualizer (slice 0010) is built against.
- `POST /v1/sessions`, `GET /v1/sessions/{id}`, `POST /v1/sessions/{id}/end`
- `GET /v1/audit`
- Admin budget-override routes (superadmin only)
- `GET /v1/health`, `GET /v1/health/detailed`, `GET /metrics`

Full per-channel install/config docs are being consolidated onto the new
website's `/docs` section as part of this same slice — this README stays
the source of truth for anything that changes here before that lands.
