# Contributing to z3rno

Thanks for your interest in contributing. This project is in an active ground-up rewrite, so expect the codebase to move quickly — please open an issue before starting substantial work, to avoid duplicate effort.

## Developer Certificate of Origin

Every commit must be signed off, certifying you have the right to submit the contribution under this project's license:

```bash
git commit -s -m "your commit message"
```

See [DCO.md](DCO.md) for the full text you're agreeing to.

## Workflow

1. Fork the repo (or branch directly if you have write access) and create a feature branch.
2. Make your change. Keep pull requests scoped to one unit of work.
3. Ensure all CI checks pass — lint, format, and tests, across every crate/binding touched.
4. Open a pull request against `main` using the PR template. Link the issue it addresses.
5. Once CI is green, the PR merges automatically (auto-merge is enabled on this repo).

## Development setup

This is a Cargo workspace with native language bindings. See each crate's own `README.md` under `crates/` for its specific build steps once the workspace scaffold lands. Broad expectations:

- Rust: latest stable toolchain, `cargo fmt` and `cargo clippy -- -D warnings` clean before opening a PR.
- Python bindings: built via [maturin](https://www.maturin.rs/) (PyO3).
- TypeScript bindings: built via [napi-rs](https://napi.rs/).

## Commit messages

Conventional commits (`feat:`, `fix:`, `docs:`, `chore:`, `ci:`, etc.) are preferred but not currently enforced by CI.

## Code of Conduct

Participation in this project is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Reporting bugs / requesting features

Use the issue templates — Bug Report, Feature Request, or Documentation — when opening an issue.
