# Zendriver Claude Code Plugin — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Claude Code plugin in the `zendriver-rs` repo that bundles the `zendriver-mcp` server, a guided binary `/setup`, scraping skills, commands, and a subagent — so a user goes from "marketplace add" to a working stealth scrape with no Rust toolchain and no system Chrome.

**Architecture:** A single plugin at `plugins/zendriver/`, listed by a repo-root `.claude-plugin/marketplace.json`. The MCP server `command` points at a stable path (`${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp`); `scripts/setup.sh` provisions a binary there via prebuilt download / `cargo install` / symlink. Skills hold the scraping intelligence (single source of truth); commands and a subagent are thin entry points that follow the skills. A new CI workflow cross-compiles binaries onto the `zendriver-mcp-v*` GitHub Release that release-plz already creates.

**Tech Stack:** Claude Code plugin manifests (JSON), Markdown skills/commands/agents, Bash (`setup.sh`, hook), GitHub Actions (cross-compile), Rust (existing `zendriver-mcp` crate — built, not modified).

**Spec:** [docs/superpowers/specs/2026-06-13-zendriver-claude-plugin-design.md](../specs/2026-06-13-zendriver-claude-plugin-design.md)

---

## Ground-truth schema notes (verified against installed plugins)

These were confirmed by reading real manifests under `~/.claude/plugins/cache/` — use them verbatim, do not "improve" them:

- **marketplace `source`** is a plain relative-path string: `"source": "./plugins/zendriver"`. (NOT an object.)
- **agent `tools:`** is a YAML array of bare tool names, e.g. `tools: [Read, Grep]`. There is **no verified syntax** to grant "all tools of a bundled MCP server." vercel's MCP-integrating agent omits `tools:` entirely → inherits all tools. **We omit `tools:` on our subagent** and constrain behavior via the system prompt. (Scoping is a future tightening once an MCP-server wildcard is confirmed.)
- **command `.md`** frontmatter that's known-good: `description` (and the harmless `argument-hint`). Reference arguments via `$ARGUMENTS` in the body. Do not rely on `$1`/`$file` positional semantics (unverified/ambiguous).
- **plugin.json** enumerates `commands` and `agents` (array of relative paths, like vercel). `skills/`, `hooks/hooks.json`, and `.mcp.json` auto-discover — do NOT enumerate them.
- **hooks.json** `SessionStart` entries use `matcher` (regex over `startup|resume|clear|compact`) and shell-form `command` strings; `${CLAUDE_PLUGIN_ROOT}` is exported to the hook process.
- **`${CLAUDE_PLUGIN_DATA}`** resolves to `~/.claude/plugins/data/<plugin>-<marketplace>/` (here: `zendriver-zendriver-rs`), persists across updates, and expands inside `.mcp.json` `command`/`args`/`env`.

**One runtime assumption to verify FIRST in Task 16 (e2e smoke):** that `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}` expand inside a slash-command body (so `/zendriver:setup` can pass the real dest path to `setup.sh`). If they don't, the fallback in `setup.sh` (deriving the data dir from `$HOME/.claude/plugins/data/zendriver-zendriver-rs`) covers it.

---

## File structure

**New files (all under the existing repo):**

| Path | Responsibility |
|------|----------------|
| `.claude-plugin/marketplace.json` | Marketplace listing the one plugin (repo root) |
| `plugins/zendriver/.claude-plugin/plugin.json` | Plugin manifest (metadata + commands/agents enumeration) |
| `plugins/zendriver/.mcp.json` | MCP server config → stable binary path |
| `plugins/zendriver/scripts/setup.sh` | Binary provisioner (prebuilt \| source \| link) |
| `plugins/zendriver/hooks/hooks.json` | SessionStart hook registration |
| `plugins/zendriver/hooks/session-start.sh` | Hook script: nudge to `/zendriver:setup` if binary missing |
| `plugins/zendriver/skills/scraping/SKILL.md` | Canonical scrape playbook (always-on) |
| `plugins/zendriver/skills/bypass/SKILL.md` | Wall-solving reference |
| `plugins/zendriver/skills/advanced/SKILL.md` | Power features reference |
| `plugins/zendriver/commands/setup.md` | `/zendriver:setup` |
| `plugins/zendriver/commands/scrape.md` | `/zendriver:scrape` |
| `plugins/zendriver/commands/extract.md` | `/zendriver:extract` |
| `plugins/zendriver/agents/zendriver-scraper.md` | Scraping subagent |
| `plugins/zendriver/README.md` | Plugin README |
| `.github/workflows/release-binaries.yml` | Cross-compile + attach to GH release |
| `.github/workflows/plugin-validate.yml` | Lint plugin manifests + shellcheck |

**Modified files:**

| Path | Change |
|------|--------|
| `README.md` | Add a "Claude Code plugin" quickstart section |
| `crates/zendriver-mcp/README.md` | Add a "Claude Code plugin" quickstart section |
| `docs/book/src/plugin.md` | New mdBook chapter (created) |
| `docs/book/src/SUMMARY.md` | Link the new chapter |
| `docs/book/src/mcp.md` | Cross-link to the plugin chapter |

---

## Phase 1 — Plugin skeleton (loadable plugin)

### Task 1: Plugin manifest + marketplace listing

**Files:**
- Create: `.claude-plugin/marketplace.json`
- Create: `plugins/zendriver/.claude-plugin/plugin.json`

- [ ] **Step 1: Write the failing validation check**

Run (expected FAIL — files don't exist yet):
```bash
jq empty .claude-plugin/marketplace.json && jq empty plugins/zendriver/.claude-plugin/plugin.json
```
Expected: `jq: error ... No such file or directory`.

- [ ] **Step 2: Create `.claude-plugin/marketplace.json`**

```json
{
  "$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
  "name": "zendriver-rs",
  "description": "Stealth browser automation for Claude — bundles the zendriver-mcp server plus scraping skills, commands, and a subagent.",
  "owner": {
    "name": "TurtIeSocks",
    "url": "https://github.com/TurtIeSocks"
  },
  "plugins": [
    {
      "name": "zendriver",
      "description": "Stealth web scraping & extraction with Claude: bundled CDP browser MCP server, a guided binary setup, and skills that teach Claude how to drive it.",
      "source": "./plugins/zendriver",
      "category": "web",
      "keywords": ["browser", "scraping", "automation", "cdp", "stealth", "mcp"]
    }
  ]
}
```

- [ ] **Step 3: Create `plugins/zendriver/.claude-plugin/plugin.json`**

```json
{
  "name": "zendriver",
  "version": "0.1.0",
  "description": "Stealth web scraping & extraction with Claude. Bundles the zendriver-mcp Chrome-DevTools-Protocol browser server, a guided /setup, scraping skills, commands, and a subagent.",
  "author": {
    "name": "TurtIeSocks",
    "url": "https://github.com/TurtIeSocks"
  },
  "repository": "https://github.com/TurtIeSocks/zendriver-rs",
  "homepage": "https://turtiesocks.github.io/zendriver-rs/",
  "license": "MIT OR Apache-2.0",
  "keywords": ["browser", "scraping", "automation", "cdp", "stealth", "mcp"],
  "commands": [
    "./commands/setup.md",
    "./commands/scrape.md",
    "./commands/extract.md"
  ],
  "agents": [
    "./agents/zendriver-scraper.md"
  ]
}
```

- [ ] **Step 4: Run validation — expect PASS**

```bash
jq empty .claude-plugin/marketplace.json && jq empty plugins/zendriver/.claude-plugin/plugin.json && echo OK
```
Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add .claude-plugin/marketplace.json plugins/zendriver/.claude-plugin/plugin.json
git commit -m "feat(plugin): add marketplace + plugin manifest skeleton"
```

---

### Task 2: MCP server config

**Files:**
- Create: `plugins/zendriver/.mcp.json`

- [ ] **Step 1: Write the failing check**

```bash
jq -e '.mcpServers.zendriver.command' plugins/zendriver/.mcp.json
```
Expected: FAIL (file missing).

- [ ] **Step 2: Create `plugins/zendriver/.mcp.json`**

Mirrors the verified semgrep stdio pattern; `command` points at the stable provisioned path.

```json
{
  "mcpServers": {
    "zendriver": {
      "command": "${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp",
      "args": ["--log", "warn"]
    }
  }
}
```

- [ ] **Step 3: Run check — expect PASS**

```bash
jq -e '.mcpServers.zendriver.command == "${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp"' plugins/zendriver/.mcp.json
```
Expected: `true`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/.mcp.json
git commit -m "feat(plugin): wire bundled zendriver MCP server to stable binary path"
```

---

## Phase 2 — Binary delivery

### Task 3: `setup.sh` provisioner

**Files:**
- Create: `plugins/zendriver/scripts/setup.sh`

Modes: `prebuilt` (download matching asset from latest `zendriver-mcp-v*` release, verify SHA256), `source` (`cargo install`), `link` (symlink from PATH). Destination passed via `--dest`; falls back to the derived data dir.

- [ ] **Step 1: Write the failing check**

```bash
shellcheck plugins/zendriver/scripts/setup.sh && bash -n plugins/zendriver/scripts/setup.sh
```
Expected: FAIL (file missing).

- [ ] **Step 2: Create `plugins/zendriver/scripts/setup.sh`**

```bash
#!/usr/bin/env bash
# Provision the zendriver-mcp binary for the Claude Code plugin.
# Usage: setup.sh --mode <prebuilt|source|link> [--dest <path>]
set -euo pipefail

REPO="TurtIeSocks/zendriver-rs"
MODE=""
DEST="${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/zendriver-zendriver-rs}/bin/zendriver-mcp"

while [ $# -gt 0 ]; do
  case "$1" in
    --mode) MODE="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

[ -n "$MODE" ] || { echo "missing --mode <prebuilt|source|link>" >&2; exit 2; }
mkdir -p "$(dirname "$DEST")"

# Map uname to a Rust target triple + asset extension.
detect_target() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin) case "$arch" in
        arm64) echo "aarch64-apple-darwin tar.gz" ;;
        x86_64) echo "x86_64-apple-darwin tar.gz" ;;
        *) return 1 ;; esac ;;
    Linux) case "$arch" in
        x86_64) echo "x86_64-unknown-linux-gnu tar.gz" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu tar.gz" ;;
        *) return 1 ;; esac ;;
    MINGW*|MSYS*|CYGWIN*) echo "x86_64-pc-windows-msvc zip" ;;
    *) return 1 ;;
  esac
}

latest_tag() {
  # Newest release tag matching zendriver-mcp-v*
  if command -v gh >/dev/null 2>&1; then
    gh release list --repo "$REPO" --limit 30 \
      | awk '{print $1}' | grep '^zendriver-mcp-v' | head -n1
  else
    curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=30" \
      | grep -o '"tag_name": *"zendriver-mcp-v[^"]*"' \
      | head -n1 | sed 's/.*"\(zendriver-mcp-v[^"]*\)"/\1/'
  fi
}

install_prebuilt() {
  local triple ext tag tmp asset url sums
  read -r triple ext < <(detect_target) || { echo "unsupported platform: $(uname -sm)" >&2; exit 1; }
  tag="$(latest_tag)"; [ -n "$tag" ] || { echo "no zendriver-mcp-v* release found" >&2; exit 1; }
  asset="zendriver-mcp-${triple}.${ext}"
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' RETURN
  url="https://github.com/$REPO/releases/download/$tag"
  echo "Downloading $asset from $tag ..." >&2
  curl -fsSL "$url/$asset" -o "$tmp/$asset"
  curl -fsSL "$url/SHA256SUMS" -o "$tmp/SHA256SUMS"
  ( cd "$tmp" && grep " $asset\$" SHA256SUMS | shasum -a 256 -c - ) \
    || { echo "checksum verification FAILED" >&2; exit 1; }
  if [ "$ext" = "zip" ]; then unzip -o "$tmp/$asset" -d "$tmp" >/dev/null
  else tar -xzf "$tmp/$asset" -C "$tmp"; fi
  install -m 0755 "$tmp/zendriver-mcp"* "$DEST" 2>/dev/null \
    || { mv "$tmp"/zendriver-mcp* "$DEST"; chmod +x "$DEST"; }
  echo "Installed prebuilt binary -> $DEST" >&2
}

install_source() {
  command -v cargo >/dev/null 2>&1 || { echo "cargo not found; install Rust or use --mode prebuilt" >&2; exit 1; }
  local root; root="$(dirname "$(dirname "$DEST")")"   # .../zendriver-zendriver-rs
  echo "Building from source via cargo (this can take several minutes) ..." >&2
  cargo install zendriver-mcp --root "$root"
  echo "Installed source build -> $DEST" >&2
}

install_link() {
  local found; found="$(command -v zendriver-mcp || true)"
  [ -n "$found" ] || { echo "no zendriver-mcp on PATH to link" >&2; exit 1; }
  ln -sf "$found" "$DEST"
  echo "Linked $found -> $DEST" >&2
}

case "$MODE" in
  prebuilt) install_prebuilt ;;
  source)   install_source ;;
  link)     install_link ;;
  *) echo "invalid --mode: $MODE" >&2; exit 2 ;;
esac

"$DEST" --version >/dev/null 2>&1 && echo "OK: zendriver-mcp runs." >&2 || echo "WARN: installed but --version check failed." >&2
```

Note: `cargo install --root <root>` lands the binary at `<root>/bin/zendriver-mcp`, which equals `$DEST` when `--dest` is the default. The `install_source` derivation assumes that layout.

- [ ] **Step 3: Run check — expect PASS**

```bash
shellcheck plugins/zendriver/scripts/setup.sh && bash -n plugins/zendriver/scripts/setup.sh && echo OK
```
Expected: `OK` (no shellcheck warnings). Fix any SC findings inline.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/scripts/setup.sh
git commit -m "feat(plugin): add setup.sh binary provisioner (prebuilt/source/link)"
```

---

### Task 4: SessionStart hook + nudge script

**Files:**
- Create: `plugins/zendriver/hooks/hooks.json`
- Create: `plugins/zendriver/hooks/session-start.sh`

- [ ] **Step 1: Write the failing check**

```bash
jq empty plugins/zendriver/hooks/hooks.json && shellcheck plugins/zendriver/hooks/session-start.sh
```
Expected: FAIL (missing).

- [ ] **Step 2: Create `plugins/zendriver/hooks/hooks.json`**

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume|clear",
        "hooks": [
          {
            "type": "command",
            "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh\""
          }
        ]
      }
    ]
  }
}
```

- [ ] **Step 3: Create `plugins/zendriver/hooks/session-start.sh`**

Emits a context nudge only when the binary is missing; silent otherwise. Uses the `SessionStart` stdout JSON contract.

```bash
#!/usr/bin/env bash
# If the zendriver-mcp binary isn't provisioned, nudge the user to run /zendriver:setup.
set -euo pipefail

DEST="${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/zendriver-zendriver-rs}/bin/zendriver-mcp"

if [ ! -x "$DEST" ]; then
  cat <<'JSON'
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "The zendriver MCP binary is not provisioned yet, so the zendriver browser tools will be unavailable until it is. If the user wants to use zendriver, tell them to run the /zendriver:setup command once (it offers a prebuilt download, a from-source cargo build, or linking an existing binary), then restart the session."
  }
}
JSON
fi
exit 0
```

- [ ] **Step 4: Run check — expect PASS**

```bash
jq empty plugins/zendriver/hooks/hooks.json && shellcheck plugins/zendriver/hooks/session-start.sh && bash -n plugins/zendriver/hooks/session-start.sh && echo OK
```
Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add plugins/zendriver/hooks/
git commit -m "feat(plugin): SessionStart hook nudges /zendriver:setup when binary missing"
```

---

## Phase 3 — Skills (parallelizable: Tasks 5, 6, 7 are independent)

### Task 5: `scraping` skill (canonical playbook)

**Files:**
- Create: `plugins/zendriver/skills/scraping/SKILL.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/skills/scraping/SKILL.md && head -1 plugins/zendriver/skills/scraping/SKILL.md | grep -q '^---$'
```
Expected: FAIL (missing).

- [ ] **Step 2: Create `plugins/zendriver/skills/scraping/SKILL.md`**

```markdown
---
name: scraping
description: Use when scraping, extracting, or reading content or data from a website with zendriver — especially JS-rendered SPAs, bot-walled pages, or anything plain fetch/WebFetch can't reach. Covers the open→goto→wait→extract flow, surgical extraction, tool selection, and gotchas.
---

# Scraping with zendriver

The canonical playbook for driving the zendriver browser well. The `/zendriver:scrape`
command and the `zendriver-scraper` subagent both follow this skill — it is the single
source of truth.

## When zendriver is the right tool

Reach for zendriver when the page is **JS-rendered**, **bot-walled**, or needs
**interaction/session state**. For static HTML a plain fetch is lighter — don't spin up a
browser to read a sitemap.

## Core flow

1. `browser_status` — is a browser already open this session? Reuse it; don't re-open.
2. `browser_open` — launch once. Stealth is **on by default**; do NOT re-enable it or set a
   profile unless a site specifically needs it.
3. `browser_goto { url, wait_for: "load" }` — navigate.
4. **Extract** (see below).
5. Close with `browser_close` when the job is done (the server also kills its owned Chrome on
   exit).

## The effectiveness lever: surgical extraction, not DOM-dumping

`browser_html { trim: true }` returns the whole page — ~40–50k tokens for a real article.
**Only dump full DOM when you genuinely need the whole page.** When the goal is specific,
pull just that:

- A few known elements / text → `browser_find` (one) or `browser_find_all` (many).
- All links → `browser_get_links`.
- Structured data, tables, computed values, JSON embedded in `<script>` → `browser_evaluate`
  with a small JS snippet that returns exactly the shape you want.
- "What resources/endpoints did this page load?" → `browser_search_resources`.

Default to targeted tools; reserve `browser_html { trim: true }` for genuinely unstructured
full-page reads.

## Gotchas (hard-won)

- **`wait_for: "load"`, not `"idle"`** on ad/tracker-heavy sites — idle's ~5s cap times out
  waiting on hanging tracker requests. Use `load` and read the DOM regardless; escalate to
  `browser_wait_for_idle` only for SPAs that genuinely settle.
- **Always `trim: true`** on `browser_html` unless you need raw markup.
- **Check `browser_status`** before assuming a browser/tab is open — calling navigation on a
  closed browser errors.
- A page that loaded but shows little content is often **JS-gated** — give it a
  `browser_wait_for_load` / short `browser_wait_for_idle`, or check for a wall (next section).

## Wall detection → hand off

If the DOM shows an anti-bot interstitial instead of content — "Just a moment…", a Cloudflare
ray ID, an Imperva/Incapsula notice, or a DataDome block — **invoke the `bypass` skill**
(via the Skill tool) and follow it, then resume extraction.

## Multi-page / pagination

Loop the core flow per URL. Prefer `browser_get_links` to discover "next" targets over
guessing URL patterns. For big crawls, run inside the `zendriver-scraper` subagent (and
background it) so the main thread's context stays clean.

## Responsible use

zendriver is for **authorized access**: your own sites, content you're permitted to read,
authorized testing, and research. Respect `robots`-style intent and rate limits; throttle
between requests. Don't use it for mass-targeting or to defeat access controls you have no
right to bypass.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/skills/scraping/SKILL.md && head -1 plugins/zendriver/skills/scraping/SKILL.md | grep -q '^---$' && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/skills/scraping/SKILL.md
git commit -m "feat(plugin): add canonical zendriver scraping skill"
```

---

### Task 6: `bypass` skill

**Files:**
- Create: `plugins/zendriver/skills/bypass/SKILL.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/skills/bypass/SKILL.md
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/skills/bypass/SKILL.md`**

```markdown
---
name: bypass
description: Use when a page is blocked by an anti-bot wall — Cloudflare Turnstile, Imperva/Incapsula, or DataDome — to detect which wall it is and solve it so the underlying content loads.
---

# Bypassing anti-bot walls with zendriver

Invoked from the `scraping` skill when an interstitial replaces real content. Identify the
wall, call the matching solver, verify, then resume extraction.

## Detect which wall

Look at the current DOM / title / response:

| Signal | Wall | Solver |
|--------|------|--------|
| "Just a moment…", `cf-chl`, `__cf` cookies, Turnstile iframe, Cloudflare ray ID | Cloudflare Turnstile | `browser_solve_turnstile` |
| "Incapsula", `_Incapsula_Resource`, `visid_incap_*` cookies, Imperva notice | Imperva / Incapsula | `browser_solve_imperva` |
| `datadome` cookie, "DataDome", `geo.captcha-delivery.com` | DataDome | `browser_solve_datadome` |

If you can't tell, re-`browser_html { trim: true }` and inspect; don't guess-fire a solver.

## Solve flow

1. Confirm the wall (table above).
2. Call the matching `browser_solve_*` tool.
3. Re-check: `browser_html { trim: true }` (or a `browser_find` for known content) — did the
   real page load?
4. If solved → return to the `scraping` skill and extract.

## When a solve doesn't stick

These walls are an active arms race; solves are **best-effort**, not guaranteed:

- Retry once (transient challenges happen).
- Try a fresh navigation (`browser_reload` or re-`browser_goto`).
- If a wall keeps returning, **surface it to the user** — say plainly that the site is
  serving a challenge the automated solver can't clear, and let them decide (manual solve,
  different approach, or stop). Do not loop indefinitely, and do not pretend a page loaded
  when it didn't.

## Responsible use

Solving a wall is for reaching content you're **authorized** to access. Don't use these tools
for mass-targeting, credential-stuffing, or defeating access controls you have no right to
bypass.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/skills/bypass/SKILL.md && head -1 plugins/zendriver/skills/bypass/SKILL.md | grep -q '^---$' && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/skills/bypass/SKILL.md
git commit -m "feat(plugin): add zendriver anti-bot bypass skill"
```

---

### Task 7: `advanced` skill

**Files:**
- Create: `plugins/zendriver/skills/advanced/SKILL.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/skills/advanced/SKILL.md
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/skills/advanced/SKILL.md`**

```markdown
---
name: advanced
description: Use for zendriver power features beyond a basic scrape — network interception and tracker-blocking, live network monitoring, event-waiting (expect), multi-tab and iframe work, cookie/storage persistence across sessions, file upload/download, and capturing pages as PDF, MHTML, or screenshots.
---

# zendriver advanced toolkit

Reference for capabilities the always-on `scraping` skill leaves out. Pull the section you
need; skip the rest.

## Capture / archive

- `browser_screenshot` — viewport or full-page PNG.
- `browser_pdf` — print-to-PDF of the current page.
- `browser_save_mhtml` — single-file MHTML archive (DOM + resources).
- `browser_get_links` — every link on the page.

## Network interception & tracker-blocking

- `browser_intercept_add_rule` — block, redirect, or modify matching requests.
- `browser_intercept_list_rules` / `browser_intercept_remove_rule` / `browser_intercept_clear_rules`.
- Use to block trackers/ads (faster, cleaner scrapes), stub endpoints, or fault-inject.
- Chain multiple interception needs into ONE rule set on the same tab — don't stand up
  competing interceptors.

## Live network monitoring

- `browser_monitor_start` → `browser_monitor_read` → `browser_monitor_stop` — observe
  requests/responses as they happen (find the JSON API behind a page, watch XHR/fetch).

## Event-waiting (expect)

- `browser_expect_register` → `browser_expect_await` (`browser_expect_cancel` to drop) —
  wait for a specific navigation/request/response/download instead of a blind sleep. Ideal
  for "click submit, then wait for the POST to complete".

## Tabs & frames

- `browser_tab_new` / `browser_tab_list` / `browser_tab_switch` / `browser_tab_activate` /
  `browser_tab_close` — multi-tab flows.
- `browser_frame_list` / `browser_frame_goto` — drive content inside iframes.

## Sessions: cookies & storage

- `browser_cookies_get/set/delete/clear` and `browser_storage_get/set/delete/clear`.
- `browser_cookies_persist` — keep a logged-in session across runs (set a download/profile
  path as needed). Treat saved cookies as secrets.

## Files

- `browser_upload` — attach a file to an `<input type=file>`.
- `browser_download` / `browser_set_download_path` — capture downloads to a known dir.

## Misc

- `browser_set_user_agent`, `browser_set_stealth_profile` — only when a site needs a specific
  identity; the defaults are already stealthy.
- `browser_request` — issue a raw HTTP request through the browser context (reuses cookies).
- `browser_install_chrome` — fetch Chrome explicitly (normally automatic on first open).
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/skills/advanced/SKILL.md && head -1 plugins/zendriver/skills/advanced/SKILL.md | grep -q '^---$' && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/skills/advanced/SKILL.md
git commit -m "feat(plugin): add zendriver advanced toolkit skill"
```

---

## Phase 4 — Commands & subagent

### Task 8: `zendriver-scraper` subagent

**Files:**
- Create: `plugins/zendriver/agents/zendriver-scraper.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/agents/zendriver-scraper.md && grep -q '^name: zendriver-scraper$' plugins/zendriver/agents/zendriver-scraper.md
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/agents/zendriver-scraper.md`**

`tools:` is omitted on purpose (see ground-truth notes) so the agent inherits the zendriver MCP tools + `Skill`. Behavior is constrained by the prompt.

```markdown
---
name: zendriver-scraper
description: Stealth web-scraping and extraction specialist. Use to scrape or extract content/data from a URL with the zendriver browser — especially JS-rendered, bot-walled, or interaction-heavy pages, or large/background extraction jobs that shouldn't clutter the main thread's context. Returns the extracted content, or strict JSON when given a schema.
---

You are a stealth web-scraping specialist driving the zendriver browser (CDP) tools.

**Follow the `scraping` skill.** Invoke it via the Skill tool at the start and obey it as your
playbook. If you hit an anti-bot wall, invoke the `bypass` skill. For capture/interception/
multi-tab needs, invoke the `advanced` skill.

## Loop

1. `browser_status`; `browser_open` if needed (stealth is default-on — leave it).
2. `browser_goto { url, wait_for: "load" }`.
3. Detect a wall → follow `bypass` → resume.
4. **Extract surgically.** Prefer `browser_find` / `browser_find_all` / `browser_get_links` /
   `browser_evaluate` to pull exactly what the goal asks for. Use `browser_html { trim: true }`
   only when you truly need the whole page.
5. `browser_close` when finished.

## Output contract

- Default goal ("full readable content"): return the cleaned main content as Markdown, plus a
  one-line note of the URL and any wall you cleared.
- If given a schema or field list: return **only** a single valid JSON object/array matching
  it — no prose, no code fence. Use `null` for fields you genuinely couldn't find; never
  invent values.
- If the page couldn't be reached (wall unsolved, hard error): say so plainly and stop —
  don't fabricate content.

## Discipline

- Don't dump 50k-token full DOM when a `browser_find` answers the goal.
- Respect rate limits; throttle on multi-page jobs.
- Authorized access only — your scrape is for content the user is permitted to read.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/agents/zendriver-scraper.md && grep -q '^name: zendriver-scraper$' plugins/zendriver/agents/zendriver-scraper.md && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/agents/zendriver-scraper.md
git commit -m "feat(plugin): add zendriver-scraper subagent"
```

---

### Task 9: `/zendriver:setup` command

**Files:**
- Create: `plugins/zendriver/commands/setup.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/commands/setup.md && head -1 plugins/zendriver/commands/setup.md | grep -q '^---$'
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/commands/setup.md`**

```markdown
---
description: Provision the zendriver-mcp binary for this plugin. Offers a prebuilt download (no Rust), a from-source cargo build, or linking an existing binary. Run once, then restart the session.
argument-hint: "[prebuilt|source|link]"
---

# Set up zendriver

Provision the `zendriver-mcp` binary so the bundled MCP server can start.

## Steps

1. **Resolve paths.** The provisioner script is at `${CLAUDE_PLUGIN_ROOT}/scripts/setup.sh`
   and the binary must land at `${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp`. First run
   `echo "ROOT=$CLAUDE_PLUGIN_ROOT DATA=$CLAUDE_PLUGIN_DATA"` to confirm both resolve to real
   paths. If `CLAUDE_PLUGIN_DATA` is empty, fall back to
   `$HOME/.claude/plugins/data/zendriver-zendriver-rs`.

2. **Probe the environment** so you can recommend a mode:
   - `uname -sm` (platform — is a prebuilt likely available?).
   - `command -v cargo` (can we build from source?).
   - `command -v zendriver-mcp` (is one already on PATH to link?).

3. **Choose a mode.** If the user passed one in `$ARGUMENTS` (`prebuilt`, `source`, or
   `link`), use it. Otherwise ask them with AskUserQuestion, presenting:
   - **Download prebuilt** *(recommended — fast, no Rust)*: fetches the matching binary from
     the latest GitHub release and verifies its checksum.
   - **Build from source** *(needs Rust; a few minutes)*: compiles the public source yourself
     with `cargo install` — choose this if you'd rather not run a prebuilt binary.
   - **Link existing**: symlink a `zendriver-mcp` already on your PATH.

4. **Run the provisioner** with the chosen mode:
   ```bash
   bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh" --mode <prebuilt|source|link> \
     --dest "${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/zendriver-zendriver-rs}/bin/zendriver-mcp"
   ```
   Surface the script's output. If it fails (e.g. prebuilt unavailable for the platform),
   suggest another mode.

5. **Tell the user to restart the session** (or run `/reload-plugins` if available) so the
   `zendriver` MCP server picks up the new binary. On first `browser_open`, Chrome is fetched
   automatically — no system Chrome needed.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/commands/setup.md && head -1 plugins/zendriver/commands/setup.md | grep -q '^---$' && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/commands/setup.md
git commit -m "feat(plugin): add /zendriver:setup command"
```

---

### Task 10: `/zendriver:scrape` command

**Files:**
- Create: `plugins/zendriver/commands/scrape.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/commands/scrape.md
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/commands/scrape.md`**

```markdown
---
description: Scrape a URL with the zendriver stealth browser and return its content. Dispatches the zendriver-scraper subagent. Usage: /zendriver:scrape <url> [goal].
argument-hint: "<url> [what to extract]"
---

# Scrape a URL with zendriver

Arguments (`$ARGUMENTS`): the first token is the URL; everything after it is the extraction
goal. If no goal is given, default to "full readable main content as Markdown".

1. Parse `$ARGUMENTS` into `<url>` and `<goal>`.
2. Dispatch the **`zendriver-scraper`** subagent (Task/Agent tool, `subagent_type:
   zendriver-scraper`) with a prompt like:
   > Scrape `<url>`. Goal: `<goal>`. Follow the scraping skill; bypass any wall; extract
   > surgically; close the browser when done.
   For a large or slow job, run it in the background.
3. Return the subagent's result to the user. Don't re-scrape in the main thread.

If the subagent reports the binary/tools are unavailable, tell the user to run
`/zendriver:setup` first.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/commands/scrape.md && head -1 plugins/zendriver/commands/scrape.md | grep -q '^---$' && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/commands/scrape.md
git commit -m "feat(plugin): add /zendriver:scrape command"
```

---

### Task 11: `/zendriver:extract` command

**Files:**
- Create: `plugins/zendriver/commands/extract.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/commands/extract.md
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/commands/extract.md`**

```markdown
---
description: Extract structured JSON from a URL with the zendriver stealth browser. Dispatches the zendriver-scraper subagent and returns only JSON matching your schema. Usage: /zendriver:extract <url> <schema-or-fields>.
argument-hint: "<url> <json-schema or field list>"
---

# Extract structured data with zendriver

Arguments (`$ARGUMENTS`): the first token is the URL; the remainder is a JSON schema or a
comma/space-separated field list describing the shape you want.

1. Parse `$ARGUMENTS` into `<url>` and `<schema>`.
2. Dispatch the **`zendriver-scraper`** subagent (`subagent_type: zendriver-scraper`) with:
   > Extract from `<url>` into this exact shape: `<schema>`. Follow the scraping skill; bypass
   > any wall. Return ONLY a single valid JSON value matching the shape — no prose, no code
   > fence. Use null for fields you can't find; never invent values.
3. Validate the returned text parses as JSON (`jq empty`). If not, ask the subagent to
   re-emit valid JSON.
4. Return the JSON to the user.

If tools are unavailable, tell the user to run `/zendriver:setup` first.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/commands/extract.md && head -1 plugins/zendriver/commands/extract.md | grep -q '^---$' && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/commands/extract.md
git commit -m "feat(plugin): add /zendriver:extract command"
```

---

## Phase 5 — Plugin README

### Task 12: Plugin README

**Files:**
- Create: `plugins/zendriver/README.md`

- [ ] **Step 1: Write the failing check**

```bash
test -f plugins/zendriver/README.md
```
Expected: FAIL.

- [ ] **Step 2: Create `plugins/zendriver/README.md`**

```markdown
# zendriver — Claude Code plugin

Stealth web scraping & extraction for Claude, powered by
[zendriver-rs](https://github.com/TurtIeSocks/zendriver-rs). Bundles the `zendriver-mcp`
Chrome-DevTools-Protocol browser server plus skills, commands, and a subagent that teach
Claude how to drive it well.

## Install

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
```

Then, in a session, provision the browser binary once:

```
/zendriver:setup
```

Pick **prebuilt** (fast, no Rust), **source** (compile it yourself), or **link** (reuse a
`zendriver-mcp` already on your PATH), then restart the session. Chrome is downloaded
automatically on first use — no system Chrome required.

## What you get

- **MCP server** — ~80 stealth browser tools (`browser_goto`, `browser_html`, `browser_find`,
  `browser_solve_turnstile`, …).
- **Skills** — `scraping` (the playbook, auto-triggers), `bypass` (anti-bot walls), `advanced`
  (interception, monitoring, capture, sessions).
- **Commands** — `/zendriver:setup`, `/zendriver:scrape <url> [goal]`,
  `/zendriver:extract <url> <schema>`.
- **Subagent** — `zendriver-scraper` runs scrape/extract jobs in its own context.

## Responsible use

For authorized access only — your own sites, permitted content, authorized testing, research.
Respect rate limits and site terms.
```

- [ ] **Step 3: Run check — expect PASS**

```bash
test -f plugins/zendriver/README.md && grep -q "claude plugin install zendriver@zendriver-rs" plugins/zendriver/README.md && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add plugins/zendriver/README.md
git commit -m "docs(plugin): add plugin README"
```

---

## Phase 6 — CI: release binaries + manifest validation

### Task 13: Cross-compile release workflow

**Files:**
- Create: `.github/workflows/release-binaries.yml`

Triggers on the `zendriver-mcp-v*` tag release-plz creates; builds 5 targets; uploads
binaries + `SHA256SUMS` + provenance to that release.

- [ ] **Step 1: Write the failing check**

```bash
test -f .github/workflows/release-binaries.yml
```
Expected: FAIL.

- [ ] **Step 2: Create `.github/workflows/release-binaries.yml`**

```yaml
name: release-binaries

on:
  push:
    tags:
      - "zendriver-mcp-v*"
  workflow_dispatch:
    inputs:
      tag:
        description: "Existing zendriver-mcp-v* release tag to attach binaries to"
        required: true

permissions:
  contents: write          # upload release assets
  id-token: write          # provenance attestation
  attestations: write

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
          - os: ubuntu-latest
            target: aarch64-unknown-linux-gnu
            linker: gcc-aarch64-linux-gnu
          - os: macos-latest
            target: aarch64-apple-darwin
          - os: macos-latest
            target: x86_64-apple-darwin
          - os: windows-latest
            target: x86_64-pc-windows-msvc
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust + target
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross linker (linux aarch64)
        if: matrix.linker != ''
        run: |
          sudo apt-get update
          sudo apt-get install -y ${{ matrix.linker }}
          echo "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc" >> "$GITHUB_ENV"

      - name: Build
        run: cargo build --release --locked -p zendriver-mcp --bin zendriver-mcp --target ${{ matrix.target }}

      - name: Package (unix)
        if: matrix.os != 'windows-latest'
        shell: bash
        run: |
          BIN=target/${{ matrix.target }}/release/zendriver-mcp
          OUT=zendriver-mcp-${{ matrix.target }}.tar.gz
          tar -czf "$OUT" -C "$(dirname "$BIN")" zendriver-mcp
          echo "ASSET=$OUT" >> "$GITHUB_ENV"
          echo "RAWBIN=$BIN" >> "$GITHUB_ENV"

      - name: Package (windows)
        if: matrix.os == 'windows-latest'
        shell: bash
        run: |
          BIN=target/${{ matrix.target }}/release/zendriver-mcp.exe
          OUT=zendriver-mcp-${{ matrix.target }}.zip
          7z a "$OUT" "./$BIN" >/dev/null
          echo "ASSET=$OUT" >> "$GITHUB_ENV"
          echo "RAWBIN=$BIN" >> "$GITHUB_ENV"

      - name: Checksum
        shell: bash
        run: shasum -a 256 "$ASSET" > "$ASSET.sha256"

      - name: Attest build provenance
        uses: actions/attest-build-provenance@v1
        with:
          subject-path: ${{ env.RAWBIN }}

      - name: Upload to release
        shell: bash
        env:
          GH_TOKEN: ${{ github.token }}
          TAG: ${{ github.event.inputs.tag || github.ref_name }}
        run: gh release upload "$TAG" "$ASSET" "$ASSET.sha256" --repo "$GITHUB_REPOSITORY" --clobber

  checksums:
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - name: Assemble combined SHA256SUMS
        env:
          GH_TOKEN: ${{ github.token }}
          TAG: ${{ github.event.inputs.tag || github.ref_name }}
        run: |
          mkdir dl && cd dl
          gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern '*.sha256'
          cat *.sha256 | sed 's#.*/##' > SHA256SUMS
          gh release upload "$TAG" SHA256SUMS --repo "$GITHUB_REPOSITORY" --clobber
```

Note: each per-target `.sha256` is named after its tarball; the `checksums` job concatenates
them into the single `SHA256SUMS` that `setup.sh` downloads. `setup.sh` greps for the asset
line and pipes to `shasum -a 256 -c`.

- [ ] **Step 3: Validate workflow YAML**

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release-binaries.yml')); print('OK')"
```
Expected: `OK`. (Full execution is validated in Task 15 via `workflow_dispatch`.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release-binaries.yml
git commit -m "ci: cross-compile zendriver-mcp binaries onto release tags"
```

---

### Task 14: Plugin-manifest validation workflow

**Files:**
- Create: `.github/workflows/plugin-validate.yml`

- [ ] **Step 1: Write the failing check**

```bash
test -f .github/workflows/plugin-validate.yml
```
Expected: FAIL.

- [ ] **Step 2: Create `.github/workflows/plugin-validate.yml`**

```yaml
name: plugin-validate

on:
  push:
    paths:
      - "plugins/**"
      - ".claude-plugin/**"
      - ".github/workflows/plugin-validate.yml"
  pull_request:
    paths:
      - "plugins/**"
      - ".claude-plugin/**"

jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Validate JSON manifests
        run: |
          set -e
          for f in .claude-plugin/marketplace.json \
                   plugins/zendriver/.claude-plugin/plugin.json \
                   plugins/zendriver/.mcp.json \
                   plugins/zendriver/hooks/hooks.json; do
            echo "jq $f"; jq empty "$f"
          done

      - name: Assert required fields
        run: |
          jq -e '.name=="zendriver-rs" and (.plugins[0].source=="./plugins/zendriver")' .claude-plugin/marketplace.json
          jq -e '.name=="zendriver"' plugins/zendriver/.claude-plugin/plugin.json
          jq -e '.mcpServers.zendriver.command=="${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp"' plugins/zendriver/.mcp.json

      - name: Shellcheck scripts
        run: |
          sudo apt-get update && sudo apt-get install -y shellcheck
          shellcheck plugins/zendriver/scripts/setup.sh plugins/zendriver/hooks/session-start.sh

      - name: Skill/command/agent frontmatter present
        run: |
          for f in plugins/zendriver/skills/*/SKILL.md \
                   plugins/zendriver/commands/*.md \
                   plugins/zendriver/agents/*.md; do
            head -1 "$f" | grep -q '^---$' || { echo "missing frontmatter: $f"; exit 1; }
          done
          echo "frontmatter OK"
```

- [ ] **Step 3: Validate workflow YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/plugin-validate.yml')); print('OK')"
```
Expected: `OK`.

- [ ] **Step 4: Run the validate steps locally**

```bash
for f in .claude-plugin/marketplace.json plugins/zendriver/.claude-plugin/plugin.json plugins/zendriver/.mcp.json plugins/zendriver/hooks/hooks.json; do jq empty "$f"; done
shellcheck plugins/zendriver/scripts/setup.sh plugins/zendriver/hooks/session-start.sh
echo OK
```
Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/plugin-validate.yml
git commit -m "ci: validate plugin manifests + shellcheck plugin scripts"
```

---

## Phase 7 — Documentation sync

### Task 15: READMEs + mdBook chapter

**Files:**
- Modify: `README.md` (add a "Claude Code plugin" section near the existing MCP/install content)
- Modify: `crates/zendriver-mcp/README.md` (same section)
- Create: `docs/book/src/plugin.md`
- Modify: `docs/book/src/SUMMARY.md` (add the chapter link)
- Modify: `docs/book/src/mcp.md` (cross-link the plugin chapter)

- [ ] **Step 1: Read current docs to place sections correctly**

```bash
sed -n '1,60p' README.md
grep -n "Install\|## " crates/zendriver-mcp/README.md | head
cat docs/book/src/SUMMARY.md
grep -n "^# \|^## " docs/book/src/mcp.md | head
```

- [ ] **Step 2: Add the same quickstart block to both READMEs**

Insert under the MCP install heading in `README.md` and `crates/zendriver-mcp/README.md`:

```markdown
### As a Claude Code plugin

The fastest path for Claude Code users — no manual MCP wiring:

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
# then, in a session:
/zendriver:setup     # pick prebuilt (no Rust), source, or link
```

You get the MCP server plus scraping skills, the `/zendriver:scrape` and `/zendriver:extract`
commands, and a `zendriver-scraper` subagent. See [`plugins/zendriver/`](plugins/zendriver/).
```

(In `crates/zendriver-mcp/README.md`, adjust the relative link to `../../plugins/zendriver/`.)

- [ ] **Step 3: Create `docs/book/src/plugin.md`**

```markdown
# Claude Code plugin

The `zendriver` Claude Code plugin bundles the [MCP server](./mcp.md) with skills, commands,
and a subagent so Claude can drive zendriver effectively out of the box.

## Install

```bash
claude plugin marketplace add TurtIeSocks/zendriver-rs
claude plugin install zendriver@zendriver-rs
/zendriver:setup
```

`/zendriver:setup` provisions the `zendriver-mcp` binary — choose:

- **prebuilt** — download the matching binary from the latest `zendriver-mcp-v*` GitHub
  release (checksum-verified). No Rust toolchain.
- **source** — `cargo install zendriver-mcp` (compile the public source yourself).
- **link** — reuse a `zendriver-mcp` already on your PATH.

Restart the session afterward. Chrome is fetched automatically on first use.

## What's included

| Kind | Items |
|------|-------|
| MCP server | the full zendriver browser tool surface |
| Skills | `scraping` (canonical playbook), `bypass` (anti-bot walls), `advanced` (interception/monitor/capture/sessions) |
| Commands | `/zendriver:setup`, `/zendriver:scrape <url> [goal]`, `/zendriver:extract <url> <schema>` |
| Subagent | `zendriver-scraper` |

## How it fits together

The `scraping` skill is the single source of truth for "how to scrape well"; the commands and
the subagent both follow it. The subagent runs jobs in its own context so big extractions
don't fill the main thread.

## Responsible use

Authorized access only — your own sites, permitted content, authorized testing, research.
Respect rate limits and site terms.
```

- [ ] **Step 4: Link the chapter in `docs/book/src/SUMMARY.md`**

Add (placed sensibly near the MCP entry — match the file's existing list style):

```markdown
- [Claude Code plugin](./plugin.md)
```

- [ ] **Step 5: Cross-link from `docs/book/src/mcp.md`**

Add near the top of `mcp.md`:

```markdown
> Using Claude Code? The [`zendriver` plugin](./plugin.md) installs this server plus skills,
> commands, and a subagent in two commands.
```

- [ ] **Step 6: Verify the book builds**

```bash
mdbook build docs/book && echo OK
```
Expected: `OK` (no missing-link or SUMMARY errors).

- [ ] **Step 7: Commit**

```bash
git add README.md crates/zendriver-mcp/README.md docs/book/src/plugin.md docs/book/src/SUMMARY.md docs/book/src/mcp.md
git commit -m "docs: document the zendriver Claude Code plugin (READMEs + mdBook)"
```

---

## Phase 8 — End-to-end verification

### Task 16: Local plugin smoke test

**Files:** none (manual verification + notes).

This is the real functional test — the static checks above only prove the files parse.

- [ ] **Step 1: Load the plugin from the working tree**

```bash
claude --plugin-dir plugins/zendriver
```
In that session, confirm the SessionStart nudge appears (binary not yet provisioned) and
that `/zendriver:setup`, `/zendriver:scrape`, `/zendriver:extract` are listed.

- [ ] **Step 2: Verify path-variable expansion (the one runtime assumption)**

Run `/zendriver:setup` and confirm the `echo "ROOT=$CLAUDE_PLUGIN_ROOT DATA=$CLAUDE_PLUGIN_DATA"`
step prints real paths. If `CLAUDE_PLUGIN_DATA` is empty in the command's Bash env, confirm
the `$HOME/.claude/plugins/data/zendriver-zendriver-rs` fallback is used. Record the result in
the plugin README's troubleshooting note if a fallback was needed.

- [ ] **Step 3: Provision via `link` or `source`**

`link` is fastest if you have a local build: `cargo build --release -p zendriver-mcp` then put
it on PATH, run `/zendriver:setup link`. Otherwise `source`. (Prebuilt can't be tested until a
release exists — see Step 6.)

- [ ] **Step 4: Restart and confirm the MCP server starts**

New session via `claude --plugin-dir plugins/zendriver`; confirm no SessionStart nudge and
that the `mcp__plugin_zendriver_zendriver__*` tools are available (e.g. ask Claude to call
`browser_status`).

- [ ] **Step 5: Run a real scrape + extract**

```
/zendriver:scrape https://example.com title and first paragraph
/zendriver:extract https://news.ycombinator.com top 5 story titles as {titles: string[]}
```
Confirm: subagent dispatches, scraping skill auto-loads, extraction is surgical (not a 50k
DOM dump), `/extract` returns valid JSON, browser closes cleanly.

- [ ] **Step 6: Exercise the release workflow (no real tag needed yet)**

After the binary workflow is merged and at least one `zendriver-mcp-v*` release exists, run
`release-binaries.yml` via `workflow_dispatch` with that tag; confirm 5 assets + per-target
`.sha256` + combined `SHA256SUMS` appear on the release, then test
`/zendriver:setup prebuilt` end to end.

- [ ] **Step 7: Final gate + record results**

```bash
# Manifests + scripts (CI parity)
for f in .claude-plugin/marketplace.json plugins/zendriver/.claude-plugin/plugin.json plugins/zendriver/.mcp.json plugins/zendriver/hooks/hooks.json; do jq empty "$f"; done
shellcheck plugins/zendriver/scripts/setup.sh plugins/zendriver/hooks/session-start.sh
mdbook build docs/book
echo "ALL GREEN"
```
Note the smoke-test outcomes (which provision mode worked, whether path vars expanded in the
command) in the PR description.

---

## Out of scope / non-goals

- No new Rust public API and no new MCP tools — the plugin only consumes the existing surface.
  Therefore the **MCP coverage ledger, insta schema snapshots, and `public-api-baseline.txt`
  are untouched** (no `cargo insta`/baseline regeneration in this PR).
- No auto-open / auto-scrape automation (rejected "maximal automation" approach).
- No `browser_read` readability→markdown MCP tool (separate `zendriver-mcp` backlog item; the
  `scraping` skill's surgical-extraction guidance is the interim answer).

## Follow-up (separate PR, noted in spec)

- **Server-side shutdown hardening:** make `zendriver-mcp` handle SIGTERM / stdin EOF with a
  graceful `browser.close()` so an owned Chrome is never orphaned on a non-graceful kill.
- **Subagent tool-scoping:** once an MCP-server wildcard for the agent `tools:` field is
  confirmed, scope `zendriver-scraper` to the zendriver tools + `Skill` instead of inheriting
  all tools.

## Self-review

- **Spec coverage:** Section 1 (layout/distribution/binary) → Tasks 1–3, 13; Section 2 (skills)
  → Tasks 5–7; Section 3 (commands/subagent) → Tasks 8–11; Section 4 (hooks/responsible-use/
  docs/testing) → Tasks 4, 12, 14, 15, 16. Release workflow → Task 13. No uncovered section.
- **Placeholder scan:** every code/JSON/YAML/markdown artifact is given in full; checks have
  exact commands + expected output. The single deliberate runtime unknown (command-body var
  expansion) is isolated and has a coded fallback (Task 16 Step 2 / `setup.sh` default).
- **Type/name consistency:** binary path `${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp` is identical
  across `.mcp.json`, `setup.sh`, `session-start.sh`, and the setup command; plugin name
  `zendriver`, marketplace `zendriver-rs`, asset name `zendriver-mcp-<triple>.<ext>`, and tag
  glob `zendriver-mcp-v*` agree across `setup.sh`, `release-binaries.yml`, and the docs.
```
