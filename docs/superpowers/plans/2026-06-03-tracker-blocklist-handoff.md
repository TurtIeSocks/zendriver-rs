# Tracker/Fingerprinter Blocklist (#2) — Session Handoff

> **Purpose:** Pick up feature #2 in a *fresh, short session*. This is a summarized handoff, not the full task-by-task plan. Full design: `docs/superpowers/specs/2026-06-03-stealth-tostring-masking-tracker-blocklist-design.md` §4. Style reference for the eventual plan: `docs/superpowers/plans/2026-06-03-stealth-tostring-masking.md`.
>
> **First action in the new session:** read spec §4, then either run `superpowers:writing-plans` to expand this into a task-by-task plan, or implement directly (it's small enough). Start with `HostMatcher` (pure, trivially testable), then the rule variant, then core wiring, then MCP.

## What this is

Opt-in blocking of third-party fingerprinter/tracker hosts, surfaced from the obscura comparison. zendriver-rs already has the network-interception mechanism (`zendriver-interception`, `Fetch.*`) but no list/policy. Independent of feature #1 (toString masking); #1 ships first.

## Locked decisions (from brainstorm — do not relitigate)

- **List:** bundle our *own* curated fingerprinter/3rd-party-tracker list (license-clean, ~50–150 hosts) **plus** runtime BYO (url / file / inline domains).
- **Enable model:** opt-in via BrowserBuilder. Not auto-on with stealth (least-opinionated).
- **Matching:** host-only (ignore path), suffix-on-dot (subdomains of a listed domain blocked), unconditional (no first-party exemption in v1).
- **Block response:** reuse the existing `Fetch.failRequest { errorReason: "BlockedByClient" }` path — that's exactly what a real adblocker/Brave triggers (`net::ERR_BLOCKED_BY_CLIENT`), maximally plausible.
- **Curation principle (load-bearing):** include third-party *passive* fingerprinters / cross-site trackers ONLY. **EXCLUDE active anti-bot challenge providers** (DataDome, Cloudflare, PerimeterX, Imperva) — you want to *pass* those; blocking breaks site access.
- **License — why not vendor / compile-time:** `include_str!`, generate-and-commit, and `build.rs` fetch all embed the list into the shipped artifact = redistribution; the source license attaches identically. Peter Lowe's list (obscura's source) is use-restricted (no clean commercial redistribution) → must NOT be vendored. CLDR/apify are baked only because they permit it. The only model that moves the license boundary onto the user is a **runtime** fetch on the user's machine under their acceptance — which is also where real freshness lives (a library consumer pins a version + builds once). So: ship our own clean list; let users point `tracker_blocklist_url` at Peter Lowe's / uBlock / anything themselves.

## Architecture & where code goes

**`zendriver-interception` (pure mechanism — no data, no network):**
- `HostMatcher`: `HashSet<String>` + parent-domain walk on dot boundaries. `is_blocked(host)` = exact, else strip leftmost label repeatedly (`a.b.evil.com → b.evil.com → evil.com`) to bare root.
  - Suggested API: `HostMatcher::new(domains: impl IntoIterator<Item = String>) -> Self`; `fn is_blocked(&self, host: &str) -> bool`.
- New enum variant `Rule::BlockHosts { matcher: std::sync::Arc<HostMatcher> }` in `crates/zendriver-interception/src/rule.rs:31`. The actor extracts the host from `Fetch.requestPaused.request.url` and, on a match, takes the **same** failRequest(BlockedByClient) branch the existing `Rule::Block` uses. Update `Rule::matches` + the actor's dispatch + the `Debug` impl.

**`zendriver` core (opt-in API + list sourcing):**
- Bundled curated list: `include_str!("trackers.txt")` (authored by us). Parser: ignore blank lines + `#` comments; collect hostnames.
- BrowserBuilder methods (mirror existing `stealth`/`persona` builder style in `crates/zendriver/src/browser.rs:819-858`):
  - `block_trackers(bool)` — toggle the bundled list.
  - `tracker_blocklist_add(impl IntoIterator<Item = String>)` / `tracker_blocklist_file(PathBuf)` / `tracker_blocklist_url(String)` — accumulate custom sources; any source implicitly enables. Only-custom = source without `block_trackers(true)`; bundled+custom = both.
- Runtime fetch + cache for `_url`: reuse the atomic-write download-on-first-use pattern in `crates/zendriver-fingerprints/src/pool/mod.rs:76` (`load_or_download`). Implement in core under the new feature (needs `reqwest`) so interception stays network-free.
- Wiring: build one `Arc<HostMatcher>` on the browser; install a `Rule::BlockHosts` on each new tab's interception at tab creation (find the tab-creation seam in `browser.rs`).

**Feature-gating:** new `tracker-blocking` feature in `zendriver` that implies `interception`; bundled list adds binary size only when enabled. `zendriver-mcp` gets a matching feature.

## MCP coverage (REQUIRED — see CLAUDE.md)

- Extend `browser_open` (`crates/zendriver-mcp/src/tools/`) with `block_trackers: bool` + optional `tracker_blocklist` input (url | file | inline domains).
- Regenerate schema snapshots: `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked` then `cargo insta accept --all`; commit `crates/zendriver-mcp/tests/snapshots/*.snap`.
- Regenerate public-API baseline (the builder methods are new public API): `cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt`.
- Ledger-exclude the lower-level internals in `crates/zendriver-mcp/mcp-coverage-ledger.toml`: `HostMatcher` and `Rule::BlockHosts` (reached via the builder/tool, not directly).

## Tests

- `HostMatcher` unit: exact match, subdomain walk, miss, bare-root, comment/blank-line parsing.
- `Rule::BlockHosts` actor test (mock transport): matching host → failRequest path.
- Core: bundled `trackers.txt` parses; builder wires the matcher; `_url` fetch+cache against a mock server (or `#[ignore]`).
- Integration (`#[ignore]`, `integration-tests` feature, real Chrome — pattern in `crates/zendriver/tests/fingerprint_integration.rs`): a listed host → `net::ERR_BLOCKED_BY_CLIENT`; an unlisted host loads.

## Pre-push gates (CLAUDE.md)

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings   # feature-gated code touched
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked   # + cargo insta accept --all
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```

## Open items (resolve during plan/implementation)

1. **Curate `trackers.txt`** (~50–150 third-party passive fingerprinter/tracker hosts; apply the §4.4 exclusion principle — no active anti-bot vendors). This is the one piece requiring judgement/research.
2. **Tab-creation seam** in `browser.rs` where the per-tab `Rule::BlockHosts` gets installed.
3. Decide whether the core fetch+cache helper is duplicated from `pool/mod.rs` or lifted into a shared spot (duplication is acceptable for v1 — it's small).
