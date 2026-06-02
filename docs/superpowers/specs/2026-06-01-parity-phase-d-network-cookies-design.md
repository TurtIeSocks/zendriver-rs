# Phase P-D — Network / Cookies / Storage Completeness (design)

Date: 2026-06-01
Status: design (delegate-mode brainstorm; awaiting user review)
Scope: close the remaining interception / cookie / download / event gaps vs zendriver-py + both Python libs. Builds on the existing `Rule` enum + `PausedRequest` (consume-once + Drop-safety), `Cookie`/`CdpCookie` mirror, whole-jar JSON persistence, the single-loop transport actor (broadcast bus, no reconnect), and `Connection::subscribe<T>(method) -> Stream`.

Items: D1 `Fetch.continueResponse` · D2 reconnection · D3 cookie set-richness (CHIPS) · D4 runtime download path + cookie filter · D5 `add_handler` design note.

Preserve: declarative Rule API + Stream escape hatch, PausedRequest Drop-safety, proxy-auth wiring, JSON (not pickle) cookie persistence, serde-`default` cookie crash-immunity, per-Tab download progress + `save_to`, single-socket flat transport.

---

## D1 — `Fetch.continueResponse` (modify response headers/status at Response stage)

The one concrete Fetch gap vs zendriver-py (`continue_response`). Today rs can `respond` (full synthetic) or `modify_and_continue` (request-side) but cannot tweak an upstream **response's** status/headers while keeping its body.

**Stream API** — new terminal on `PausedRequest` (Response stage only):
```rust
impl PausedRequest {
    pub async fn continue_response(
        self,
        status: Option<u16>,
        phrase: Option<String>,
        headers: Option<Vec<(String, String)>>,
    ) -> Result<(), InterceptionError>;
}
```
Dispatches `Fetch.continueResponse { requestId, responseCode?, responsePhrase?, responseHeaders? }`. Sets `released = true` like the other terminals. Returns `InterceptionError::WrongStage` if `self.response.is_none()` (i.e. called at the Request stage — Chrome rejects it; fail fast with a clear error rather than a raw CDP error). `responseHeaders` = **replace** semantics (CDP-faithful); rustdoc notes it replaces, not merges, and points at `body()` for read-then-decide.

**Declarative API** — new `Rule` variant mirroring `Modify`'s closure shape:
```rust
pub enum Rule {
    // … Block / Redirect / Respond / Modify …
    ModifyResponse {
        pattern: UrlPattern,
        modify: Arc<dyn Fn(&ResponseInfo) -> ResponseOverrides + Send + Sync>,
    },
}
pub struct ResponseOverrides { pub status: Option<u16>, pub phrase: Option<String>, pub headers: Option<Vec<(String,String)>> }
```
Only fires when the request is paused at the Response stage (`InterceptBuilder::at_response()`); the actor applies it via `Fetch.continueResponse`. A `ModifyResponse` rule matched at the Request stage is a no-op-with-`debug!` (the actor only has request data then) — documented.

Touch points: `paused.rs` (new terminal), `rule.rs` (variant + `matches` arm + Debug), `actor.rs` (Response-stage dispatch + `headers_to_cdp` reuse), `types.rs` (`ResponseOverrides`), `builder.rs` (rule registration), `error.rs` (`WrongStage`).

Tests: `continue_response(Some(204), None, Some(hdrs))` dispatches `Fetch.continueResponse` w/ those fields; calling it with `response: None` returns `WrongStage`; a `ModifyResponse` rule at Response stage drives `continue_response` through the actor.

---

## D2 — Reconnection / recovery

Today the actor exits permanently on ws death (drains `pending` with `SHUTDOWN_DRAIN_CODE`, terminates the loop). Both Python libs reconnect; zendriver-py also re-arms domains. **But** full transparent reconnect is hard in rs's handle model: re-attaching targets yields **new `sessionId`s**, invalidating every live `Tab`/`Frame`/`Element` handle (they cache the old session). Designing for full handle-preservation is its own project.

**Scoped v1 (recommended):**
1. **Typed disconnect.** Replace the generic shutdown surfaced to callers with `ZendriverError::Disconnected` (distinct from a clean `close()`), so a long-running consumer can tell "Chrome died / socket dropped" from "I closed it."
2. **Manual `Browser::reconnect()`.** Re-dial the browser ws (`/devtools/browser/<id>` survives as long as Chrome lives), restart the actor on the **same** `Connection` (same broadcast bus, so raw event subscribers re-attach automatically), re-run `Target.setAutoAttach{flatten:true}` (re-fires the observer chain ⇒ stealth re-injects on each target) + re-apply the P-A A4 `WebSocketConfig` (max_size). Refresh the `TabRegistrar`. **Documents that existing `Tab`/`Frame` handles are invalidated — callers must re-acquire via `main_tab()` / `tabs()`.**
3. **Optional `BrowserBuilder::auto_reconnect(RetryPolicy)`** — on ws death the actor loops `reconnect()` with backoff (max attempts / delay) before giving up and surfacing `Disconnected`. Off by default.

**Deferred:** transparent handle-preserving reconnect (session-id remap so live `Tab` handles keep working) + feature re-arm (Network.enable / Fetch rules / DOMStorage.enable that individual features turned on). Tracked as a follow-up; v1 covers the browser-connection + event-stream + stealth re-injection, which is the bulk of the "long scraper survives a blip" value.

Touch points: `transport/actor.rs` (loop: on close, either reconnect-in-place per policy or send `Disconnected`), `transport/connection.rs` (reconnect entry that swaps the ws but keeps `event_tx`/`pending` plumbing), `browser.rs` (`reconnect()` + `auto_reconnect` builder + registry refresh), `error.rs` (`Disconnected`).

Tests: ws drop with `auto_reconnect` off → in-flight calls get `Disconnected` (not the opaque shutdown code); `reconnect()` re-dials + re-sends `setAutoAttach`; with a retry policy, a simulated drop triggers a reconnect attempt (wiremock/mock ws).

---

## D3 — Cookie set-richness (CHIPS + priority)

rs `Cookie` models 9 fields; nodriver/zendriver-py carry the full CDP `CookieParam`. Partitioned cookies (CHIPS) and priority can't be set today. Additive fields on `Cookie` + `CdpCookie` (serde `default` keeps reads crash-immune + old JSON forward-compatible):
```rust
pub struct Cookie {
    // … existing 9 …
    #[serde(default, skip_serializing_if = "Option::is_none")] pub priority: Option<CookiePriority>,   // Network.CookiePriority
    #[serde(default, skip_serializing_if = "Option::is_none")] pub same_party: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub source_scheme: Option<CookieSourceScheme>, // Unset|NonSecure|Secure
    #[serde(default, skip_serializing_if = "Option::is_none")] pub source_port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub partition_key: Option<String>,      // CHIPS top-level site
}
pub enum CookiePriority { Low, Medium, High }
pub enum CookieSourceScheme { Unset, NonSecure, Secure }
```
Mirror in `CdpCookie` (camelCase: `priority`, `sameParty`, `sourceScheme`, `sourcePort`, `partitionKey`) + extend both `From` impls. On reads (`all`/`for_url`), CDP populates whatever it knows; unknown stays `None`. The serde-`default` discipline that made rs immune to the Chrome-146 `sameParty` crash is preserved (these are all optional).

Note: CDP `partitionKey` recently became an object (`{topLevelSite, hasCrossSiteAncestor}`) on some channels — model `partition_key` as `Option<String>` for the common top-level-site case and document the limitation; a structured variant can follow if needed.

Tests: setting a cookie with `priority: High` + `partition_key: Some(..)` puts `priority`/`partitionKey` on the `Storage.setCookies` wire payload; round-trip through JSON preserves the new fields; a read with the fields absent yields `None` (no crash).

---

## D4 — Runtime download path + cookie filter

**Runtime download path.** rs has `BrowserBuilder::downloads_dir` (browser-scope, launch-time — already ahead of Python) but no *runtime* setter; the `expect_download` flow forces a TempDir. Add:
```rust
impl Tab     { pub async fn set_download_path(&self, dir: impl Into<PathBuf>) -> Result<()>; }
impl Browser { pub async fn set_download_path(&self, dir: impl Into<PathBuf>) -> Result<()>; }
```
Dispatches `Browser.setDownloadBehavior { behavior: "allow", downloadPath: dir }` (plain `allow` → files keep their suggested names in the chosen dir; distinct from the `expect_download` coordinator's `allowAndName`+TempDir capture flow, which stays for the "await + save_to" use case). Tab-scope sets it for that tab's browser context; Browser-scope sets it browser-wide.

**Cookie save/load filter.** nodriver's save/load take a regex `pattern`; rs persistence is whole-jar. Add closure-predicate variants (more flexible + idiomatic than a regex string):
```rust
impl CookieJar {
    pub async fn save_to_file_matching(&self, path, filter: impl Fn(&Cookie) -> bool) -> Result<()>;
    pub async fn load_from_file_matching(&self, path, filter: impl Fn(&Cookie) -> bool) -> Result<()>;
}
```
`save_*` filters `all()` before writing; `load_*` filters the parsed `Vec<Cookie>` before `set_many`. Existing `save_to_file`/`load_from_file` stay (filter = accept-all).

Tests: `set_download_path` dispatches `Browser.setDownloadBehavior { behavior: "allow", downloadPath }`; `save_to_file_matching(|c| c.domain.contains("x.test"))` writes only matching cookies.

---

## D5 — `add_handler` (design note, mostly no-build)

nodriver/zendriver-py expose `add_handler(event, cb)` where `cb` takes `(event)` or `(event, tab)`. rs already has the **better** primitive: `Connection::subscribe<T>(method) -> Stream<T>` (+ `subscribe_raw`) — composable with `select!` / `StreamExt`, no callback-arity introspection, no blocking-handler footgun (zendriver had to make handlers async in 0.10.2 to avoid blocking its listen loop; rs sidesteps this entirely).

**Recommendation: keep Stream as the idiom.** Optionally add a thin ergonomic adapter for users porting callback code:
```rust
impl Connection {
    pub fn on_event<T>(&self, method: &str, cb: impl FnMut(T) + Send + 'static) -> SubscriptionGuard;
}
```
spawns a task draining `subscribe::<T>()` into `cb`; dropping the returned `SubscriptionGuard` cancels it. Low priority — Stream covers every case; the adapter is sugar. Document the Stream→callback mapping in the mdBook events chapter rather than chasing nodriver's exact `add_handler` signature.

Tests (only if the adapter ships): `on_event` invokes `cb` per matching event; dropping the guard stops delivery.

---

## Cross-cutting
- **Deps:** none new.
- **Feature gates:** D1 stays under `interception`; rest are core.
- **SEMVER:** D1/D3/D4/D5 additive (new `Rule` variant is additive; `Cookie` gains optional fields). D2 changes the disconnect error type (was opaque shutdown → typed `Disconnected`) — pre-1.0 fine, note in CHANGELOG Changed. The A4 `WebSocketConfig` must be re-applied inside D2 `reconnect()`.
- **Docs:** mdBook interception chapter gains `continue_response`/`ModifyResponse`; cookies chapter gains CHIPS/priority + filter; a new "reconnection" section with the handle-invalidation caveat; events chapter documents Stream-vs-add_handler.
- **Ordering:** D1 (isolated, high value) + D3 (additive fields) + D4 (small) first; D2 last (largest, riskiest). D5 is a doc + optional sugar.

## Out of scope (deferred)
- Transparent handle-preserving auto-reconnect + full feature re-arm (D2 follow-up).
- Structured CHIPS `partitionKey` object variant (string top-level-site first).
- `add_handler` exact `(event, tab)` arity parity (Stream is the rs idiom).
- SOCKS5 authed-proxy forwarder (SKIP — zendriver-py dropped it; Fetch.auth covers HTTP/HTTPS).
- `continue_response` body rewrite (CDP `continueResponse` can't change the body; use `respond` for a full synthetic — documented).

## Assumptions (delegate-mode checkpoint — correct any before writing-plans)
1. **D1 = both a `PausedRequest::continue_response` Stream terminal AND a closure-based `Rule::ModifyResponse`** (mirrors `Modify`); Response-stage only, `WrongStage` error otherwise; headers = replace (CDP-faithful).
2. **D2 ships the scoped v1** — typed `Disconnected` error + manual `Browser::reconnect()` (re-attach + re-observe + registry refresh, **handles invalidated, re-acquire via `main_tab()`/`tabs()`**) + optional `auto_reconnect(RetryPolicy)`. Transparent handle-preserving reconnect is **deferred**.
3. **D3 adds `priority`/`same_party`/`source_scheme`/`source_port`/`partition_key`** (all optional, serde-`default`); `partition_key` is a `String` top-level-site for now (structured object deferred).
4. **D4 runtime download path = `Tab`/`Browser::set_download_path(dir)`** via `setDownloadBehavior{behavior:"allow"}`, distinct from the `expect_download` capture flow; cookie filter = **closure predicate** (`save_to_file_matching`/`load_from_file_matching`), not a regex string.
5. **D5 keeps Stream as the idiom**; the `on_event` callback adapter is optional sugar, low priority.
6. D2's `reconnect()` re-applies the A4 `WebSocketConfig` (max_size).
