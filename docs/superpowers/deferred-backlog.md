# zendriver-rs — Deferred / Never-Got-Around-To Backlog

> **Snapshot:** 2026-06-03. **Re-verified:** 2026-07-16 (every §1–§4 item re-checked against current shipped code by a per-item verification sweep; statuses below reflect the 2026-07-16 pass). Re-verify again before acting — a file/line/flag named here may move.
>
> **Method (to regenerate):** grep specs/plans for deferral language (`deferred`, `out of scope`, `later`, `follow-up`, `future`, `not yet`, `for now`, `Option B`, `post-1.0`, `TODO`, unchecked `- [ ]` that aren't TDD steps) + code for `TODO`/`FIXME`/`todo!()`/`#[ignore]`/`#[allow(dead_code)]`/prose deferrals + the MCP ledger `excluded` entries. Cross-check each against shipped code.
>
> **Big picture:** the 6-phase port + parity A–E + fingerprints PR2 + MCP all shipped; most "deferred to phase N" items were since built. The genuine remaining tail is small and concentrated in stealth depth + a few convenience/limitation tails. The one item that is a *live failure* (not a niceties-deferral) is the fingerprint-pool asset URL — see §0.

Legend: 🐞 shipped-but-broken · 🔧 actionable tail · 🎯 intentional non-goal (recorded decision — *not* a gap) · 🧹 cleanup / stale-doc / code-residue · ✅ closed since snapshot

---

## §0 — 🐞 Live failure (fix first)

- **Fingerprint pool asset URL is still a placeholder** → real-device pool sampling **errors at runtime**, it is not merely deferred. `TODO(#25)` above the placeholder `POOL_URL` at `crates/zendriver-mcp/src/tools/fingerprints.rs:42` (lines 83-90 confirm the asset "does not exist yet" and fails). Blocked on publishing the fingerprint-pool release asset. Until then the generative path is the only working persona source.

---

## §1 — 🔧 Genuine tail (actionable; "never got around to it")

### Cheap win — blocker cleared since snapshot
- **`visible_only` find filter is still a NO-OP** — `crates/zendriver/src/query/mod.rs:905` & `:1276-1282` (`let _ = self.visible_only; let filtered = candidates;`). **But the old blocker is gone:** `actionability::check_visible` now *exists* (`crates/zendriver/src/query/actionability.rs:72-89`, already wired into `Element::is_visible` at `reads.rs:312`). It was simply never connected to `visible_only`. Small connect job now, not the big lift the stale `TODO(T16)` comment implies.

### Stealth / anti-detection
- **DataDome native slider/puzzle solving** (image-diff + Bézier drag). CAPTCHA surface is callback-only. — `crates/zendriver-datadome/src/bypass.rs:154-162`, `lib.rs:7`
- **DataDome `cookie_domain` public-suffix-list.** Naive last-2-labels heuristic; `co.uk` fails (test documents it). No `publicsuffix`/`psl` dep. — `crates/zendriver-datadome/src/captcha.rs:147-166,184`
- **WebGPU full-adapter fabrication when the host has no GPU.** Patch returns early if `GPUAdapter` undefined; only decorates a real adapter's `.info`. — `crates/zendriver-stealth/src/patches/webgpu.js:23-29`
- **Sec-CH-UA for non-Chrome browsers** (Brave/Edge/Vivaldi); "post-1.0". `UserAgentMetadata::realistic()` hardcodes Chrome branding only; `Channel::Brave/Edge` picks the executable, not the brands. — `crates/zendriver-stealth/src/fingerprint.rs:35-62`

### Fingerprints
- (pool asset URL moved to §0 — it's a live failure, not a nicety)
- **Mobile personas** (desktop-only). `Platform` enum = Win32/MacIntel/LinuxX86_64 only; generative sampler filters out mobile UAs; `mobile` hardcoded false. — `crates/zendriver-stealth/src/profile.rs:24-28`, `crates/zendriver-fingerprints/src/generative/mapping.rs:23-38`
- **Auto-refreshing pool on a schedule** + `force_refresh` / cache-TTL knob. Pure download-on-first-use with permanent cache; any cache hit short-circuits. — `crates/zendriver-fingerprints/src/pool/mod.rs:76-101`, `generative/download.rs:17-34`
- **Full attribute-coverage expansion** (screen metrics, navigator extras) + matching Persona/JS-patch growth. `persona_from_assignment` maps only platform/deviceMemory/hardwareConcurrency/videoCard/fonts; `Persona` has no screen-metric/navigator-extra fields. (Commit `402a427c` added *static* screen.js/mouse.js patches — coherent surface — but did **not** grow the BN or Persona.) — `crates/zendriver-fingerprints/src/generative/mapping.rs:41-83`, `crates/zendriver-stealth/src/persona/mod.rs:36-61`

### Geo / locale
- **timezone-from-geo derivation** (country → IANA tz, sharing the `geo` module). `geo::persona()` derives locale+languages only, leaves timezone `None`. — `crates/zendriver-stealth/src/geo/mod.rs:34-50`
- **Auto IP-geo resolution / Option B exit-IP probe.** Only the empty `GeoResolver` seam ships; zero `impl`; `geo_locale` still needs an explicit country. — `crates/zendriver-stealth/src/geo/mod.rs:52-62`, `browser.rs:1012`

### Network / cookies / tabs / frames
- **Transparent handle-preserving reconnect** (session-id remap so live `Tab` handles survive) + feature re-arm. `reconnect` clears the tab map (new sessionIds → invalidated handles). — `crates/zendriver/src/browser.rs:3479` (doc `:3436-3438`)
- **`partition_key` as a structured object** (currently flat `Option<String>` top-level-site). — `crates/zendriver/src/cookies/mod.rs:154-159`
- **Streaming response bodies** (monitor + HTTP `request()`); whole-body only, needs a Fetch-interception path. — `crates/zendriver/src/monitor/mod.rs:188`, `request.rs:346-380`
- **Frame OOPIF placeholder** — *resolved, see §-closed* (backlog previously mis-cited a test fixture).

### Elements / input
- **Button-triggered file pickers** via `Page.fileChooserOpened`; today `upload_files` uses `DOM.setFileInputFiles` on direct `<input type=file>` only. — `crates/zendriver/src/element/actions.rs:585,600`
- **Case-insensitive matchers** (`[a="v" i]`, lowercased text compare). No CI flag/variant; no CI matcher in the public API. — `crates/zendriver/src/query/predicate.rs:53-66,85-86`
- **Custom mouse pressure / pen / touch input.** Mouse dispatch carries no pressure/pointerType; no `Input.dispatchTouchEvent` anywhere. — `crates/zendriver/src/input/mouse.rs:152-175`

### Launch / context / fetcher
- **Fetcher Beta/Dev/Canary channels not wired** (return `UnsupportedPlatform`; only Stable/Latest). — `crates/zendriver-fetcher/src/resolver.rs:37-39`, `version.rs:13-18`

### MCP surface
- **Per-action `version` accessor not exposed.** `chrome_version` hardcoded `String::new()`; core lib has no version getter. — `crates/zendriver-mcp/src/tools/lifecycle.rs:146` (doc `:78-80`)
- **Intercept `method` / `post-data` reserved for follow-up.** `ModifyRequest` exposes only `headers`. — `crates/zendriver-mcp/src/tools/intercept.rs:29-30,87-95`
- **Tab-scoped expectations/rules leak** until manual cancel or full `browser_close`. `browser_tab_close` never drains `s.expectations`/`s.rules`; handles carry no `tab_id`. — `crates/zendriver-mcp/src/tools/tabs.rs:244,264-311`, `state.rs:150,166`

---

## §2 — 🎯 Intentional non-goals (recorded decisions — do not mistake for gaps)

- **Cross-tab drag-and-drop.** All drag is single-tab (`Tab::mouse_drag` on one session); Phase-4 spec lists it under Non-goals ("defer until proven needed"). — `crates/zendriver/src/tab.rs:2095`, `phase4-design.md:35`
- **Element-scoped find over MCP** (low demand). Ledger `excluded`; `Selector` exposes `frame_id` scoping only. — `mcp-coverage-ledger.toml:159,168`, `crates/zendriver-mcp/src/selectors.rs:61-93`
- **Interception `Stream` escape-hatch over MCP** (needs server-side event buffering + backpressure). MCP exposes declarative rule tools only; lib-side `subscribe` stays library-only. — `crates/zendriver-mcp/src/tools/intercept.rs`, `mcp-server-design.md:533`
- **bot.incolumitas behavioral-score / TCP-fingerprint / TLS-JA3 evasion** — needs proxy + TLS-stack control outside the workspace. — `phase2-stealth:30`
- **Active reese84 token synthesis** — belongs in a separate out-of-workspace `imperva-sensor-rs`. — `imperva:27`
- **Forging the DataDome device-check payload** (mint cookie out-of-browser) — extremely brittle. — `datadome:374`
- **Container GPU support** (Vulkan/GLES, Windows image) — belongs in `zendriver-docker`. — `datadome:375`
- **MCP `on_captcha` solver callback** — agents handle `CaptchaRequired` out-of-band. — `datadome:377`
- **SOCKS5 proxy forwarder** — matches zendriver-py's own drop. — `parity-C:138`
- **Real-device-data collection pipeline** — ship a curated asset, not a scraper. — `fingerprint:50`
- **OCR helpers** (tesseract/easyocr) — pair with an external crate. — `migration:196`
- **Widevine / DRM playback** — vanilla Chrome ships no CDM. — `migration:201`
- **Trace viewer / video recording** — integrate externally. — `migration-playwright:177`
- **Selenium-WebDriver drop-in API** — explicit non-goal since P1. — `phase5:31`
- **`browser.get(url)` shorthand / `find(text, best_match=True)` fuzzy match** — deliberate API split; use `main_tab()`+`goto` / `text_regex`. — `migration docs`

---

## §3 — 🧹 Cleanup / stale-doc / code-residue

- **Migration docs are stale.** Both `docs/book/src/migration-zendriver-python.md:186-194` and `migration-nodriver-python.md:235-244` still list canvas/WebGL/font/audio fingerprint spoofing + browserforge as "not yet" — **all shipped** (canvas.js/audio.js patches seeded in `patches.rs`; real-device pool in `zendriver-fingerprints`). Cheap doc fix.
- **~60 `#[allow(dead_code)]`** (non-test), **~35 still cite shipped task IDs** (e.g. "land with FindBuilder ext in T12" while `FindBuilder::one()/many()` are live). Audit: stale allows to remove, or genuinely-unused code. Concentrated in `crates/zendriver/src/query/selectors.rs`, `query/actionability.rs`, `input/*`, `element/mod.rs`.
- **43 `#[ignore]` integration tests** (grew from 38) need real Chrome / `--ignored`. **No CI job runs them:** the real-Chrome `test-integration` job runs `cargo nextest` with no `--run-ignored`, and no workflow passes `--ignored`/`--run-ignored`. They are silently rotting. — `.github/workflows/ci.yml:159`
- **`TODO(P5)` re-enable per-tab dialog-routing override.** Test `expect_dialog_resolves_on_alert` still `#[ignore]`. — `crates/zendriver/tests/integration_phase5.rs:215,221`
- **Imperva demo-site list `TODO`** (no stable public demo). — `crates/zendriver/tests/imperva_v0.rs`
- **Example placeholders:** `crates/zendriver/examples/cloudflare_bypass.rs:12-14` ("not yet clicked; single raw left-click"), `imperva_bypass.rs:5-6,40` (placeholder URL — set `IMPERVA_TEST_URL`).
- **`chromiumoxide_cdp` typed-Command migration: 0% done.** 28 raw `.call_raw()` sites / 561 `json!` params / 0 typed `.execute()`; grep for `chromiumoxide_cdp::` = 0 uses. The P1-deferred raw-JSON → typed-`Command` refactor was never started. — `crates/zendriver-transport/src/connection.rs:170`
- **Imperva fast-path interception integration test.** 🟡 A *unit* test now exists (`crates/zendriver-imperva/src/bypass.rs:847` — interception arm preempts the poll tick), but the nightly *integration* test (`crates/zendriver/tests/imperva_v0.rs:44-50`) still calls `wait_for_clearance()` with no `.with_interception()` — integration-level fast-path coverage remains absent.

---

## §-closed — ✅ Closed since the 2026-06-03 snapshot

- **Frame-session `Runtime.evaluate`** → **shipped** (commit `5440066b`). `QueryScope::session()` returns the frame's session; `execution_context_id()` pins eval to the frame's isolated-world context; test `in_frame_override_routes_dispatch_to_frame_session` asserts it. (Backlog's old `query/mod.rs:1873` citation is now just a comment inside that test.)
- **Nightly real-Chrome anti-detection CI** → **exists**. `nightly-stealth-tests` (cron `0 6 * * *`, real Chrome) runs `--test stealth_phase2`, which hits `bot.sannysoft.com` and `arh.antoinevastel.com/bots/areyouheadless`. — `.github/workflows/ci.yml:200-224`
- **OOPIF bootstrap "placeholder Frame"** → **was already done**; the backlog mis-cited test-fixture setup. Real impl: `crates/zendriver/src/frame/oopif.rs:49` `register_oopif_frame`, wired at `browser.rs:1497` for `kind=="iframe"` (commit `b13fdbd3`).
- **Per-context proxy auth — first-class API** → **shipped** via `Browser::browser_context()` → `BrowserContextBuilder` (`.proxy()`/`.proxy_bypass()`/`.proxy_auth()`/`.build()`), auto-installing per-tab `Fetch.authRequired` chained into the one per-session interception actor (2026-07-16 plan).

---

*Originally a session artifact during the obscura-comparison / stealth-features work (2026-06-03); re-verified 2026-07-16. A tracking snapshot, not a spec/plan. Update or delete as items close.*
