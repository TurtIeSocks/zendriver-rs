# Parity Phase P-C — Launch / Config / Browser Control — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Checkbox steps. Full detail in `docs/superpowers/specs/2026-06-01-parity-phase-c-launch-config-design.md`. TDD from spec + signatures.

**Goal:** Launch-time + browser-control parity — connect-to-existing, expert mode, extensions, lang/user-agent/sandbox/channel, grant_permissions.

**Architecture:** Core `zendriver` crate. `BrowserBuilder` field/method additions feeding `build_flags`; new `Browser::connect` lifecycle branch (`owns_process: bool`); `find_chrome_executable` channel tables; `Browser.grantPermissions` browser-scope.

**Tech Stack:** Rust 2024, tokio `Command`, CDP, `MockConnection`. Possible new dep `rustix` (C4 root detect, optional). `zip` already present (C3 crx).

**Order:** C5 → C4 → C1 → C3 → C2. C4/C1/C3/C2 all touch `browser.rs` — serialize.

---

### Task C5: grant_permissions — Spec §C5
**Files:** Modify `crates/zendriver/src/browser.rs` (+ `PermissionType` enum, maybe `src/permissions.rs`).
- [ ] Failing tests: `grant_permissions(&[PermissionType::Geolocation], Some("https://x"))` dispatches `Browser.grantPermissions{permissions:["geolocation"],origin:"https://x"}`; `reset_permissions()`→`Browser.resetPermissions`.
- [ ] Run → fail.
- [ ] Implement `PermissionType` enum mirroring CDP `Browser.PermissionType` (with `as_cdp()->&str`), `Browser::grant_permissions(&[PermissionType], Option<&str>)`, `grant_all_permissions()` (full list), `reset_permissions()`. Browser-scope dispatch.
- [ ] Run → pass.
- [ ] Commit: `feat(browser): grant_permissions / grant_all_permissions / reset_permissions`

### Task C4: lang / user_agent / sandbox / channel — Spec §C4
**Files:** Modify `crates/zendriver/src/browser.rs` (`BrowserBuilder` fields + methods + `build_flags` + `find_chrome_executable` + `Channel` enum).
- [ ] Failing tests: `lang("en-US")`→flags contain `--lang=en-US`; `user_agent("X")`→`--user-agent=X`; `sandbox(false)`→`--no-sandbox`; `channel(Channel::Brave)` resolves a Brave path (mock probe) or clean error if absent. Existing `build_flags` tests stay green.
- [ ] Run → fail.
- [ ] Implement builder methods `lang`, `user_agent`, `sandbox(bool)` (default on; off→`--no-sandbox`; keep CI auto-add), `channel(Channel)`; `enum Channel{Chrome,Chromium,Brave,Edge,Auto}`; extend `find_chrome_executable` with per-channel + per-OS path tables (`.executable()` still overrides). Optional: root-uid auto-no-sandbox via `rustix::process::geteuid()` (add dep) — gate behind a small helper; if skipping, leave explicit `sandbox(false)`.
- [ ] Run → pass.
- [ ] Commit: `feat(browser): lang/user_agent/sandbox builder flags + Channel (Brave/Edge) detection`

### Task C1: connect-to-existing Chrome — Spec §C1
**Files:** Modify `crates/zendriver/src/browser.rs` (`BrowserBuilder::connect`, `Browser::connect`, `owns_process` field + `close()`/`Drop` branch). Possibly a small `/json/version` HTTP resolve helper.
- [ ] Failing tests: `BrowserBuilder::connect("ws://127.0.0.1:9222/devtools/browser/x")` calls `connect_with_observers` and does NOT spawn a Command; a connected `Browser` has `owns_process==false` and `close()` does not attempt process kill (assert via the branch / no child). HTTP-resolve: wiremock `/json/version` → ws url (gate or minimal request).
- [ ] Run → fail.
- [ ] Implement: `Browser` gains `owns_process: bool` (true via `launch`, false via `connect`); `close()`/`Drop` skip kill when false. `BrowserBuilder::connect(endpoint)` — if `ws://`/`wss://` use directly; if `http(s)://host:port` GET `/json/version` → `webSocketDebuggerUrl`. Skip Command/TempDir; reuse `connect_with_observers(ws_url, observers)` so `.stealth()` applies to new targets. `Browser::connect(endpoint)` shortcut. rustdoc: spawn-only fields ignored; pre-existing tabs predate stealth.
- [ ] Run → pass.
- [ ] Commit: `feat(browser): connect to a running Chrome debug session (ws/http endpoint)`

### Task C3: extensions — Spec §C3
**Files:** Modify `crates/zendriver/src/browser.rs` (`add_extension`/`extensions` + `build_flags` merge; crx unzip via `zip`).
- [ ] Failing tests: two ext dirs → flags contain `--load-extension=a,b` + `--disable-extensions-except=a,b` + `--enable-unsafe-extension-debugging`; with stealth `Off` + extensions, flags still carry `DisableLoadExtensionCommandLineSwitch` in `--disable-features`.
- [ ] Run → fail.
- [ ] Implement: `BrowserBuilder::add_extension(path)`/`extensions(paths)`; in `build_flags`, when extensions present add the load/disable-except/debugging flags + ensure `DisableLoadExtensionCommandLineSwitch` merged into `--disable-features` regardless of stealth profile. `.crx` → unzip to a `TempDir` (held by `Browser`) using existing `zip` dep (dirs accepted as-is).
- [ ] Run → pass.
- [ ] Commit: `feat(browser): load unpacked/crx extensions (Chrome 136+ workaround)`

### Task C2: expert mode + force-open-shadow-roots — Spec §C2
**Files:** Modify `crates/zendriver/src/browser.rs` (`expert`/`force_open_shadow_roots` + flags); add a small observer or script for the shadow hook (e.g. `crates/zendriver-stealth/src/patches/force_open_shadow.js` + a gated injector, OR a `zendriver`-side TargetObserver).
- [ ] Failing tests: `expert(true)` → `build_flags` contains `--disable-web-security` + `--disable-site-isolation-trials`; `force_open_shadow_roots(true)` → an `addScriptToEvaluateOnNewDocument` carrying `attachShadow` override dispatched on target attach.
- [ ] Run → fail.
- [ ] Implement: `BrowserBuilder::expert(bool)` (append the two flags), `force_open_shadow_roots(bool)` (inject `Element.prototype.attachShadow` → force `{mode:"open"}` via its own `addScriptToEvaluateOnNewDocument`, independent of the stealth bundle, doc-warned detectable).
- [ ] Run → pass.
- [ ] Commit: `feat(browser): expert mode flags + opt-in force_open_shadow_roots`

---
## Phase verification
Parallel: `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test -p zendriver`. Background if >5s.
## Self-review
Spec C1-C5 ✓. C2 split into two opt-ins (Assumption 2). C4 root-detect optional (Assumption 4). C1 highest-value; SOCKS5 out of scope. If `rustix` dep undesired at impl time, ship explicit `sandbox(false)` only and note.
