# Header coherence (#38): Accept-Encoding vs claimed Chrome major

**Issue:** [#38](https://github.com/TurtIeSocks/zendriver-rs/issues/38) — "Generative
fingerprints: header-network coherence (Accept / sec-ch-ua / header order)".
Follow-up to [#25](https://github.com/TurtIeSocks/zendriver-rs/issues/25) (generative Bayesian fingerprint network).
**Date:** 2026-06-02
**Scope:** `zendriver-stealth` apply path (resolve + observer). **No** new crate,
**no** network download, **no** `Persona`/`HeaderProfile` type, **no** new MCP tool.

> ⚠️ **This spec reframes the issue.** #38 was filed assuming a faithful port of
> browserforge's header Bayesian network (vendor the blobs, reuse #25's sampler,
> add a carrier type, inject all headers + order via CDP). After grounding the
> design in the actual data files and zendriver's apply path, that port is
> **redundant and partly counter-productive for a real-Chrome-over-CDP tool**.
> The genuinely valuable, non-harmful slice is a single deterministic override:
> **`Accept-Encoding`, conditioned on the claimed Chrome major.** The reasoning
> is in §3; the rejected work is in §9 (Assumptions) and §10 (Out of scope).
> Assumption **A1** is the headline judgement call — the delegate-mode review
> checkpoint.

## 1. Goal

Keep the request headers Chrome sends coherent with the **claimed** stealth
identity. Concretely: when a spoofed profile pins a Chrome major (or platform)
that differs from the real launched binary, ensure `Accept-Encoding` matches the
*claimed* major rather than leaking the *binary's* value. Everything else the
header network models is either already coherent or cannot be set safely/at all
over CDP (§3).

## 2. Why not the faithful header-network port — verified data

Downloaded and inspected the three apify blobs the issue names
(`packages/header-generator/src/data_files/`, Apache-2.0):

- **`header-network-definition.zip`** — 43.5 KB → `network.json` 808 KB, **19
  nodes**. Same Bayesian schema as #25 (`conditionalProbabilities`
  `deeper`/`skip`, `*STRINGIFIED*`).
  - 4 conditioning roots: `*BROWSER` (e.g. `chrome/147.0.0.0`, 126 values incl.
    Safari/Firefox/Edge), `*OPERATING_SYSTEM` (`macos|windows|ios|android|linux`),
    `*DEVICE` (`desktop|mobile`), `*HTTP_VERSION` (`_2.0_|_1.1_`).
  - Header nodes come in **two casing variants** — lowercase (`accept`,
    `user-agent`, `sec-ch-ua-mobile`, …) for HTTP/2, Title-Case (`Accept`,
    `User-Agent`, …) for HTTP/1.1. **Header-name casing is itself the
    fingerprint** the network models.
  - `accept` / `accept-encoding` / `dnt` / `upgrade-insecure-requests` are
    conditioned on the sampled **`user-agent` string** (the full UA is the CPT
    key); `sec-ch-ua*` on the roots + `sec-ch-ua-mobile`.
- **`input-network-definition.zip`** — 4.5 KB, **5 nodes** (`*DEVICE → *OS →
  *BROWSER_HTTP → {*HTTP_VERSION, *BROWSER}`). Purpose: turn a user's *partial*
  constraints into a coherent (device, os, browser, http) tuple. **zendriver
  already knows all four** from the resolved `Fingerprint` (desktop, platform,
  chrome/<major>, HTTP/2), so this network is unused for us.
- **`headers-order.json`** — per-browser header order. The `chrome` list is
  exactly the order **a real Chrome already emits** (`Host, Connection, …,
  sec-ch-ua, sec-ch-ua-mobile, sec-ch-ua-platform, …, User-Agent, Accept,
  Sec-Fetch-*, Referer, Accept-Encoding, Accept-Language, Cookie`, then the
  HTTP/2 pseudo-headers).

zendriver's apply path (verified in code):

- `crates/zendriver-stealth/src/fingerprint.rs:113` — `probe_chrome_version(exe)`:
  the default `chrome_major`/`chrome_full` is **read from the real binary**. So
  **claimed major == binary major** unless the user calls `.chrome_version(n)`
  (`profile.rs:202`) or supplies a full `Fingerprint` override.
- `crates/zendriver-stealth/src/observer.rs:86` — `Emulation.setUserAgentOverride`
  carries `userAgentMetadata`. Chrome **derives `sec-ch-ua`, `sec-ch-ua-mobile`,
  `sec-ch-ua-platform`** (and the high-entropy UA-CH hints) from that metadata,
  for the **claimed** major.

## 3. The per-header verdict (the core of the design)

For each header the network models, what a real Chrome over CDP already does, and
whether we can/should override it:

| Header | Chrome already sends | Override via CDP? | Verdict |
|---|---|---|---|
| `sec-ch-ua`, `-mobile`, `-platform` | **Yes, coherent** — derived from the UA-CH metadata the observer sets (claimed major) | Would *break* coherence (BN samples a different UA) | **Skip** |
| `User-Agent` | Yes — we already override it coherently | n/a | **Skip** |
| `Accept` | Yes — but it is **per-request-type** (document vs `image/*` vs `*/*` for fetch) | `setExtraHTTPHeaders` is **global** → forcing one value corrupts sub-resource requests | **Skip** (footgun) |
| `Sec-Fetch-*` | Yes, per-request | global override would corrupt | **Skip** |
| `Accept-Encoding` | Yes — the **binary's** value; **uniform across request types** | safe to set globally; skews with claimed-vs-binary major (`zstd` since Chrome 123) | **Override — the one valuable, safe header** |
| `DNT` | No (only if user enabled it) | injecting `DNT:1` is *rarer*, not stealthier | **Skip** |
| `upgrade-insecure-requests` | Yes (`1` on navigation) | redundant | **Skip** |
| `Connection`, `te` | network-stack managed; unused on HTTP/2 | not injectable | **Skip** |
| **header order / name casing** | Chrome's **real order**, HTTP/2-lowercased | `setExtraHTTPHeaders` cannot reorder; HTTP/2 forces lowercase | **Skip** (already correct; not controllable) |

**Conclusion:** the lone header that is (a) uniform across request types,
(b) able to skew from the claimed identity, and (c) safely settable globally over
CDP is **`Accept-Encoding`**. Everything else is already coherent (Chrome is a
real browser), per-request and unsafe to force globally, or non-injectable.
Sampling headers from the BN would, at best, reproduce what Chrome already sends
and, at worst, inject values inconsistent with the claimed major — the opposite
of the issue's goal. A **deterministic, by-major** value is strictly *more*
coherent than a BN sample.

## 4. Design

### 4.1 `accept_encoding_for(major)` — the coherence rule

A tiny pure helper (new `crates/zendriver-stealth/src/headers.rs`):

```rust
/// The `Accept-Encoding` a real Chrome of `major` sends for a top-level
/// navigation over HTTPS. `zstd` shipped enabled-by-default in Chrome 123
/// (2024-03); `br` (brotli) since Chrome 50. Older majors zendriver will not
/// realistically drive, so two branches suffice.
pub(crate) fn accept_encoding_for(major: u32) -> &'static str {
    if major >= 123 {
        "gzip, deflate, br, zstd"
    } else {
        "gzip, deflate, br"
    }
}
```

### 4.2 Detect the skew in `resolve_fingerprint`

`StealthProfile::resolve_fingerprint` (`profile.rs:280`) already holds both values
it needs: on the `auto_detect` path, `fp.chrome_major` is the **binary** major
*before* the `.chrome_version()` override is applied at line 288. Compute the
override only when the claimed and binary majors land on **different sides of the
`zstd` boundary** (i.e. the encodings actually differ):

```rust
// before applying per_field.chrome_major:
let binary_major = fp.chrome_major;                 // probed (auto_detect path)
// after applying overrides, with claimed = fp.chrome_major:
let coherent_accept_encoding: Option<String> =
    (accept_encoding_for(claimed) != accept_encoding_for(binary_major))
        .then(|| accept_encoding_for(claimed).to_string());
```

- **Default path (no override):** claimed == binary → `None` → **zero added CDP
  traffic, no `Network.enable`.** The common case pays nothing.
- **`fingerprint_override` (full `Fingerprint`) / connect-without-exe path:** the
  binary major is unknown; treat as `None` (no skew we can prove). Documented
  limitation — these are explicit-identity paths where the user owns coherence.

`resolve_fingerprint` returns the `Option<String>` alongside the `Fingerprint`
(small struct or tuple — plan picks the seam; both are `pub(crate)`-reachable from
`browser.rs`, **no public-API change**).

### 4.3 Apply in the observer

Thread the `Option<String>` into `StealthObserver` (constructor arg or builder
setter, mirroring `with_persona`). In `on_target_attached`, for a `page` target in
`Spoofed` mode **only when `Some`**:

```rust
session.call("Network.enable", json!({})).await?;
session.call(
    "Network.setExtraHTTPHeaders",
    json!({ "headers": { "Accept-Encoding": ae } }),
).await?;
```

`Network.enable` is idempotent (interception / monitor / network-idle already
enable it where active) and is invisible to the page. It is sent **only** on the
skew path, so the default profile is unaffected.

### 4.4 Empirical CDP gate (resolved during implementation, not assumed)

**Risk:** Chrome's network service may *append* rather than *replace*
`Accept-Encoding` set via `setExtraHTTPHeaders`, yielding a duplicated/merged
header — which is *more* detectable, not less. The plan's **first step** is a
throwaway probe against a real Chrome (set the header, read the outbound request
via a local echo server / `Network.requestWillBeSentExtraInfo`) to confirm
clean replacement.

- **If it replaces cleanly:** ship §4.3 as written (no new feature dependency).
- **If it appends/duplicates:** escalate to the `Fetch` path — the
  `zendriver-interception` crate already replaces request headers
  (`paused.rs:198`, `actor.rs:454`). The coherence fix is then **gated behind the
  `interception` feature** (off by default). This fork is decided by the probe,
  not guessed.

## 5. Files touched

- `crates/zendriver-stealth/src/headers.rs` — **new**: `accept_encoding_for`,
  unit tests, module doc explaining the `zstd`/`br` history.
- `crates/zendriver-stealth/src/lib.rs` — `mod headers;`.
- `crates/zendriver-stealth/src/profile.rs` — `resolve_fingerprint` also computes
  + returns the `Option<String>` coherent `Accept-Encoding` (signature tweak;
  update the 2 call sites in `browser.rs`).
- `crates/zendriver-stealth/src/observer.rs` — carry the `Option<String>`; send
  `Network.enable` + `Network.setExtraHTTPHeaders` on the skew path; extend the
  existing command-sequence tests.
- `crates/zendriver/src/browser.rs` (≈1796, ≈2017) — pass the new value from
  `resolve_fingerprint` into `StealthObserver`.
- **(conditional, only if §4.4 forks to Fetch):** `Cargo.toml` feature wiring +
  interception-gated apply path.

## 6. Testing

- **`accept_encoding_for`**: `122 → "gzip, deflate, br"`, `123 → "…, zstd"`,
  `140 → "…, zstd"` boundary table.
- **Skew detection** (`resolve_fingerprint`): binary 143 + `.chrome_version(120)`
  → `Some("gzip, deflate, br")`; binary 143 + `.chrome_version(140)` → `None`
  (both ≥123); no override → `None`.
- **Observer sequence** (extend `observer.rs` `MockConnection` tests): with a
  `Some` override, the spoofed page target emits `Network.enable` +
  `Network.setExtraHTTPHeaders` (assert the header value); with `None`, neither
  command appears.
- **CDP probe** (manual / `#[ignore]` real-Chrome): confirm clean replacement —
  the empirical gate in §4.4. Documents the observed behavior in a test comment.

## 7. Cargo / CI gates

- `cargo fmt --all`; `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- `cargo test -p zendriver-stealth`.
- If §4.4 forks to Fetch: `cargo clippy -p zendriver-mcp --all-features
  --all-targets -- -D warnings` and the interception-feature build.
- **Public-API:** the intended change is `pub(crate)`-internal (no `zendriver`
  public surface delta) → no baseline regen expected. If the `resolve_fingerprint`
  signature change turns out to be `pub`-visible, run the
  `public-api` check (`CLAUDE.md`) and regenerate the baseline.

## 8. MCP coverage

No new public API and no new capability type → **no new tool, no ledger entry.**
The behavior rides transitively on the existing stealth/launch tools: an MCP
client that pins `chrome_version` already exercises the skew path. (If §4.4 forks
to an interception-gated path, confirm the MCP interception feature still builds;
still no new tool.)

## 9. Assumptions (delegate-mode judgement calls — **review checkpoint**)

1. **A1 — Reframe: do not port the header Bayesian network.** A real Chrome over
   CDP already emits coherent `sec-ch-ua*`, `Accept`, order, and casing; the BN's
   value is for *browser-less* HTTP clients, which zendriver is not. Porting 808 KB
   of monthly-churning data to drive headers that are redundant (and, for `Accept`
   / `sec-ch-ua`, *harmful* to force) is rejected in favor of the deterministic
   `Accept-Encoding` fix. **This is the call most worth your veto** — if you want
   the faithful port purely for browserforge parity, say so and I'll respec around
   #25's `Generator` + a `HeaderProfile` carrier + an MCP tool.
2. **A2 — `Accept-Encoding` is the only header we override.** Justified per-header
   in §3 (uniformity, skew, safe-global-set).
3. **A3 — Deterministic by-major, not BN-sampled.** A by-major constant is *more*
   coherent than a random BN sample and needs no download. Two-branch rule on the
   `zstd`/123 boundary.
4. **A4 — Override only on proven skew** (claimed vs binary straddle the boundary),
   so the default profile adds zero CDP traffic. Unknown-binary paths (full
   `Fingerprint` override, connect-without-exe) → no injection.
5. **A5 — `sec-ch-ua*` left untouched** — already coherent from the UA-CH metadata
   the observer sets; injecting BN values would *introduce* mismatch.
6. **A6 — Header order / casing not addressed** — Chrome's native order already
   matches `headers-order.json`; `setExtraHTTPHeaders` cannot reorder and HTTP/2
   forces lowercase. Not controllable via the lightweight path; no benefit for a
   real Chrome.
7. **A7 — `DNT` not injected** — default-absent is the common, less-suspicious case.
8. **A8 — Application surface decided empirically (§4.4):** `setExtraHTTPHeaders`
   first; fall back to the interception/`Fetch` path (feature-gated) only if the
   probe shows it appends rather than replaces.
9. **A9 — No new carrier type.** The issue's "extend `Persona` vs new
   `HeaderProfile`" question is moot: a single derived string lives in the resolve
   path, not a sampled profile object.

## 10. Out of scope (recorded so #38's close is auditable)

- Faithful header-network port + `HeaderProfile` carrier + generative-headers MCP
  tool (rejected, A1). If revived, it is a separate spec.
- Header **order** / name **casing** spoofing (A6).
- `sec-ch-ua*`, `Accept`, `Sec-Fetch-*`, `DNT`, `te`, `Connection`,
  `upgrade-insecure-requests` injection (§3).
- A general user-facing "extra HTTP headers" builder API (`.extra_http_headers()`).
  Reasonable future ergonomics, but speculative for #38 — YAGNI.
- Geo-IP-derived `Accept-Language` (separate issue
  [#39](https://github.com/TurtIeSocks/zendriver-rs/issues/39)).
- Mobile / non-Chrome personas.

## 11. Risks

- **`setExtraHTTPHeaders` append-vs-replace** (§4.4) — the one real unknown;
  retired by the probe before committing to the application surface.
- **`Accept-Encoding` value drift** — if a future Chrome changes its default
  encoding set, the two-branch rule needs a new boundary. Low frequency
  (`zstd` was the first change since `br` in 2016); cheap to extend; covered by a
  boundary test.
- **Narrow benefit** — the fix only bites when a user pins an identity across the
  `zstd` boundary. That is exactly the case that is otherwise silently
  incoherent, and the cost on the default path is zero. Accepted.
