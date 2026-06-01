# Parity Phase P-D — Network / Cookies / Storage Completeness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Checkbox steps. Full detail in `docs/superpowers/specs/2026-06-01-parity-phase-d-network-cookies-design.md`. TDD from spec + signatures.

**Goal:** Close interception/cookie/download/event gaps — `Fetch.continueResponse`, reconnection v1, CHIPS+priority cookies, runtime download path + cookie filter, event-stream sugar.

**Architecture:** `zendriver-interception` (D1), `zendriver-transport` + `zendriver` (D2), `zendriver` cookies/download (D3/D4), `zendriver-transport` (D5). Preserve PausedRequest Drop-safety, Stream API, JSON cookie persistence, serde-default crash-immunity.

**Tech Stack:** Rust 2024, CDP Fetch/Network/Storage/Browser domains, `MockConnection`.

**Order:** D1 → D3 → D4 → D2 → D5. D2 largest/riskiest; D5 optional.

---

### Task D1: Fetch.continueResponse — Spec §D1
**Files:** Modify `crates/zendriver-interception/src/paused.rs` (`continue_response`), `rule.rs` (`ModifyResponse` variant + `matches` + Debug), `types.rs` (`ResponseOverrides`), `actor.rs` (Response-stage dispatch), `builder.rs` (register), `error.rs` (`WrongStage`).
- [ ] Failing tests: `continue_response(Some(204),None,Some(hdrs))` on a Response-stage `PausedRequest` dispatches `Fetch.continueResponse{requestId,responseCode:204,responseHeaders:[…]}`; calling with `response:None` returns `WrongStage`; a `Rule::ModifyResponse` at Response stage drives `continue_response` via actor.
- [ ] Run → fail.
- [ ] Implement `PausedRequest::continue_response(self, status:Option<u16>, phrase:Option<String>, headers:Option<Vec<(String,String)>>)` (sets `released`, `WrongStage` if `response.is_none()`, reuse `headers_to_cdp`); `Rule::ModifyResponse{pattern, modify:Arc<dyn Fn(&ResponseInfo)->ResponseOverrides + Send+Sync>}` + matches arm + hand-written Debug; `ResponseOverrides{status,phrase,headers}`; actor applies it at Response stage (no-op+debug at Request stage); `InterceptionError::WrongStage`.
- [ ] Run → pass. `cargo test -p zendriver-interception`
- [ ] Commit: `feat(interception): Fetch.continueResponse — continue_response + Rule::ModifyResponse`

### Task D3: cookie set-richness (CHIPS + priority) — Spec §D3
**Files:** Modify `crates/zendriver/src/cookies/mod.rs` (`Cookie` + `CdpCookie` fields + `From` impls + `CookiePriority`/`CookieSourceScheme` enums).
- [ ] Failing tests: setting a cookie with `priority:High` + `partition_key:Some("https://top")` puts `priority`/`partitionKey` on the `Storage.setCookies` wire payload; JSON round-trip preserves new fields; a read missing the fields yields `None` (no panic).
- [ ] Run → fail.
- [ ] Implement: add `priority:Option<CookiePriority>`, `same_party:Option<bool>`, `source_scheme:Option<CookieSourceScheme>`, `source_port:Option<i32>`, `partition_key:Option<String>` to `Cookie` (snake_case, serde default+skip_if_none) and `CdpCookie` (camelCase: priority/sameParty/sourceScheme/sourcePort/partitionKey); extend both `From` impls. Enums `CookiePriority{Low,Medium,High}`, `CookieSourceScheme{Unset,NonSecure,Secure}` (serde to CDP strings).
- [ ] Run → pass. `cargo test -p zendriver cookies`
- [ ] Commit: `feat(cookies): CHIPS partitionKey + priority/sameParty/sourceScheme/sourcePort`

### Task D4: runtime download path + cookie filter — Spec §D4
**Files:** Modify `crates/zendriver/src/tab.rs` + `browser.rs` (`set_download_path`); `crates/zendriver/src/cookies/persistence.rs` (filtered save/load).
- [ ] Failing tests: `Tab::set_download_path(dir)` dispatches `Browser.setDownloadBehavior{behavior:"allow",downloadPath:dir}`; `save_to_file_matching(path,|c| c.domain.contains("x.test"))` writes only matching cookies; `load_from_file_matching` filters before set.
- [ ] Run → fail.
- [ ] Implement `Tab::set_download_path(dir)` + `Browser::set_download_path(dir)` (`Browser.setDownloadBehavior` behavior:"allow"); `CookieJar::save_to_file_matching(path, filter: impl Fn(&Cookie)->bool)` + `load_from_file_matching`. Existing `save_to_file`/`load_from_file` unchanged.
- [ ] Run → pass.
- [ ] Commit: `feat(tab,cookies): runtime set_download_path + filtered cookie save/load`

### Task D2: reconnection v1 — Spec §D2
**Files:** Modify `crates/zendriver-transport/src/actor.rs` + `connection.rs` (reconnect entry; typed disconnect), `crates/zendriver/src/browser.rs` (`reconnect()`, `auto_reconnect` builder, registry refresh), `error.rs` (`Disconnected`).
- [ ] Failing tests: with `auto_reconnect` off, a ws drop surfaces `ZendriverError::Disconnected` (distinct from clean shutdown) to in-flight callers; `Browser::reconnect()` re-dials + re-sends `Target.setAutoAttach{flatten:true}` (mock ws); with a `RetryPolicy`, a simulated drop triggers a reconnect attempt.
- [ ] Run → fail.
- [ ] Implement scoped v1 per spec: typed `Disconnected` (transport surfaces a distinct code/variant vs clean close; map to `ZendriverError::Disconnected`); `Browser::reconnect()` re-dials browser ws (reuse C1's connect path), restarts actor on same `Connection`/broadcast bus, re-runs `setAutoAttach{flatten:true}` (re-fires observers ⇒ stealth re-inject), re-applies A4 `WebSocketConfig`, refreshes `TabRegistrar`; `BrowserBuilder::auto_reconnect(RetryPolicy{max_attempts,delay})` loops reconnect on drop. rustdoc: existing Tab/Frame handles invalidated → re-acquire via `main_tab()`/`tabs()`. **Defer** transparent handle-preserving reconnect.
- [ ] Run → pass. `cargo test -p zendriver-transport -p zendriver`
- [ ] Commit: `feat(transport,browser): reconnection v1 — typed Disconnected + Browser::reconnect + auto_reconnect`

### Task D5: on_event adapter (OPTIONAL, low-pri) — Spec §D5
**Files:** Modify `crates/zendriver-transport/src/connection.rs` (`on_event` + `SubscriptionGuard`).
- [ ] Failing tests: `on_event::<T>(method, cb)` invokes `cb` per matching event; dropping the returned `SubscriptionGuard` stops delivery.
- [ ] Run → fail.
- [ ] Implement `on_event<T>(&self, method:&str, cb: impl FnMut(T)+Send+'static) -> SubscriptionGuard` (spawn task draining `subscribe::<T>()`; guard drop → abort). Document Stream as the primary idiom.
- [ ] Run → pass.
- [ ] Commit: `feat(transport): on_event callback adapter over subscribe() Stream`

> D5 is optional sugar — skip if budget tight; Stream API already covers all cases. Note skip in phase summary if dropped.

---
## Phase verification
Parallel: `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. Background if >5s.
## Self-review
Spec D1-D5 ✓. D2 = scoped v1 (Assumption 2); transparent reconnect deferred. D3 fields optional/serde-default. D4 distinct from expect_download capture. D5 optional. D2 `reconnect()` re-applies A4 config (cross-phase dep — A4 must land first; it does, in P-A).
