# zendriver-rs — project instructions

## Before every push (REQUIRED)

CI fails the PR on formatting or lint regressions, so run these locally and fix
**before** pushing — never push and rely on CI to catch them:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
```

Then confirm both gates pass exactly as CI runs them:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

- `cargo fmt --all` first — the most common CI failure is unformatted code.
- `clippy --fix` auto-applies machine-applicable lints; review the diff, then
  hand-fix anything `--fix` couldn't (clippy CI uses `-D warnings`, so any
  remaining warning is a hard failure).
- Re-stage / amend after the fixes so the pushed commit is already clean.

CI clippy runs on **default features**; if you touched feature-gated code
(`interception` / `expect` / `cloudflare` / `imperva` / `fetcher`), also run
`cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings`.

## Schema snapshots (zendriver-mcp)

After changing any MCP tool input/output type, regenerate + accept the `insta`
JSON-schema snapshots and ensure none stay pending:

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```

Commit the updated `crates/zendriver-mcp/tests/snapshots/*.snap` — the wire
shape is reviewed.
