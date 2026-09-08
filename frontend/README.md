# frontend

An in-monorepo Next.js app that visualizes a z3rno memory graph — the
slice 0010 successor to the old archived stack's `/graph` page. Talks
directly to a running `z3rno-server`; local dev tool only for now, no
static export, no deploy pipeline.

## What it does

- Lists every memory in a tenant as graph nodes (`GET /v1/memories` — there's
  no full-graph endpoint, so this is the node seed).
- Clicking a node fetches its one-hop neighbors (`GET /v1/memories/{id}/neighbors`)
  and adds any newly-discovered nodes/edges to the view. This is
  progressive one-hop expansion, not a full-graph render — z3rno has no
  multi-hop graph traversal at any layer today (see z3rno#29).
- Clicking a node also shows its content/tier/metadata in the side panel.

## Running it

```
npm install
npm run dev
```

Then, in the settings panel, set the server URL (`http://localhost:8080`
by default) and an auth token, and click **Save & load**.

### Getting a usable token

Neither `--superadmin-api-key` nor any CLI command currently issues a
working token for the memory routes (`GET /v1/memories`, `.../neighbors`,
`recall`, etc. all need `Admin`/`Write`/`Read`, which `Superadmin` doesn't
satisfy — see `server/README.md`'s Auth section, and
[z3rno#35](https://github.com/the-ai-project-co/z3rno/issues/35)). For local
dev, start the server with a known `--jwt-secret` and hand-craft an HS256
JWT signed with it:

```
z3rno serve --jwt-secret dev-secret --sqlite-path z3rno.db
```

```python
import hmac, hashlib, base64, json, time

def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()

header = {"alg": "HS256", "typ": "JWT"}
claims = {"sub": "dev", "org_id": "local", "role": "admin", "exp": int(time.time()) + 3600}
signing_input = (
    b64url(json.dumps(header, separators=(",", ":")).encode())
    + "."
    + b64url(json.dumps(claims, separators=(",", ":")).encode())
)
sig = hmac.new(b"dev-secret", signing_input.encode(), hashlib.sha256).digest()
print(signing_input + "." + b64url(sig))
```

`org_id` is the tenant the token can see — use the same tenant you're
storing memories against (`z3rno store --tenant local ...`, or the HTTP
API's default). Paste the printed token into the frontend's "Auth token"
field.

This whole flow (server + hand-signed JWT + `curl`) was used to verify
these routes end-to-end while building this app — see PR history for the
exact commands.
