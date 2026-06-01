# Phase P-C — Launch / Config / Browser Control (design)

Date: 2026-06-01
Status: design (delegate-mode brainstorm; awaiting user review)
Scope: launch-time + browser-control gaps vs nodriver / zendriver-py. Builds on `BrowserBuilder` (`headless/executable/user_data_dir/proxy_auth/downloads_dir/arg/args/stealth/observer` → `launch(self)`), `build_flags(&self, user_data_dir)`, stderr `parse_devtools_line` → `connect_with_observers`, and the stealth `flags_for_profile` + `bootstrap_script` (9-patch IIFE via `Page.addScriptToEvaluateOnNewDocument`).

Items: C1 connect-to-existing · C2 expert mode + open-shadow hook · C3 extensions · C4 lang/user-agent/sandbox/channel · C5 grant_permissions.

Preserve: stealth tier model, `port=0`+stderr-parse (no free-port race), `kill_on_drop`, CHROME_BIN, fetcher, proxy-auth wiring.

---

## C1 — Connect to an already-running Chrome

rs always spawns. nodriver/zendriver-py can attach to a running debug session (host+port). Add a non-spawning terminal on the builder.

```rust
impl BrowserBuilder {
    pub async fn connect(self, endpoint: impl Into<String>) -> Result<Browser, ZendriverError>;
}
impl Browser {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Browser, ZendriverError>; // = builder().connect()
}
```
- `endpoint` accepts a **`ws://…/devtools/browser/<id>`** URL (used directly) **or** an **`http://host:port`** base (resolve `GET /json/version` → `webSocketDebuggerUrl`, like nodriver). Detect by scheme.
- Reuses `connect_with_observers(ws_url, observers)` — so `.stealth(profile)` / `.observer(..)` STILL apply: the stealth observer installs on **newly-attached** targets via the existing browser-wide `setAutoAttach{flatten:true}`. Already-open tabs predate the observer (documented: stealth applies to tabs opened after connect).
- **Ownership:** connected `Browser` sets `owns_process = false` → `close()` shuts the `Connection` but does **NOT** kill Chrome; `Drop` likewise detaches, no `kill_on_drop`. Spawn-only builder fields (`executable`, `user_data_dir`, flags, `downloads_dir`) are ignored on the connect path — rustdoc says so explicitly.

Touch points: `Browser` gains `owns_process: bool` (true for `launch`, false for `connect`); `close()`/`Drop` branch on it; new `connect` path skips Command/TempDir, optionally does the `/json/version` HTTP resolve (reuse `reqwest`, already a dep behind `fetcher` — gate the HTTP-resolve helper or use a minimal raw request; ws:// path needs no HTTP).

Tests: ws:// endpoint → `connect_with_observers` called, no spawn; `close()` on a connected browser does not attempt process kill (assert via the `owns_process` branch). HTTP-resolve covered by a wiremock `/json/version`.

---

## C2 — Expert mode + force-open-shadow-roots

nodriver `start(expert=True)` and zendriver-py both relax site isolation; zendriver-py also adds `--disable-web-security`. Separately, both force `Element.prototype.attachShadow` to open mode so closed shadow roots become walkable — **but this is itself detectable** (nodriver disables `cf_verify` under expert).

**Split into two orthogonal opt-ins** (cleaner + more honest than nodriver's bundling):
```rust
impl BrowserBuilder {
    pub fn expert(mut self, on: bool) -> Self;                  // launch flags
    pub fn force_open_shadow_roots(mut self, on: bool) -> Self; // JS hook (detectable)
}
```
- `expert(true)` → append `--disable-web-security` + `--disable-site-isolation-trials` in `build_flags`. Flags only.
- `force_open_shadow_roots(true)` → inject an extra `Page.addScriptToEvaluateOnNewDocument` carrying `Element.prototype.attachShadow` override forcing `{mode:"open"}`. Independent of the stealth bundle (works even with stealth Off; does NOT pollute the Spoofed shim). rustdoc **warns it is detectable** — recommend only when you must walk closed roots (e.g. some challenge widgets). Wired as a small built-in `TargetObserver` (or appended to the stealth observer's script set) so it runs on every new target.

Why split: the flags are debugging-grade and low-risk; the shadow hook trades stealth for reach. Bundling them (nodriver) forces users to accept detectability to get web-security-off. Assumption flags the divergence.

Tests: `expert(true)` → `build_flags` contains both flags; `force_open_shadow_roots(true)` → an `addScriptToEvaluateOnNewDocument` with `attachShadow` is dispatched on target attach.

---

## C3 — Extensions

Neither rs feature exists. zendriver-py/nodriver load unpacked extensions + work around Chrome 136+.

```rust
impl BrowserBuilder {
    pub fn add_extension(mut self, path: impl Into<PathBuf>) -> Self;
    pub fn extensions(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self;
}
```
At launch, when extensions are present, `build_flags` adds:
- `--load-extension=<dir1,dir2,…>`
- `--disable-extensions-except=<dir1,dir2,…>`
- `--enable-unsafe-extension-debugging` (nodriver; needed on recent Chrome)
- ensure `DisableLoadExtensionCommandLineSwitch` is in the `--disable-features` list **regardless of stealth profile** (today it only rides in `shared_stealth_flags`; Off profile would silently fail to load extensions). Merge it into the base disable-features when extensions are set.

**Input format:** unpacked **directory** paths (what `--load-extension` requires). A `.crx` path is auto-unzipped to a temp dir using the **existing `zip` dep** (already used by `fetcher`); the temp dir lives as long as the `Browser`. (crx-unzip is the one non-trivial bit; can ship dirs-first and add crx in a follow-up — see Assumptions.)

Tests: two dirs → flags contain `--load-extension=a,b` + `--disable-extensions-except=a,b` + the debugging flag; Off profile + extensions still carries `DisableLoadExtensionCommandLineSwitch`.

---

## C4 — lang / user-agent / sandbox / channel

Small builder fields feeding `build_flags` + executable discovery.

```rust
impl BrowserBuilder {
    pub fn lang(mut self, lang: impl Into<String>) -> Self;        // --lang=
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self;    // --user-agent= (launch-time, static)
    pub fn sandbox(mut self, on: bool) -> Self;                    // off → --no-sandbox
    pub fn channel(mut self, channel: Channel) -> Self;            // Chrome|Chromium|Brave|Edge|Auto
}
pub enum Channel { Chrome, Chromium, Brave, Edge, Auto }
```
- **`lang`** → `--lang=<v>` flag (zendriver-py kept it; today rs users must `.arg("--lang=…")`).
- **`user_agent`** → `--user-agent=<v>` launch flag. NOTE the three UA paths now exist: this static launch flag (C4), the runtime `Tab::set_user_agent` (P-B B4), and the stealth-profile UA (UA-CH coherent). rustdoc cross-references; for stealth use the profile.
- **`sandbox(false)`** → `--no-sandbox`. Default sandbox-on. Keep the existing CI auto-add. **Auto-no-sandbox-as-root (posix uid 0):** both Python libs do this; rs only checks `CI`. Add a uid==0 check — needs a `geteuid` syscall (no std API). Options: a tiny `rustix` dep (`rustix::process::geteuid()`), or `unsafe { libc::geteuid() }`. Proposed: explicit `sandbox(false)` is the primary control; auto-root-detect is a nice-to-have gated on adding `rustix` (preferred, no `unsafe`). See Assumptions.
- **`channel`** → extend `find_chrome_executable` with per-channel path tables (Brave: `…/Brave-Browser/...`, Edge: `…/Microsoft Edge/...`, etc., per-OS). `Auto` = current behavior (first found). `.executable(path)` still overrides everything.

Tests: `lang("en-US")` → flag present; `sandbox(false)` → `--no-sandbox`; `channel(Brave)` resolves a Brave path (mock the path probe) or errors cleanly when absent.

---

## C5 — grant_permissions

Absent in rs; both Python libs have `grant_all_permissions`. Browser-domain command (browser-scope dispatch, like `Tab::activate`).

```rust
impl Browser {
    pub async fn grant_permissions(&self, perms: &[PermissionType], origin: Option<&str>) -> Result<()>;
    pub async fn grant_all_permissions(&self) -> Result<()>;   // the full PermissionType list
    pub async fn reset_permissions(&self) -> Result<()>;       // Browser.resetPermissions
}
pub enum PermissionType { Geolocation, Notifications, Camera, Microphone, ClipboardReadWrite, /* … full CDP set */ }
```
Maps `Browser.grantPermissions { permissions, origin? }` / `Browser.resetPermissions`. `PermissionType` mirrors the CDP `Browser.PermissionType` enum; `grant_all_permissions` sends the complete list (nodriver semantics). `origin = None` grants browser-wide.

Tests: `grant_permissions(&[Geolocation], Some("https://x"))` dispatches `Browser.grantPermissions` with the mapped strings + origin; `reset_permissions` dispatches `Browser.resetPermissions`.

---

## Cross-cutting
- **Deps:** possibly +`rustix` (C4 root-detect, optional); `zip` already present (C3 crx). `reqwest` already present behind `fetcher` (C1 http-resolve) — keep the HTTP-resolve helper feature-gated or do a minimal request so the bare `connect(ws://…)` needs no new gate.
- **Feature gates:** none new for the core builder methods.
- **SEMVER:** all additive (new builder methods + Browser methods + `owns_process` internal). Pre-1.0. CHANGELOG: Added.
- **Docs:** mdBook gets a "connect to existing Chrome", "expert mode", and "loading extensions" section.
- **Ordering:** C4 (small flag fields) + C5 (isolated) first; C1 (lifecycle branch) + C3 (flag merge + crx) next; C2 (observer + flags) last. C1 is the highest-value gap.

## Out of scope (deferred)
- SOCKS5 authenticated-proxy forwarder (zendriver-py dropped it too; rs Fetch.auth covers HTTP/HTTPS — see P-D / SKIP list).
- `create_from_undetected_chromedriver` (Python-specific).
- Config-as-reusable-template object (builder is idiomatic).
- Per-context proxy auth inheritance for new tabs (separate; documented limitation).

## Assumptions (delegate-mode checkpoint — correct any before writing-plans)
1. **C1 connect = `BrowserBuilder::connect(endpoint)` + `Browser::connect` shortcut**; accepts `ws://` (direct) or `http://host:port` (resolve via `/json/version`); connected browser **never kills the process** (`owns_process=false`); spawn-only builder fields ignored + doc-warned. `.stealth()` still applies to tabs opened after connect.
2. **C2 split into `expert(bool)` (flags only) + `force_open_shadow_roots(bool)` (detectable JS hook)** rather than nodriver's bundled `expert`. Shadow hook independent of stealth bundle, doc-warned detectable.
3. **C3 extensions take unpacked dirs; `.crx` auto-unzipped via existing `zip` dep** (or dirs-first + crx follow-up). Extension flags (`--load-extension`/`--disable-extensions-except`/`--enable-unsafe-extension-debugging` + `DisableLoadExtensionCommandLineSwitch`) added regardless of stealth profile.
4. **C4: explicit `sandbox(false)` is the primary no-sandbox control + keep CI auto-add; auto-root-detect (uid 0) is optional, preferring a tiny `rustix` dep over `unsafe libc`.**
5. **C4 Brave/Edge via a `Channel` enum + `channel()` builder** extending `find_chrome_executable`; `.executable()` still overrides.
6. **C4 ships a static `--user-agent` launch flag** despite overlap with B4 runtime override + stealth UA; cross-referenced in docs.
7. **C5 `grant_permissions(perms, origin)` + `grant_all_permissions()` + `reset_permissions()`** with a `PermissionType` enum mirroring CDP; browser-scope dispatch.
8. **C1 is highest-value; SOCKS5 forwarder stays out of scope** (matches zendriver-py's own drop).
