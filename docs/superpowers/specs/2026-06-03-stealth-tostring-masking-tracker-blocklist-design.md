# Stealth: native-function `toString` masking + tracker/fingerprinter blocklist

- **Date:** 2026-06-03
- **Status:** Approved design (pre-implementation)
- **Crates touched:** `zendriver-stealth`, `zendriver-interception`, `zendriver`, `zendriver-mcp`
- **Origin:** comparison with [obscura](https://github.com/h4ckf0r0day/obscura) (a synthetic DOM + V8 engine) surfaced two anti-detection techniques worth importing into zendriver-rs, which drives *real* Chrome.

The two features are independent and may be implemented as separate task groups; they share this spec because they share the obscura-derived motivation and the stealth theme.

## 1. Context & motivation

1. **Native-function `toString` masking.** The spoofed profile patches ~16 prototype
   methods/getters by reassignment (`proto.getParameter = function(p){…}`,
   `crates/zendriver-stealth/src/patches/webgl.js:8`) and `Object.defineProperty(..,{get})`
   (`patches/webdriver.js:4`). Those replacements report their JS source via
   `Function.prototype.toString` and carry an empty `.name` — the canonical
   FingerprintJS/CreepJS unmasking vector. The spoofed profile does not currently
   mask this; it was deferred under the "detected-sites out of scope" tier
   (`2026-05-23-zendriver-rs-phase2-stealth-design.md` §scope).
2. **Tracker/fingerprinter blocklist.** obscura ships a domain blocklist that
   short-circuits requests. zendriver-rs already has the mechanism
   (`zendriver-interception`, `Fetch.*`) but no list or policy. Blocking
   third-party fingerprinters shrinks the detection surface and speeds page loads.

obscura's own `toString` masking is single-realm (it cannot defend cross-realm
probes), and its blocklist is vendored from Peter Lowe's list with no attribution
(a latent license problem). This design improves on both.

## 2. Goals / non-goals

**Goals**

- Spoofed-profile patched functions/getters are indistinguishable from native under
  direct *and* cross-realm `toString` / `.name` / `.length` inspection, in every frame.
- Opt-in blocking of third-party fingerprinter/tracker hosts, with a license-clean
  bundled default and runtime-extensible custom lists.

**Non-goals**

- Masking in synthetic/detached realms that never receive a CDP document lifecycle
  (best-effort only; see §5.4).
- Blocking active anti-bot challenge providers (we want to *pass* those; see §4.4).
- Build-time or vendored third-party blocklists (license + reproducibility; see §4.3).
- Native-profile changes (it patches nothing, so it has no `toString` tell).

## 3. Feature 1 — native-function `toString` masking

### 3.1 Approach — centralized JS prelude + route all patches through helpers

New `crates/zendriver-stealth/src/patches/_native.js`, prepended **first** in
`bootstrap_script` (before the identity IIFE in `patches.rs:58`). Inside the
bootstrap's scope it installs:

- A `Function.prototype.toString` override backed by a closure-private `WeakSet` of
  marked functions. If `this` is marked, return a synthesized native string —
  `function NAME() { [native code] }`, or the `function get NAME() { [native code] }`
  / `set` form for accessors. Otherwise delegate to the saved original `toString`.
- The override marks itself and the saved original native (self-concealing), so
  `Function.prototype.toString.toString()` also reports native.
- `__zdReplace(obj, prop, make)`: capture `orig = obj[prop]`; `fn = make(orig)`; copy
  `orig.name` and `orig.length` onto `fn` via `defineProperty`; add `fn` to the marked
  set; assign `obj[prop] = fn`.
- `__zdGetter(obj, prop, getFn, { enumerable, setFn })`: `defineProperty` an accessor;
  mark `getFn` (and `setFn`) native, recording the `get `/`set ` name form.

Each patch file is refactored to route its method replacement and `defineProperty`
through these helpers instead of raw assignment/`defineProperty`.

### 3.2 Cross-realm coverage

The full bootstrap is injected via `Page.addScriptToEvaluateOnNewDocument`, which runs
in every frame/document. The prelude therefore re-installs the override in each realm,
so cross-realm probes (`iframe.contentWindow.Function.prototype.toString.call(patchedFn)`)
hit the *child* realm's overridden `toString`. This closes the single-realm hole obscura
cannot.

### 3.3 Files touched

- **New:** `patches/_native.js`.
- `patches.rs`: prepend the prelude; tests asserting routing.
- Patch files routed through helpers: `webdriver.js`, `navigator_props.js`, `plugins.js`,
  `chrome.js`, `permissions.js`, `codecs.js`, `user_agent_data.js`, `broken_image.js`,
  `webgl.js`, `canvas.js`, `audio.js`, `client_rects.js`, `fonts.js`, `hardware.js`,
  `webrtc.js`, `webgpu.js`.

### 3.4 Rejected alternatives

- **Trap `Object.defineProperty` to auto-mark:** misses plain `proto.x = fn`
  reassignment, the dominant pattern here.
- **Post-pass snapshot diff:** requires a hand-maintained registry of patched targets;
  brittle.

### 3.5 Scope, API, tests

- Spoofed profile only.
- No public API or MCP surface added (internal bootstrap behavior).
- **Unit:** bootstrap contains the prelude; patches route through `__zdReplace` /
  `__zdGetter`.
- **Integration (`#[ignore]`, real Chrome):** `WebGLRenderingContext.prototype.getParameter.toString()`
  ends in `[native code]`, `.name === 'getParameter'`, `.length === 1`;
  `Object.getOwnPropertyDescriptor(Navigator.prototype,'webdriver').get.toString()` is
  the getter native form; a cross-frame iframe repeats the `getParameter` assertion.

## 4. Feature 2 — tracker/fingerprinter blocklist

### 4.1 Mechanism (`zendriver-interception`, pure matching — no data, no network)

- `HostMatcher`: a `HashSet<String>` plus a parent-domain walk on dot boundaries.
  `is_blocked(host)` checks the exact host, then strips the leftmost label repeatedly
  (`a.b.evil.com → b.evil.com → evil.com`) until a match or the bare root.
- `Rule::BlockHosts { matcher: Arc<HostMatcher> }` (extends the enum in
  `rule.rs:31`): the actor extracts the host from `Fetch.requestPaused.request.url`; on a
  match it reuses the existing `Block` path
  (`Fetch.failRequest { errorReason: "BlockedByClient" }`). Composes in registration
  order with other rules.

### 4.2 Opt-in & sourcing (`zendriver` core)

- **Bundled curated list:** `include_str!` of our own `trackers.txt` (~50–150 known
  third-party fingerprinter/tracker hosts). Authored by us → no third-party license.
- **BrowserBuilder methods:**
  - `block_trackers(bool)` — toggle the bundled list.
  - `tracker_blocklist_add(domains)` / `tracker_blocklist_file(path)` /
    `tracker_blocklist_url(url)` — accumulate custom sources; the presence of any source
    implicitly enables blocking. Only-custom = supply a source without
    `block_trackers(true)`; bundled + custom = both.
- **Runtime fetch + cache for `_url`:** reuse the atomic-write download-on-first-use
  pattern from `zendriver-fingerprints` `pool::load_or_download` (`pool/mod.rs:76`).
  Implemented in core (under the `tracker-blocking` feature, with `reqwest`) so
  interception stays network-free.
- **Wiring:** the browser builds one `Arc<HostMatcher>` and installs a `Rule::BlockHosts`
  on each new tab's interception at creation.

### 4.3 Why not vendored / compile-time

The shipped artifact embeds whatever the build sees, so `include_str!`,
generate-and-commit, and `build.rs` fetch are **all redistribution** — the list's
license attaches identically. CLDR (Unicode license) and the apify fingerprint network
(Apache-2.0) are baked precisely because they *permit* redistribution; Peter Lowe's list
does not (use-restricted, redistribution discouraged, commercial use gated). The only
model that moves the license boundary onto the end user is a runtime fetch on the user's
machine under their acceptance — which is also where actual freshness lives (a library
consumer pins a version and builds once, so a compile-time list is stale by runtime).
Hence: bundle our own clean list; offer runtime BYO for everything else (a user who
accepts Peter Lowe's terms points `tracker_blocklist_url` at it themselves).

### 4.4 Curation principle (load-bearing)

The curated list contains third-party *passive* fingerprinters and cross-site trackers
only. It **deliberately excludes active anti-bot challenge providers** (DataDome,
Cloudflare, PerimeterX, Imperva): blocking those breaks access to the very sites we are
trying to reach.

### 4.5 Matching semantics

Host-only (path ignored), suffix-on-dot (subdomains of a listed domain are blocked),
unconditional (no first-party exemption in v1; possible later refinement).

### 4.6 Feature-gating

New `tracker-blocking` feature in core that implies `interception`; the bundled list
adds binary size only when enabled. `zendriver-mcp` exposes a matching feature.

### 4.7 MCP coverage

Extend `browser_open` with `block_trackers: bool` and an optional `tracker_blocklist`
input (url | file | inline domains). Regenerate the `insta` schema snapshots and
`public-api-baseline.txt`. Ledger-exclude the lower-level `HostMatcher` and
`Rule::BlockHosts` as internal (reached via the builder/tool).

### 4.8 Rejected alternatives

- **Separate pre-filter layer before the rule list:** a parallel concept; the rule
  variant composes and reuses the handle/teardown.
- **A dedicated Fetch handler in core:** duplicates the interception actor.
- **List as N `Rule::Block` glob patterns:** O(n) glob per request over thousands of
  entries; `HostMatcher` is the reason for a set-membership variant.

### 4.9 Tests

- `HostMatcher` unit: exact match, subdomain walk, miss, bare-root.
- `Rule::BlockHosts` actor (mock transport): match → fail path.
- Core: bundled list parses; builder wires the matcher; fetch+cache against a mock
  server (or `#[ignore]`).
- Integration (`#[ignore]`, real Chrome): a listed host yields
  `net::ERR_BLOCKED_BY_CLIENT`; an unlisted host loads.

## 5. Cross-cutting

### 5.1 Crate layering

Interception holds the pure matching mechanism (no data, no network). Core owns the
bundled data, the opt-in builder API, and the runtime fetch/cache. Stealth owns the
`toString` prelude. No new cross-crate dependencies beyond core → interception (already
present behind the `interception` feature).

### 5.2 MCP & schema discipline

Per project rules, any public API change is validated against `zendriver-mcp`. Feature 1
adds none. Feature 2 adds builder options surfaced through `browser_open`; regenerate
schema snapshots and the public-API baseline, and record ledger exclusions for the
internal types.

### 5.3 Pre-push gates

`cargo fmt --all`; `clippy --all-targets -D warnings` (default features, and — since
feature-gated code is touched — `-p zendriver-mcp --all-features`); schema snapshots;
the public-api check.

### 5.4 Residual limitations

- **toString masking:** synthetic/detached realms with no CDP document lifecycle
  (`document.implementation.createHTMLDocument`, a detached-never-inserted iframe) keep a
  pristine `Function.prototype.toString` and can unmask. Niche; obscura cannot do real
  cross-frame at all.
- **Blocklist:** an unconditional host match may block a listed host used first-party;
  acceptable for a curated third-party list, revisitable via a first-party exemption.

## 6. Open items (plan-time)

- Exact contents of the curated `trackers.txt` (authored during implementation; reviewed
  against the §4.4 exclusion principle).
- Precise seam in core where per-tab interception is installed at tab creation.
