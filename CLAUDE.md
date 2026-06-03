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
(`interception` / `expect` / `monitor` / `cloudflare` / `imperva` / `fetcher` /
`fingerprints`), also run
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
- If it is **deliberately not exposed**, record it in
  `crates/zendriver-mcp/mcp-coverage-ledger.toml` with an `excluded = "<reason>"`
  entry (otherwise add `covered = "<tool-name>"`). Legitimate non-goals:
  APIs that don't fit a request/response tool
  (e.g. a `Stream`-returning subscription like `tab.monitor()`), internal
  `pub(crate)` items, or purely-Rust ergonomics with no agent-facing value.

Treat a public API with no MCP tool and no ledger entry as a coverage gap to
close. The `mcp-coverage` CI job (`.github/workflows/mcp-coverage.yml`) enforces
this: `tests/public_api.rs` diffs the current `zendriver` public API against
`public-api-baseline.txt` and fails if any new item is missing from the ledger.
Run it locally (needs nightly + `cargo-public-api` v0.52.0):

```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```

If you intentionally changed the public API, regenerate the baseline:

```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
```

## Workspace layout

9-crate workspace (`edition = 2024`, MSRV 1.85). Roles:

| Crate | Role |
|-------|------|
| `zendriver` | Core: async browser automation over the Chrome DevTools Protocol. The public API everything extends. |
| `zendriver-transport` | Internal WebSocket + CDP routing actor (plumbing). |
| `zendriver-stealth` | Anti-detection patches + personas. |
| `zendriver-fingerprints` | Real-device persona sources (pool + generative). |
| `zendriver-interception` | Network interception via the `Fetch.*` CDP domain. |
| `zendriver-cloudflare` | Cloudflare Turnstile bypass. |
| `zendriver-imperva` | Imperva WAF / Incapsula bypass. |
| `zendriver-fetcher` | Chrome binary downloader. |
| `zendriver-mcp` | MCP server exposing the `zendriver` surface as agent tools (see MCP coverage above). |

Capability crates are wired into `zendriver` behind features (`interception` /
`cloudflare` / `imperva` / `fetcher` / `expect` / `monitor`), which
`zendriver-mcp` re-exposes behind matching MCP features.
