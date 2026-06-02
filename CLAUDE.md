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

## MCP coverage (REQUIRED before finishing a PR)

`zendriver-mcp` must track the `zendriver` surface as closely as practical:
every user-facing capability should be reachable through an MCP tool. So
**any PR that adds or changes a public API MUST be validated against
`zendriver-mcp`** before it is finished — add/extend the corresponding tool,
or consciously record why the API is out of scope.

For each new or changed public item in a PR (a `BrowserBuilder` option, a
`Tab`/`Frame`/`Element` method, a new type or feature):

- Ask: is it reachable via a tool under `crates/zendriver-mcp/src/tools/`? If
  it should be and isn't, add/extend the tool (then run the schema-snapshot
  step above for the I/O change).
- If it is **deliberately not exposed**, say so in the PR description with the
  reason. Legitimate non-goals: APIs that don't fit a request/response tool
  (e.g. a `Stream`-returning subscription like `tab.monitor()`), internal
  `pub(crate)` items, or purely-Rust ergonomics with no agent-facing value.

Treat a public API with no MCP tool and no recorded reason as a coverage gap
to close. (A CI check enforcing this is tracked separately.)
