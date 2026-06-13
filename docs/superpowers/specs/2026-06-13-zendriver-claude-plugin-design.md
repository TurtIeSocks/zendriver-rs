# Design: `zendriver` Claude Code plugin

- **Date:** 2026-06-13
- **Status:** Approved (brainstorm complete) — pending implementation plan
- **Topic:** A Claude Code plugin that makes `zendriver-rs` easy to set up and effective to use with Claude.

## Goal & context

`zendriver-mcp` already exposes ~80 stealth-browser tools over MCP, but a user
must find the binary, wire the server, and then still know *how* to drive 80
tools well. This plugin closes both gaps: **easy setup** (one marketplace add +
install + a guided `/setup`) and **effective use** (skills that teach Claude the
scraping strategy, a flagship command, and a dedicated subagent).

Headline use case: **stealth scraping / extraction** — pulling content or
structured data from JS-rendered or bot-walled sites. Anti-bot bypass,
interactive automation, and capture/archive are *supporting* flows (e.g. bypass
is a sub-step when a wall blocks a scrape), not co-equal headliners.

## Architecture decision

**Chosen: Approach A — skill as single source of truth.** One canonical
scrape-playbook skill holds the flow and gotchas; the subagent's prompt says
"follow the `zendriver:scraping` skill"; commands are thin entry points. This is
DRY — one place to edit when zendriver changes — and avoids the bypass/scrape
sequence drifting across three copies.

Rejected:
- **B — self-contained components:** each command/subagent embeds its own flow;
  simpler per file, drifts badly over time.
- **C — maximal automation:** hooks auto-open the browser / auto-scrape on URL
  paste; most "magic," most brittle, fires when unwanted.

## Component overview

| Component | What | Where |
|-----------|------|-------|
| MCP server | bundled, command → `${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp` | `plugins/zendriver/.mcp.json` |
| Skills | `scraping` (always-on, canonical), `bypass`, `advanced` | `plugins/zendriver/skills/*/SKILL.md` |
| Commands | `/zendriver:setup`, `/zendriver:scrape`, `/zendriver:extract` | `plugins/zendriver/commands/*.md` |
| Subagent | `zendriver-scraper` | `plugins/zendriver/agents/zendriver-scraper.md` |
| Hook | SessionStart setup-nudge (1 hook) | `plugins/zendriver/hooks/hooks.json` |
| Provisioner | `setup.sh` (prebuilt \| source \| link) | `plugins/zendriver/scripts/setup.sh` |
| Release CI | cross-compile + attach to GH release | `.github/workflows/release-binaries.yml` |

## Section 1 — Repo layout, distribution & binary delivery

### Repo layout (all new, in the existing `zendriver-rs` repo)

```
zendriver-rs/
├── .claude-plugin/
│   └── marketplace.json          # marketplace, name = "zendriver-rs"
├── plugins/zendriver/            # the plugin, name = "zendriver"
│   ├── .claude-plugin/plugin.json
│   ├── .mcp.json                 # command → ${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp
│   ├── scripts/setup.sh          # prebuilt | source | link provisioner
│   ├── hooks/hooks.json
│   ├── skills/
│   │   ├── scraping/SKILL.md
│   │   ├── bypass/SKILL.md
│   │   └── advanced/SKILL.md
│   ├── commands/
│   │   ├── setup.md
│   │   ├── scrape.md
│   │   └── extract.md
│   ├── agents/zendriver-scraper.md
│   └── README.md
└── .github/workflows/release-binaries.yml
```

### Install (user side) — zero Rust on the prebuilt path

```
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
# first session: /zendriver:setup  → pick delivery → restart session
```

`marketplace.json` lists the one plugin via a relative-path source pointing at
`plugins/zendriver`.

### Binary delivery — `scripts/setup.sh`, 3 modes

`.mcp.json` points at a **stable path** (`${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp`),
so delivery is decoupled from config — whatever lands a binary there wins:

- `--mode prebuilt` (default): `uname -sm` → target triple → download the
  matching asset from the latest `zendriver-mcp-v*` GitHub Release → verify
  against published `SHA256SUMS` → install to the stable path, `chmod +x`.
- `--mode source` (trust escape hatch; needs Rust):
  `cargo install zendriver-mcp --root ${CLAUDE_PLUGIN_DATA}` — lands at the same
  path. For devs who would rather compile the public source than trust a
  prebuilt binary from a relatively unknown repo.
- `--mode link`: symlink an existing `zendriver-mcp` from PATH.

`/zendriver:setup` detects platform + tool availability, presents the 3 modes
(prebuilt recommended), runs the chosen one, then tells the user to restart the
session so the MCP server picks up the binary.

### Release workflow

`release-binaries.yml` triggers on the `zendriver-mcp-v*` tag that **release-plz
already creates** (so no parallel release process). It cross-compiles macOS
(arm64 + x64), Linux (x64 + arm64, gnu), and Windows (x64); uploads the binaries
plus `SHA256SUMS` and a build-provenance attestation to that same release.

### Bonus: no system Chrome required

The `fetcher` feature is in `zendriver-mcp`'s default features, so the binary
auto-downloads its own Chrome on first `browser_open` (or via
`browser_install_chrome`). Combined with the prebuilt binary path, a user needs
neither Rust nor a system Chrome.

## Section 2 — Skills layer (the intelligence)

The plugin namespace already prefixes `zendriver:`, so skill names omit the
redundant prefix. All three auto-trigger by `description`; only the relevant one
loads (progressive disclosure).

### `scraping` — canonical playbook (single source of truth)

- **Trigger:** "Use when scraping, extracting, or reading content/data from a
  website — especially JS-rendered SPAs, bot-walled, or pages plain fetch can't
  reach. Covers the goto→wait→extract flow, surgical extraction, and zendriver
  tool selection."
- **Body:**
  - **Core flow:** `browser_open` → `browser_goto {url, wait_for:"load"}` →
    extract. Stealth is default-on — do not re-enable it.
  - **The effectiveness lever — surgical extraction, not DOM-dumping:**
    `browser_html {trim:true}` is ~40–50k tokens. When the goal is specific (a
    table, prices, links), use `browser_find` / `browser_find_all`,
    `browser_get_links`, or `browser_evaluate` to pull just that; reserve full
    DOM for when the whole page is genuinely needed. This keeps Claude both
    effective and token-cheap.
  - **Gotchas (hard-won project memory):** `wait_for:"load"` not `"idle"` on
    ad/tracker-heavy sites (idle's 5s cap times out); `trim:true`; check
    `browser_status` before assuming a browser is open.
  - **Wall detection → handoff:** "Just a moment" / Cloudflare ray / Imperva /
    DataDome markers → invoke the `bypass` skill.
  - **Cleanup:** close when done (server also cleans owned Chrome on exit).
  - **Responsible use:** authorized access only (your own sites, permitted
    content, authorized testing, research); respect rate limits.

### `bypass` — wall-solving reference (loads only when a wall appears)

- **Trigger:** "Use when a page is blocked by Cloudflare Turnstile,
  Imperva/Incapsula, or DataDome — detecting which wall and solving it to reach
  content."
- **Body:** per-wall detection signatures → matching
  `browser_solve_turnstile|imperva|datadome` → failure handling. Honors the
  project's stealth-hardening philosophy: solves are **best-effort**; hard
  interactive challenges surface to the user rather than pretending to bypass.
  Responsible-use framing: bypass is for legitimate access, not
  mass-targeting/abuse.

### `advanced` — power features (loads on demand)

- **Trigger:** "Use for network interception/tracker-blocking, live network
  monitoring, event-waiting (expect), multi-tab/frame work, cookie/storage
  persistence, or capturing pages as PDF/MHTML/screenshot with zendriver."
- **Body:** sectioned reference for `intercept_*`, `monitor_*`, `expect_*`,
  tabs/frames, cookies/storage persist, and capture tools. Keeps the always-on
  `scraping` skill lean.

## Section 3 — Commands & subagent

**Split that avoids subagent overhead on small jobs:** a conversational scrape
("grab the prices from X") runs on the main thread following the `scraping`
skill inline; explicit `/zendriver:scrape`, structured extraction, or
big/background jobs dispatch the `zendriver-scraper` subagent. Both obey the same
skill.

### Commands (`.md` + frontmatter `description`, `argument-hint`; body is the prompt)

- **`/zendriver:setup`** — provisioning. Instructs Claude to detect OS/arch
  (`uname -sm`), probe for `cargo`/`gh`, ask the user the 3 delivery modes
  (prebuilt recommended), run `bash ${CLAUDE_PLUGIN_ROOT}/scripts/setup.sh
  --mode <choice>`, then tell them to restart the session. Uses Bash +
  AskUserQuestion (main-thread tools).
- **`/zendriver:scrape <url> [goal]`** — flagship. Dispatches `zendriver-scraper`
  with URL + goal (`goal` defaults to "full readable content"); returns the
  result. Backgroundable for big jobs.
- **`/zendriver:extract <url> <schema|fields>`** — structured. Dispatches the
  subagent with an instruction to return only valid JSON matching the given
  schema/field list.

### Subagent (`agents/zendriver-scraper.md`)

- **Frontmatter:** `name: zendriver-scraper`; a `description` (scraping/extraction
  specialist; used by the commands and dispatchable for background extraction);
  `tools` scoped to the zendriver MCP server + `Skill` (to load the
  `scraping`/`bypass` skills) — wildcard `mcp__plugin_zendriver_zendriver__*` if
  the agent `tools` field supports it, else enumerated (confirm at impl). No
  model override — inherits the session model (tunable later).
- **System prompt:** "You are a stealth-scraping specialist. Invoke the
  `zendriver:scraping` skill and follow it. Run goto→wait→detect-wall→(invoke
  `bypass`)→**surgical** extract. Prefer targeted `find`/`get_links`/`evaluate`
  over dumping full DOM. Return the extraction (or strict JSON when a schema is
  given). Close the browser when done." Owns the loop in its own context, so the
  main thread stays clean.

## Section 4 — Hooks, responsible-use, docs sync, testing

### Hooks — one hook, and an honest cut

- **SessionStart (keep):** fast `test -x ${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp`;
  missing → emit additionalContext "zendriver binary not provisioned — run
  `/zendriver:setup`"; present → silent. Cheap, every session.
- **SessionEnd browser-close (cut):** a shell hook cannot call the
  `browser_close` MCP tool, and `pkill chrome` would kill the user's *own*
  Chrome — the exact case zendriver's `owns_process` guard protects. zendriver
  already SIGKILLs its **owned** Chrome via `kill_on_drop` when the server
  process exits ([browser.rs:2052], [browser.rs:3144]) and `close()` does
  graceful SIGTERM→SIGKILL ([browser.rs:2977]). The only gap is a hard-SIGKILL of
  the server (Drop never runs → possible orphan); the correct fix is
  **server-side** (handle SIGTERM → graceful `browser.close()`), filed as a
  separate `zendriver-mcp` hardening todo, not a plugin hook.

### Responsible-use framing

A few lines in the `scraping` and `bypass` skills: zendriver is for authorized
access (own sites, permitted content, authorized testing, research); respect
rate limits; bypass is for legitimate access, not mass-targeting/abuse. Aligns
with the existing stealth-hardening philosophy (expose capability; the user owns
responsibility).

### Docs sync (per project CLAUDE.md) — clean, no Rust churn

- **New:** the plugin's `README.md`; a "Claude Code plugin" quickstart in the
  root `README.md` and `crates/zendriver-mcp/README.md`; a plugin chapter in the
  mdBook (`docs/book/src/plugin.md`, linked from `mcp.md`).
- **N/A (nothing to regenerate):** the plugin only *consumes* existing tools, so
  there is no new/changed public Rust API → the **MCP coverage ledger, insta
  schema snapshots, and public-api baseline are untouched**. Keeps the PR free of
  generated-file churn.

### Testing / validation

- **Static:** `jq` parse of every JSON manifest (plugin.json, marketplace.json,
  .mcp.json, hooks.json) + `shellcheck scripts/setup.sh`, run by a light
  `plugin-validate` CI job.
- **E2E smoke (manual, documented):** `claude --plugin-dir plugins/zendriver` →
  `/zendriver:setup` (prebuilt) → `/zendriver:scrape <test-url>` → confirm
  extraction and clean teardown.
- **Release workflow:** exercise `release-binaries.yml` via `workflow_dispatch`
  before relying on a real tag.

## Non-goals / out of scope

- Auto-opening the browser or auto-scraping on URL paste (rejected approach C).
- A `browser_read` readability→markdown MCP tool (separate zendriver-mcp backlog
  item; the `scraping` skill's surgical-extraction guidance is the interim
  answer).
- Exposing new Rust public API or new MCP tools — the plugin consumes the
  existing surface only.

## Follow-up todos (separate from this PR)

- **Server-side shutdown hardening:** `zendriver-mcp` handles SIGTERM / stdin EOF
  with a graceful `browser.close()` so an owned Chrome is never orphaned even on
  a non-graceful server kill.

## Success criteria

- A new user runs `marketplace add` + `install` + `/zendriver:setup` (prebuilt)
  and reaches a working scrape with no Rust toolchain and no system Chrome.
- The `scraping` skill auto-triggers on scrape intent; Claude extracts
  surgically rather than dumping full DOM by default.
- `/zendriver:scrape` and `/zendriver:extract` produce content / valid JSON via
  the subagent without bloating main-thread context.
- All JSON manifests parse and `setup.sh` passes shellcheck in CI.
- No Rust public-API, ledger, or snapshot changes (clean PR).
