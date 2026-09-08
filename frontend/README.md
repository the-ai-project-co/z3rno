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

Start the server with a known `--jwt-secret`, then mint a token with
`z3rno token` (previously there was no self-service way to do this — see
[z3rno#35](https://github.com/the-ai-project-co/z3rno/issues/35), now
fixed):

```
z3rno serve --jwt-secret dev-secret --sqlite-path z3rno.db
z3rno token --tenant local --role admin --jwt-secret dev-secret
```

`--tenant` is the tenant the token can see — use the same tenant you're
storing memories against (`z3rno store --tenant local ...`, or the HTTP
API's default). Paste the printed token into the frontend's "Auth token"
field.
