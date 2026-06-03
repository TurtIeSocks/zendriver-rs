# zendriver-rs — Deferred / Never-Got-Around-To Backlog

> **Snapshot:** 2026-06-03. Point-in-time scan of `docs/superpowers/{specs,plans}/`, the mdBook (`docs/book/src/`), all crate code + tests, and `crates/zendriver-mcp/mcp-coverage-ledger.toml`. Statuses verified against shipped code/git as of this date — re-verify before acting on any item (a file/line/flag named here may have moved).
>
> **Method (to regenerate):** grep specs/plans for deferral language (`deferred`, `out of scope`, `later`, `follow-up`, `future`, `not yet`, `for now`, `Option B`, `post-1.0`, `TODO`, unchecked `- [ ]` that aren't TDD steps) + code for `TODO`/`FIXME`/`todo!()`/`#[ignore]`/`#[allow(dead_code)]`/prose deferrals + the MCP ledger `excluded` entries. Cross-check each against shipped code.
>
> **Already being handled (excluded from the tail below):**
> - **#1 native-function `toString` masking** → PR [#49](https://github.com/TurtIeSocks/zendriver-rs/pull/49) (in review).
> - **#2 tracker/fingerprinter blocklist** → handoff doc `docs/superpowers/plans/2026-06-03-tracker-blocklist-handoff.md`.
>
> **Big picture:** the 6-phase port + parity A–E + fingerprints PR2 + MCP all shipped; most "deferred to phase N" items were since built. The genuine remaining tail is small and concentrated in stealth depth + a few convenience/limitation tails.

Legend: 🔧 actionable tail · 🎯 intentional non-goal (recorded decision — *not* a gap) · 🧹 cleanup / stale-doc / code-residue · ❓ status unverified

---

## §1 — 🔧 Genuine tail (actionable; "never got around to it")

### Stealth / anti-detection
- **DataDome native slider/puzzle solving** (image-diff gap + Bézier drag). v1 delegates to an opt-in solver callback. — `2026-06-02-datadome-bypass-design.md:371`
- **DataDome `cookie_domain` public-suffix-list.** Multi-label TLDs (`co.uk`) fail; v1 accepts the limitation. — `crates/zendriver-datadome/src/captcha.rs:184`, `datadome plan:1000`
- **WebGPU full-adapter fabrication when the host has no GPU.** v1 only decorates a real adapter's `.info`. — `datadome plan:1597`
- **Sec-CH-UA for non-Chrome browsers** (Brave/Edge/Vivaldi); "post-1.0". — `2026-05-23-...-phase2-stealth-design.md:35`

### Fingerprints
- **`TODO(#25)` real pool release-asset URL** still a placeholder (pool sampling errors until the dataset is hosted). — `crates/zendriver-mcp/src/tools/fingerprints.rs:42`, `gen-bn plan:876`
- **Mobile personas** (desktop-only today). — `fingerprint:51`, `gen-bn:180`
- **Auto-refreshing pool on a schedule** + `force_refresh` / cache-TTL knob. — `fingerprint:52`, `gen-bn:103`
- **Full attribute-coverage expansion** (screen metrics, navigator extras) + matching Persona/JS-patch growth. — `gen-bn:332`

### Geo / locale
- **timezone-from-geo derivation** (country → IANA tz, sharing the `geo` module). — `2026-06-03-geo-ip-locale-design.md:42`
- **Auto IP-geo resolution / Option B exit-IP probe** (only the `GeoResolver` seam ships; `geo_locale` needs an explicit country). — `geo plan:1299`, `crates/zendriver-stealth/src/geo/mod.rs:54`

### Network / cookies / tabs / frames
- **Transparent handle-preserving reconnect** (session-id remap so live `Tab` handles survive) + feature re-arm. v1 invalidates handles; re-acquire via `main_tab()`/`tabs()`. — `parity-D:58`, `crates/zendriver/src/browser.rs:2715`
- **`partition_key` as a structured object** (currently a flat `String` top-level-site). — `crates/zendriver/src/cookies/mod.rs:157`
- **Streaming response bodies** (monitor + HTTP `request()`); needs a Fetch-interception path. — `network-monitor:200`
- **OOPIF bootstrap placeholder Frame** — `crates/zendriver/src/browser.rs:3979,3988` (known limitation around out-of-process iframes).
- **Frame-session follow-up:** `Runtime.evaluate` should run on the Frame's own session. — `crates/zendriver/src/query/mod.rs:1873`

### Elements / input
- **`visible_only` find filter is a NO-OP** pending `actionability::check_visible` (`TODO(T16)`; MCP passes the bool through). — `crates/zendriver/src/query/mod.rs:901,1276`
- **Button-triggered file pickers** via `Page.fileChooserOpened`; today only direct `<input type=file>`. — `phase3:798`
- **Case-insensitive matchers** (`[a="v" i]`, lowercased text compare). — `find-dom:152`
- **Custom mouse pressure / pen / touch input.** — `phase3:38`
- **Cross-tab drag-and-drop.** — `phase4:35`

### Launch / context / fetcher
- **Per-context proxy auth** (auth is browser-wide today). — `book/browser-context.md:73`
- **Fetcher Beta/Dev/Canary channels not wired** (treated as Latest). — `crates/zendriver-fetcher/src/version.rs:13,26`

### MCP surface
- **Element-scoped find over MCP** (low demand). — `mcp-coverage-ledger.toml:159`
- **Interception `Stream` escape-hatch over MCP** (needs server-side event buffering + backpressure). — `mcp-server:533`
- **Per-action `version` accessor not exposed.** — `crates/zendriver-mcp/src/tools/lifecycle.rs:54`
- **Intercept `method` / `post-data` reserved for follow-up.** — `crates/zendriver-mcp/src/tools/intercept.rs:30`
- **Tab-scoped expectations/rules leak** until manual cancel/close (v0 limitation). — `crates/zendriver-mcp/src/tools/tabs.rs:244`, `crates/zendriver-mcp/src/server.rs:224`

---

## §2 — 🎯 Intentional non-goals (recorded decisions — do not mistake for gaps)

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

- **Migration docs are stale.** "Known gaps in v0.1.0" still lists canvas/WebGL/font/audio fingerprint spoofing + browserforge integration as "not yet" — **all shipped**. Fix `docs/book/src/migration-zendriver-python.md:184+` and `migration-nodriver-python.md:233+`.
- **~40 `#[allow(dead_code)]`** referencing tasks (T12/T15/T17/T23, etc.) that mostly shipped. Audit: stale allows to remove, or genuinely-unused code. Concentrated in `crates/zendriver/src/query/selectors.rs`, `query/actionability.rs`, `input/*`, `element/mod.rs`, `zendriver-fetcher/src/manifest.rs`.
- **38 `#[ignore]` integration tests** (need real Chrome / `--ignored`). Confirm a CI job actually runs them on a Chrome runner; otherwise they're silently rotting. (The new `stealth_native_masking.rs` from PR #49 joins this set.)
- **`TODO(P5)` re-enable per-tab dialog-routing override.** — `crates/zendriver/tests/integration_phase5.rs:215`
- **Imperva demo-site list `TODO`** (no stable public demo). — `crates/zendriver/tests/imperva_v0.rs:7`
- **Example placeholders:** `cloudflare_bypass.rs:13` ("not yet clicked; single raw left-click"), `imperva_bypass.rs:5` (placeholder URL — set `IMPERVA_TEST_URL`).

---

## §4 — ❓ Status unverified (couldn't confirm done/open quickly)

- **`chromiumoxide_cdp` typed-Command migration** (P1 deferred raw-JSON → typed `Command`; many calls may still be raw). — `phase1 plan:4027`
- **Nightly real-Chrome anti-detection CI** (sannysoft / areyouheadless) — does the job exist? — `phase2:12`
- **Imperva fast-path interception integration test** (deferred to nightly). — `imperva plan:2197`

---

*Generated as a session artifact during the obscura-comparison / stealth-features work on 2026-06-03. Not a spec or plan — a tracking snapshot. Update or delete as items close.*
