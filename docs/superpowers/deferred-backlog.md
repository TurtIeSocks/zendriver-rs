# zendriver-rs — Deferred / Never-Got-Around-To Backlog

> **Snapshot:** 2026-06-03. **Re-verified:** 2026-07-16 (every §1–§4 item re-checked against current shipped code by a per-item verification sweep; statuses below reflect the 2026-07-16 pass). Re-verify again before acting — a file/line/flag named here may move.
>
> **Method (to regenerate):** grep specs/plans for deferral language (`deferred`, `out of scope`, `later`, `follow-up`, `future`, `not yet`, `for now`, `Option B`, `post-1.0`, `TODO`, unchecked `- [ ]` that aren't TDD steps) + code for `TODO`/`FIXME`/`todo!()`/`#[ignore]`/`#[allow(dead_code)]`/prose deferrals + the MCP ledger `excluded` entries. Cross-check each against shipped code.
>
> **Big picture:** the 6-phase port + parity A–E + fingerprints PR2 + MCP all shipped; most "deferred to phase N" items were since built. The genuine remaining tail is small and concentrated in stealth depth + a few convenience/limitation tails. One item is parked on an external decision rather than a niceties-deferral — the fingerprint-pool asset URL — see §0; it fails soft with a clear, actionable error today, not a crash.

Legend: ⏸ parked (blocked on an external decision, not a bug) · 🐞 shipped-but-broken · 🔧 actionable tail · 🎯 intentional non-goal (recorded decision — *not* a gap) · 🧹 cleanup / stale-doc / code-residue · ✅ closed since snapshot

---

## §0 — ⏸ Parked (blocked on a decision, not a live failure)

- **Fingerprint pool asset URL is still a placeholder** → `source: pool` in `zendriver-mcp`'s `fingerprints_generate` returns a clear, actionable `internal_error` ("pool load failed (the pool asset may not be published yet — see issue #25): ...") rather than crashing or failing silently; `generative` remains the working default persona source. Parked on the #25 dataset-curation decision (extract + host a real-device fingerprint pool as a release asset) — not something to "fix" locally. `TODO(#25)` above the placeholder `POOL_URL` at `crates/zendriver-mcp/src/tools/fingerprints.rs:42-43` (lines 82-94 show the clear-error path).

---

## §1 — 🔧 Genuine tail (actionable; "never got around to it")

### Cheap win — blocker cleared since snapshot
- **`visible_only` find filter is still a NO-OP** — `crates/zendriver/src/query/mod.rs:905` & `:1276-1282` (`let _ = self.visible_only; let filtered = candidates;`). **But the old blocker is gone:** `actionability::check_visible` now *exists* (`crates/zendriver/src/query/actionability.rs:72-89`, already wired into `Element::is_visible` at `reads.rs:312`). It was simply never connected to `visible_only`. Small connect job now, not the big lift the stale `TODO(T16)` comment implies.

### Stealth / anti-detection
- **DataDome native slider/puzzle solving** (image-diff + Bézier drag). CAPTCHA surface is callback-only. — `crates/zendriver-datadome/src/bypass.rs:154-162`, `lib.rs:7`
- **DataDome `cookie_domain` public-suffix-list.** — *resolved, see §-closed*.
- **WebGPU full-adapter fabrication when the host has no GPU.** Patch returns early if `GPUAdapter` undefined; only decorates a real adapter's `.info`. — `crates/zendriver-stealth/src/patches/webgpu.js:23-29`
- **Sec-CH-UA for non-Chrome browsers** (Brave/Edge/Vivaldi); "post-1.0". `UserAgentMetadata::realistic()` hardcodes Chrome branding only; `Channel::Brave/Edge` picks the executable, not the brands. — `crates/zendriver-stealth/src/fingerprint.rs:35-62`

### Fingerprints
- (pool asset URL moved to §0 — parked on the #25 dataset-curation decision, not a nicety, and not a crash)
- **Mobile personas** (desktop-only). `Platform` enum = Win32/MacIntel/LinuxX86_64 only; generative sampler filters out mobile UAs; `mobile` hardcoded false. — `crates/zendriver-stealth/src/profile.rs:24-28`, `crates/zendriver-fingerprints/src/generative/mapping.rs:23-38`
- **Auto-refreshing pool on a schedule** + `force_refresh` / cache-TTL knob. Pure download-on-first-use with permanent cache; any cache hit short-circuits. — `crates/zendriver-fingerprints/src/pool/mod.rs:76-101`, `generative/download.rs:17-34`
- **Full attribute-coverage expansion** (screen metrics, navigator extras) + matching Persona/JS-patch growth. `persona_from_assignment` maps only platform/deviceMemory/hardwareConcurrency/videoCard/fonts; `Persona` has no screen-metric/navigator-extra fields. (Commit `402a427c` added *static* screen.js/mouse.js patches — coherent surface — but did **not** grow the BN or Persona.) — `crates/zendriver-fingerprints/src/generative/mapping.rs:41-83`, `crates/zendriver-stealth/src/persona/mod.rs:36-61`

### Geo / locale
- **`geo_auto` exact timezone from ip-api's `timezone` field** (more precise than the country-representative zone; needs a `GeoResolver` contract change to return a tz). `geo::persona()` (and `geo_auto`/`geo_locale` through it) sets a single representative IANA zone per country — multi-timezone countries (US, RU, CA, AU, BR, ...) don't get the visitor's actual local zone. `IpApiResolver`'s underlying `ip-api.com` response already carries a `timezone` field that isn't threaded through. — `crates/zendriver-stealth/src/geo/mod.rs`, `crates/zendriver/src/geo_resolver.rs`

### Network / cookies / tabs / frames
- **Transparent handle-preserving reconnect** (session-id remap so live `Tab` handles survive) + feature re-arm. `reconnect` clears the tab map (new sessionIds → invalidated handles). — `crates/zendriver/src/browser.rs:3479` (doc `:3436-3438`)
- **`partition_key` as a structured object** (currently flat `Option<String>` top-level-site). — `crates/zendriver/src/cookies/mod.rs:154-159`
- **Streaming response bodies** (monitor + HTTP `request()`); whole-body only, needs a Fetch-interception path. — `crates/zendriver/src/monitor/mod.rs:188`, `request.rs:346-380`
- **Frame OOPIF placeholder** — *resolved, see §-closed* (backlog previously mis-cited a test fixture).
- **Frame-scoped `.text()`/`.xpath()`/`.text_regex()` selectors ignoring CDP `contextId`** — *resolved, see §-closed*.
- **Cross-frame result ordering is non-deterministic** — *resolved, see §-closed*.

### Elements / input
- **Button-triggered file pickers** via `Page.fileChooserOpened`; today `upload_files` uses `DOM.setFileInputFiles` on direct `<input type=file>` only. — `crates/zendriver/src/element/actions.rs:585,600`
- **Case-insensitive matchers** (`[a="v" i]`, lowercased text compare). No CI flag/variant; no CI matcher in the public API. — `crates/zendriver/src/query/predicate.rs:53-66,85-86`
- **Custom mouse pressure / pen / touch input.** Mouse dispatch carries no pressure/pointerType; no `Input.dispatchTouchEvent` anywhere. — `crates/zendriver/src/input/mouse.rs:152-175`

### Launch / context / fetcher
- **Fetcher Beta/Dev/Canary channels not wired** — *resolved, see §-closed*.

### MCP surface
- **Per-action `version` accessor not exposed.** `chrome_version` hardcoded `String::new()`; core lib has no version getter. — `crates/zendriver-mcp/src/tools/lifecycle.rs:146` (doc `:78-80`)
- **Intercept `method` / `post-data` reserved for follow-up.** `ModifyRequest` exposes only `headers`. — `crates/zendriver-mcp/src/tools/intercept.rs:29-30,87-95`
- **Tab-scoped expectations/rules leak** until manual cancel or full `browser_close`. `browser_tab_close` never drains `s.expectations`/`s.rules`; handles carry no `tab_id`. — `crates/zendriver-mcp/src/tools/tabs.rs:244,264-311`, `state.rs:150,166`
- **MCP `geo_endpoint` override is not proxy-mirrored** (`IpApiResolver`'s proxy setter is `pub(crate)` to `zendriver`, so `zendriver-mcp` can't thread `input.proxy` into a custom-endpoint resolver the way `geo_auto()`'s bundled default gets mirrored). Currently only a `tracing::warn!` when both are set. Proper fix: expose the setter (or a builder arg) and wire `input.proxy` through in `lifecycle.rs::open`. — `crates/zendriver-mcp/src/tools/lifecycle.rs:143-159`, `crates/zendriver/src/geo_resolver.rs` (`with_proxy`)

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

- **Migration docs are stale.** — *resolved, see §-closed*.
- **~60 `#[allow(dead_code)]`** (non-test), **~35 still cite shipped task IDs** (e.g. "land with FindBuilder ext in T12" while `FindBuilder::one()/many()` are live). Audit: stale allows to remove, or genuinely-unused code. Concentrated in `crates/zendriver/src/query/selectors.rs`, `query/actionability.rs`, `input/*`, `element/mod.rs`.
- **43 `#[ignore]` integration tests** (grew from 38) need real Chrome / `--ignored`. **No CI job runs them:** the real-Chrome `test-integration` job runs `cargo nextest` with no `--run-ignored`, and no workflow passes `--ignored`/`--run-ignored`. They are silently rotting. — `.github/workflows/ci.yml:159`
- **`TODO(P5)` re-enable per-tab dialog-routing override.** Test `expect_dialog_resolves_on_alert` still `#[ignore]`. — `crates/zendriver/tests/integration_phase5.rs:215,221`
- **Imperva demo-site list `TODO`** (no stable public demo). — `crates/zendriver/tests/imperva_v0.rs`
- **Example placeholders:** `crates/zendriver/examples/cloudflare_bypass.rs:12-14` ("not yet clicked; single raw left-click"), `imperva_bypass.rs:5-6,40` (placeholder URL — set `IMPERVA_TEST_URL`).
- **`chromiumoxide_cdp` typed-Command migration: 0% done.** 28 raw `.call_raw()` sites / 561 `json!` params / 0 typed `.execute()`; grep for `chromiumoxide_cdp::` = 0 uses. The P1-deferred raw-JSON → typed-`Command` refactor was never started. — `crates/zendriver-transport/src/connection.rs:170`
- **Imperva fast-path interception integration test.** 🟡 A *unit* test now exists (`crates/zendriver-imperva/src/bypass.rs:847` — interception arm preempts the poll tick), but the nightly *integration* test (`crates/zendriver/tests/imperva_v0.rs:44-50`) still calls `wait_for_clearance()` with no `.with_interception()` — integration-level fast-path coverage remains absent.

---

## §-closed — ✅ Closed since the 2026-06-03 snapshot

- **Migration docs are stale** → **fixed** (2026-07-16, `docs: remove stale 'not yet' claims for shipped fingerprint features`). Both `docs/book/src/migration-zendriver-python.md` and `migration-nodriver-python.md`'s "Known gaps" sections no longer list canvas/WebGL/font/audio fingerprint spoofing or a browserforge equivalent as unshipped — both replaced with a note pointing at the `fingerprint.md` chapter (canvas/webgl/audio/fonts patches in `crates/zendriver-stealth/src/patches/`) and its `zendriver-fingerprints` pool/generative section (the browserforge-equivalent). The remaining genuine gaps in both files (OCR helpers, Widevine/DRM, `browser.get`/`__await__` shorthand, fuzzy `find`) are untouched — still real. `mdbook build docs/book` still passes.
- **DataDome `cookie_domain` public-suffix-list** → **fixed** (2026-07-16, `fix(datadome): use public-suffix list for cookie_domain (co.uk etc.)`). `cookie_domain` in `crates/zendriver-datadome/src/captcha.rs` now looks up the registrable domain via the [`psl`](https://crates.io/crates/psl) crate (compiled-in Mozilla Public Suffix List, no runtime fetch/embedded-data-file wiring needed) instead of a naive "drop the leftmost label" heuristic — `shop.example.co.uk` now correctly derives `.example.co.uk` instead of the old `.co.uk`. Note: used `psl` rather than the literally-named `publicsuffix` crate — `publicsuffix` v2 dropped its bundled list and now requires the caller to supply the PSL data file at runtime (its own README recommends `psl` for "a faster, ... static list"), so `psl`'s fully embedded, zero-I/O compiled list is the variant this backlog item's "prefer the embedded-list variant" note actually pointed to. Falls back to the bare host (no leading dot) for hosts with no registrable domain: bare IP addresses (guarded explicitly — the PSL's default "unlisted TLD" rule would otherwise misparse `127.0.0.1` as suffix `1`) and single-label hosts like `localhost`. Added to `psl.workspace = true` in `zendriver-datadome/Cargo.toml` (+ workspace `Cargo.toml`/`Cargo.lock`). Covered by unit tests: the original `co.uk` case (now asserting the correct `.example.co.uk`), a private-section PSL entry (`uk.com`), and the IP-address fallback.
- **Fetcher Beta/Dev/Canary channels not wired** → **fixed** (2026-07-16, `fix(fetcher): wire Beta/Dev/Canary channels`). `crates/zendriver-fetcher/src/resolver.rs` gained `resolve_channel_download_url`, which resolves a non-`Stable` `Channel` against Chrome for Testing's per-channel `last-known-good-versions-with-downloads.json` manifest (new `ChannelsResponse`/`ChannelEntry` types + `fetch_channels_manifest_from` in `manifest.rs`, keyed by channel name rather than a flat version list) — `Stable`/`Latest`/`Explicit` keep resolving through the existing flat `known-good-versions-with-downloads.json` path. `Fetcher::ensure_chrome` (`fetcher.rs`) branches on `VersionSpec::Channel` to pick the right manifest + resolver; `Channel::as_cft_str` maps the enum to the manifest's `"Stable"`/`"Beta"`/`"Dev"`/`"Canary"` keys. Covered by resolver-level unit tests (per-channel resolution, missing-platform, missing-channel) plus an end-to-end wiremock test resolving `Channel::Beta` through a stub channels manifest. Docs updated: `docs/book/src/fetcher.md`, `docs/book/src/error-reference.md`, and the `zendriver-mcp` `browser_install_chrome` tool doc comments (all four channels were already accepted on the wire; only the "not yet wired" prose was stale).
- **Cross-frame result ordering is non-deterministic** → **fixed** (2026-07-16, `fix(tab): sort frames() by frame id for deterministic cross-frame results`). `Tab::frames()` (`crates/zendriver/src/tab.rs`) now sorts its `HashMap`-backed snapshot by `Frame::id()` before returning, so `include_frames()`'s cross-frame fan-out (`one_across_frames`/`many_across_frames` in `crates/zendriver/src/query/mod.rs`, both of which iterate `tab.frames()` in registry order) gets a stable, run-to-run-consistent frame order instead of unspecified `HashMap::values()` iteration. Covered by a MockConnection unit test (`frames_are_sorted_by_id_regardless_of_attach_order`) that attaches three frames out of lexical order and asserts `frames()` returns them sorted.
- **Frame-session `Runtime.evaluate`** → **shipped** (commit `5440066b`). `QueryScope::session()` returns the frame's session; `execution_context_id()` pins eval to the frame's isolated-world context; test `in_frame_override_routes_dispatch_to_frame_session` asserts it. (Backlog's old `query/mod.rs:1873` citation is now just a comment inside that test.)
- **Nightly real-Chrome anti-detection CI** → **exists**. `nightly-stealth-tests` (cron `0 6 * * *`, real Chrome) runs `--test stealth_phase2`, which hits `bot.sannysoft.com` and `arh.antoinevastel.com/bots/areyouheadless`. — `.github/workflows/ci.yml:200-224`
- **OOPIF bootstrap "placeholder Frame"** → **was already done**; the backlog mis-cited test-fixture setup. Real impl: `crates/zendriver/src/frame/oopif.rs:49` `register_oopif_frame`, wired at `browser.rs:1497` for `kind=="iframe"` (commit `b13fdbd3`).
- **Per-context proxy auth — first-class API** → **shipped** via `Browser::browser_context()` → `BrowserContextBuilder` (`.proxy()`/`.proxy_bypass()`/`.proxy_auth()`/`.build()`), auto-installing per-tab `Fetch.authRequired` chained into the one per-session interception actor (2026-07-16 plan).
- **Auto IP-geo resolution / Option B exit-IP probe** → **shipped** via `BrowserBuilder::geo_auto()` (bundled `IpApiResolver`, a proxied `ip-api.com` GET, opt-in) + `BrowserBuilder::geo_resolver()` for a custom `GeoResolver` impl, plus a structured `BrowserBuilder::proxy(url)` (reusing `crate::proxy::split_proxy_url`) so the probe mirrors the browser's own proxy. Exposed over MCP as `browser_open.geo_auto` / `.geo_endpoint` / `.proxy`. Explicit `geo_locale`/persona locale still wins and skips the probe; fail-soft on probe failure. (2026-07-16 `geo-auto-resolver` plan.) Note: this closed only the country→locale/languages half — timezone-from-geo derivation was the remaining half, since closed below.
- **timezone-from-geo derivation** → **shipped** via `geo::persona(country)` now also setting `Persona.timezone` to a representative IANA zone, drawn from a generated `TIMEZONES` table (country → zone, from IANA `zone1970.tab` tag `2026c`, first-occurrence-per-country + curated overrides for RU/UA/CA/AU/BR). `locale-gen` emits `TIMEZONES` + `TZDATA_VERSION` (mirroring the existing `COUNTRIES`/`CLDR_VERSION` pattern); `geo::tzdata_version()` exposes provenance. Wired through `geo_locale`/`geo_auto` for free (both terminate in `geo::persona`). Representative-zone caveat: multi-timezone countries get one zone, not the visitor's precise local zone — see the new `geo_auto` follow-up above. (2026-07-16 `timezone-from-geo` plan.) — `crates/zendriver-stealth/src/geo/mod.rs`, `crates/locale-gen/src/lib.rs`
- **Frame-scoped xpath/text/text_regex selectors ignoring `contextId`** → **fixed** (2026-07-16, `fix(query): pin frame-scoped xpath/text/role selectors to the frame contextId`). `resolve_xpath_many`, `resolve_text_many`, and `resolve_text_regex_many` in `crates/zendriver/src/query/selectors.rs` now thread `scope.execution_context_id()` into `Runtime.evaluate`'s `contextId` on their `Tab`/`Frame` arm — same pattern `resolve_css_many`/`resolve_predicate_many` already used. Extracted a shared `eval_expr_in_scope` helper so every css/xpath/text/text_regex/predicate resolver (both `_one` and `_many`) routes through one `contextId`-pinning implementation instead of repeating it per resolver. `resolve_role_one`/`resolve_role_many` were never actually broken — they delegate to `resolve_css_many`, which already set `contextId` correctly. Covered by a MockConnection unit test (asserts `contextId` on the outbound `Runtime.evaluate`) plus a real-Chrome `#[ignore]` iframe test (`crates/zendriver/tests/find_frame_selectors.rs`).
- **🐞 `text_regex` selector lacks the "narrowest match" filter** → **fixed** (2026-07-16, `fix(query): narrow text_regex matches to the innermost element (was returning ancestors)`). `build_text_regex_js_tab`/`build_text_regex_fn_body` in `crates/zendriver/src/query/selectors.rs` now subtract any element with a descendant also matching the same `RegExp` (reusing `r.test(t)`) before the `best_match` sort, via a shared `regex_narrowing_js` fragment — mirroring the narrowing the substring builders already had, without touching their emitted JS. Covered by a MockConnection unit test (asserts the descendant-subtraction filter is present in the emitted expression) plus a real-Chrome `#[ignore]` test (`crates/zendriver/tests/find_predicate_iframe.rs::text_regex_resolves_to_innermost_leaf_not_ancestor`) confirming `.text_regex("match-me").one()` on `<div><span>match-me</span></div>` now resolves to the `<span>`, not `<html>`.

---

*Originally a session artifact during the obscura-comparison / stealth-features work (2026-06-03); re-verified 2026-07-16. A tracking snapshot, not a spec/plan. Update or delete as items close.*
