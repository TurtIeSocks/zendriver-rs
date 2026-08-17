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

CI clippy runs on **default features**; if you touched any of the
feature-gated crates listed under Workspace layout, also run the all-features
pass in its own target dir:

`CARGO_TARGET_DIR=target/all-features cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings`

A dedicated `CARGO_TARGET_DIR` keeps the default-feature and all-feature
clippy caches from invalidating each other — without it they share `target/`
and `--all-features` changing the feature set forces a full rebuild on every
feature-gated push. Costs extra disk, saves that rebuild.

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

## Documentation sync (REQUIRED before finishing a PR)

Three doc surfaces must stay in sync with the public API. Any PR that adds or
changes a user-facing capability MUST update **all three** (or consciously note
why a surface doesn't apply) before it is finished:

1. **READMEs** (`README.md` + `crates/zendriver-mcp/README.md`) — feature
   matrix, install examples, the MCP tool count, and the "what agents get"
   bullets. The tool count appears verbatim in several places — grep for the
   old number and replace every hit.
2. **Rustdocs** — doc comments on every new/changed public item (`BrowserBuilder`
   option, `Tab`/`Frame`/`Element` method, new type, feature flag). These render
   on docs.rs, so keep examples `no_run`-compilable.
3. **The mdBook** (`docs/book/src/`) — add or extend the relevant chapter
   (a new `BrowserBuilder` option → its feature's chapter; a new MCP tool →
   `mcp.md`, including the tool count + category table). Confirm it still builds:
   `mdbook build docs/book`.

The published MCP tool count = tools compiled with the default features
(`cargo install zendriver-mcp`); `--all-features` adds the opt-in
`fingerprints` / `geo` tools. Treat a shipped behavior change with a stale
README / rustdoc / book as an incomplete PR — same bar as the MCP coverage
check above.

## PR scope

Default to one PR for related fixes rather than splitting on review-hygiene
grounds alone. Caught 2026-08-06 on #161: the macOS symlink fix went onto its
own branch off `main` because the PR body argued the work "deserves its own
review rather than a footnote in a feature PR." That reasoning ignored the
deciding fact: the CLI that reproduces the bug exists only on the PR branch,
so a fix on `main` was unreachable. Rin's reasons for keeping it one PR: the
diff is small next to the PR, the two are genuinely related, and this repo
carries heavy CI ceremony while she was already carrying PRs from another
session.

## Workspace layout

9-crate workspace (`edition = 2024`, MSRV 1.85) — each crate's Cargo.toml
`description` states its role.

Capability crates are wired into `zendriver` behind features (`interception` /
`cloudflare` / `imperva` / `datadome` / `fetcher` / `expect` / `monitor` /
`geo` / `tracker-blocking`), which `zendriver-mcp` re-exposes behind matching
MCP features (plus `fingerprints`).
