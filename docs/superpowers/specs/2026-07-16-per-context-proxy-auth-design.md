# Design: first-class per-context proxy authentication

**Date:** 2026-07-16
**Status:** approved (delegate-mode brainstorming; assumptions accepted)
**Crate(s):** `zendriver` (+ `zendriver-interception` reuse), `zendriver-mcp` (ledger only)
**Feature gate:** proxy is always available; auth requires `interception`.

## Problem

Per-context proxy *authentication* has no first-class API. Today:

- `Browser::create_browser_context_with(proxy_server, proxy_bypass_list)`
  ([`browser.rs:3156`](../../../crates/zendriver/src/browser.rs)) threads
  `proxyServer` / `proxyBypassList` into `Target.createBrowserContext`, but has
  **no credential slot**.
- Chrome's `Target.createBrowserContext` `proxyServer` **cannot carry auth** —
  userinfo in the URL is ignored. Proxy auth must go through the interception
  path: `Fetch.authRequired` → `Fetch.continueWithAuth`.
- So each context tab needs a manual
  `tab.intercept().handle_auth(user, pass).start()`, and the caller must **hold
  the returned `InterceptHandle` alive** or the actor silently tears down. This
  is per-tab boilerplate plus a hold-the-handle footgun (see the current
  `examples/browser_context_isolation.rs`, which hand-rolls `split_proxy` and
  juggles handles).
- `BrowserBuilder::proxy_auth` ([`browser.rs:608`](../../../crates/zendriver/src/browser.rs))
  is launch-time, browser-wide, and main-tab-only — not per-context.

**Hard constraint (zendriver#208, see [`browser.rs:2660`](../../../crates/zendriver/src/browser.rs)):**
auth (`handle_auth`) and tracker-blocking (`block_hosts`) must be chained into a
**single** `InterceptBuilder` per session. Two separate actors on the same
session double-resolve the same `Fetch.requestPaused`. Any per-context auth
mechanism must fold into the one-actor-per-session model.

## Goal

Bind proxy **and credentials** to a `BrowserContext` once; every tab it spawns
is transparently authenticated, with no per-tab boilerplate and no handle to
hold.

## API — builder

```rust
// Headline case: one string, done. Embedded userinfo is auto-split.
let ctx = browser
    .browser_context()
    .proxy("http://user:pass@host:port")
    .proxy_bypass("<-loopback>")
    .build().await?;

// Explicit creds (when they can't live in the URL):
let ctx = browser
    .browser_context()
    .proxy("http://host:port")
    .proxy_auth("user", "pass")
    .build().await?;
```

`Browser::browser_context()` returns a `BrowserContextBuilder`.

| Method | Feature gate | Behavior |
|--------|--------------|----------|
| `proxy(impl Into<String>)` | always | Upstream `scheme://[user:pass@]host:port`. If userinfo is present **and** no explicit `.proxy_auth()` was set, the userinfo becomes the auth creds and is **stripped** from the `proxyServer` sent to CDP (Chrome ignores it there anyway). |
| `proxy_auth(user, pass)` | `interception` | Explicit creds; override any embedded userinfo. |
| `proxy_bypass(impl Into<String>)` | always | Bypass list (`<-loopback>`, `*.internal.example.com`). |
| `build(self) -> Result<BrowserContext>` | always | Sends `Target.createBrowserContext { proxyServer(stripped), proxyBypassList }`, registers creds (if any), returns the `BrowserContext`. `async`. |

**Compatibility:** `Browser::create_browser_context()` (no-arg) is kept.
`Browser::create_browser_context_with(proxy, bypass)` is kept as a no-auth
convenience routed through the builder — the existing tests and example keep
compiling. (API churn is acceptable pre-release, but there is no reason to break
these gratuitously.)

## Core mechanism — credential storage + auto-install

1. `BrowserInner` gains
   `context_proxy_auth: tokio::sync::Mutex<HashMap<String /*contextId*/, (String, String) /*user,pass*/>>`.
2. `BrowserContextBuilder::build()` inserts `(user, pass)` under the new context
   id when auth is configured.
3. **Tab registrar** ([`browser.rs:1560`](../../../crates/zendriver/src/browser.rs)):
   the attach path already has `session.target_info.browser_context_id`
   (confirmed present: [`observer.rs:120`](../../../crates/zendriver-transport/src/observer.rs))
   and already builds a per-tab `InterceptBuilder` for tracker-blocking. Extend
   it so that, for a new page tab, it builds **one** `InterceptBuilder` chaining
   BOTH:
   - `block_hosts(matcher)` when tracker-blocking is configured (existing), and
   - `handle_auth(user, pass)` when `context_proxy_auth` has an entry for this
     tab's `browser_context_id` (new).

   This satisfies the #208 one-actor-per-session constraint. The handle is
   parked in the existing per-session handle map (`tracker_handles`, keyed by
   sessionId — consider renaming to `session_intercept_handles` for clarity),
   and is already removed on tab detach
   ([`browser.rs:1608`](../../../crates/zendriver/src/browser.rs)).
4. `BrowserContext::Drop`
   ([`browser_context.rs:94`](../../../crates/zendriver/src/browser_context.rs))
   also removes the context's `context_proxy_auth` entry. Its tabs' actors drop
   with the tabs.

### Actor-install decision table (per new page tab)

| tracker matcher | context auth entry | actor built |
|-----------------|--------------------|-------------|
| no | no | none (unchanged) |
| yes | no | `block_hosts` only (unchanged) |
| no | yes | `handle_auth` only |
| yes | yes | **one** actor: `block_hosts` + `handle_auth` chained |

## Feature gating

Proxy-without-auth needs no `interception`. `.proxy_auth()` and the
userinfo→auth split are `#[cfg(feature = "interception")]` (mirrors
`BrowserBuilder::proxy_auth`, which is already interception-gated at
[`browser.rs:608`](../../../crates/zendriver/src/browser.rs)).

Without the `interception` feature: `.proxy("http://user:pass@host")` still
strips userinfo so `proxyServer` is clean, but `build()` emits a `warn!` that
credentials were supplied while `interception` is off (auth is inactive). No
silent drop.

## Error handling

- `build()` bubbles transport errors from `Target.createBrowserContext` and the
  existing missing-`browserContextId` → `ZendriverError::Navigation`.
- Malformed `.proxy()` (unparseable / no host component — a syntactic check,
  no DNS resolution) → error at `build()`, reusing `ZendriverError::Navigation`
  (revisit if too coarse).
- Registrar auth-install is best-effort + `warn!` on failure, mirroring the
  existing fire-and-forget tracker install.

## Testing

Unit (`MockConnection`, no Chrome):

1. `build()` sends `Target.createBrowserContext` with a **stripped**
   `proxyServer` (userinfo removed) plus `proxyBypassList`.
2. Creds registered under the returned context id; omitted when no auth.
3. Registrar attach with a matching `browser_context_id` → a single
   `Fetch.enable { handleAuthRequests: true }`, and `Fetch.authRequired` →
   `Fetch.continueWithAuth` carrying the creds.
4. tracker + auth both set on the same session → **one** `Fetch.enable` / one
   actor (guards #208).
5. Explicit `.proxy_auth()` overrides embedded userinfo.
6. `BrowserContext` drop clears its `context_proxy_auth` entry.

Integration (real Chrome, `#[ignore]`, joins the nightly set): rewrite
`examples/browser_context_isolation.rs` onto the new API (deletes the manual
`split_proxy` + per-tab handle juggling); two contexts with distinct
creds/proxies show isolated exit IPs.

## MCP coverage

No MCP tool exposes browser contexts today (grep: zero hits). `BrowserContext`
is a handle-returning lifecycle API whose tabs are addressed through the
existing tab tools — it does not fit a request/response tool. Add a
`mcp-coverage-ledger.toml` **`excluded`** entry for the new public items
(`Browser::browser_context`, `BrowserContextBuilder` + its methods):

> `excluded = "BrowserContext is a handle-returning lifecycle API; per-context proxy+auth is configured at Rust construction time and its tabs are driven through existing tab tools — no request/response MCP surface. Agent-facing per-context proxies would be a separate browser_context_open tool (out of scope)."`

Run the public-api baseline regen + schema-snapshot steps per `CLAUDE.md` if the
public API diff flags the new items.

## Docs

- Rustdoc (`no_run`) on `browser_context()`, `BrowserContextBuilder`, and each
  method.
- mdBook `docs/book/src/browser-context.md`: replace the ":73 per-context auth
  on the roadmap" and ":173 No per-context auth yet" notes with the new API.
- README browser-context bullets (no MCP tool-count change — nothing new on the
  wire).
- Flip the `deferred-backlog.md` §1 per-context-proxy-auth entry to closed on
  ship.

## Out of scope / non-goals

- Agent-facing per-context proxy over MCP (separate `browser_context_open` tool).
- Fixing the launch-time `BrowserBuilder::proxy_auth` main-tab-only limitation —
  a separate backlog item; unchanged here.
- SOCKS5 forwarder, per-request proxy rotation.

## Assumptions (delegate-mode judgement calls)

1. **Builder over arg-growth** — `browser.browser_context()…build()` rather than
   more positional params on `create_browser_context_with`. Matches the codebase
   builder idiom and the standing "favor builder pattern" preference.
2. **Entry name `browser.browser_context()`** returning the builder (no context
   getter exists to clash with).
3. **Userinfo auto-split is the headline ergonomic** — `.proxy("http://user:pass@host")`
   alone wires both sides; `.proxy_auth()` overrides.
4. **One actor per session** (auth + tracker chained via the existing registrar
   path + per-session handle map), respecting #208 — never a second actor.
5. **`.proxy_auth()` is `interception`-gated**, mirroring `BrowserBuilder::proxy_auth`;
   without the feature → strip userinfo + `warn!`, auth inactive.
6. **`create_browser_context_with` kept** as a no-auth convenience routed through
   the builder (no gratuitous break of the existing tests/example).
7. **MCP: ledger `excluded`** — no agent-facing per-context proxy tool this round.
8. **Launch-time `BrowserBuilder::proxy_auth` untouched** (browser-wide/main-tab).
9. **Reuse `ZendriverError::Navigation`** for malformed/missing-host rather than a
   new error variant — revisit if too coarse.
