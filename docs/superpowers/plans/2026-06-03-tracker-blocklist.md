# Tracker/Fingerprinter Blocklist (#2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opt-in blocking of third-party fingerprinter/tracker hosts. A bundled, license-clean curated list plus runtime BYO (inline domains / local file / URL), surfaced through `BrowserBuilder` and `browser_open`, blocking matched hosts via the existing `Fetch.failRequest { BlockedByClient }` path on every tab.

**Architecture:** A pure `HostMatcher` (set + parent-domain walk) and a new `Rule::BlockHosts` variant live in `zendriver-interception` (no data, no network). `zendriver` core owns the bundled `trackers.txt`, the opt-in builder API, the runtime fetch+cache, and wiring: it builds one `Arc<HostMatcher>` at launch and installs a `Rule::BlockHosts` interception on the main tab and on every new tab (via the `TabRegistrar`). `zendriver-mcp` extends `browser_open`. All of it sits behind a new `tracker-blocking` cargo feature that implies `interception`.

**Tech Stack:** Rust (`edition 2024`, MSRV 1.85). New deps in core under the feature: `reqwest`, `dirs` (both already workspace deps). `tokio`/`serde`/`schemars` already present.

**Spec:** `docs/superpowers/specs/2026-06-03-stealth-tostring-masking-tracker-blocklist-design.md` §4. Handoff: `docs/superpowers/plans/2026-06-03-tracker-blocklist-handoff.md`. Style reference: `docs/superpowers/plans/2026-06-03-stealth-tostring-masking.md`.

**Locked decisions (do not relitigate — from brainstorm, spec §4):** host-only matching, suffix-on-dot, unconditional (no first-party exemption in v1); reuse `failRequest(BlockedByClient)`; curated list = third-party *passive* fingerprinters + cross-site trackers ONLY, **excluding** active anti-bot vendors (DataDome / Cloudflare / PerimeterX / Imperva / Akamai Bot Manager / Kasada / Arkose / hCaptcha / reCAPTCHA); ship our own clean list, never vendor a use-restricted one; runtime fetch for `_url`.

---

## File Structure

| File | Responsibility | Action |
|------|----------------|--------|
| `crates/zendriver-interception/src/host_matcher.rs` | `HostMatcher` (set + dot-boundary parent walk) + `host_of(url)` extractor | **Create** |
| `crates/zendriver-interception/src/lib.rs` | Export `host_matcher` module + `HostMatcher` | Modify |
| `crates/zendriver-interception/src/rule.rs` | `Rule::BlockHosts` variant + `matches` arm + `Debug` arm + unit test | Modify |
| `crates/zendriver-interception/src/actor.rs` | Dispatch `BlockHosts` → `fail_request(BlockedByClient)`; actor test | Modify |
| `crates/zendriver-interception/src/builder.rs` | `InterceptBuilder::block_hosts(Arc<HostMatcher>)` | Modify |
| `crates/zendriver/src/trackers.txt` | Bundled curated host list (authored by us) | **Create** |
| `crates/zendriver/src/tracker.rs` | Bundled-list parser, `load_or_download_blocklist`, cache path | **Create** |
| `crates/zendriver/Cargo.toml` | `tracker-blocking` feature + `reqwest`/`dirs` optional deps | Modify |
| `crates/zendriver/src/browser.rs` | Builder fields+methods, `build_tracker_matcher`, `BrowserInner` fields, `FinishConnect` plumbing, main-tab + per-tab (`TabRegistrar`) install | Modify |
| `crates/zendriver/src/lib.rs` | Re-export `HostMatcher`; declare `mod tracker` | Modify |
| `crates/zendriver/tests/tracker_blocklist_integration.rs` | Real-Chrome `#[ignore]` test | **Create** |
| `crates/zendriver-mcp/Cargo.toml` | `tracker-blocking` feature (+ default) | Modify |
| `crates/zendriver-mcp/src/tools/lifecycle.rs` | `OpenInput.block_trackers` + `tracker_blocklist`; builder wiring | Modify |
| `crates/zendriver-mcp/mcp-coverage-ledger.toml` | Cover builder methods; exclude internals | Modify |
| `crates/zendriver-mcp/public-api-baseline.txt` | Regenerated baseline | Modify (regen) |
| `crates/zendriver-mcp/tests/snapshots/*.snap` | Regenerated schema snapshots | Modify (regen) |

---

## Task 1: `HostMatcher` + `host_of` (pure mechanism)

**Files:**
- Create: `crates/zendriver-interception/src/host_matcher.rs`
- Modify: `crates/zendriver-interception/src/lib.rs`
- Test: `crates/zendriver-interception/src/host_matcher.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write `host_matcher.rs` with failing tests first**

Create `crates/zendriver-interception/src/host_matcher.rs`. Start with the tests (they reference `HostMatcher` / `host_of` which don't exist yet):

```rust
//! Host-set matcher for the tracker/fingerprinter blocklist.
//!
//! Pure mechanism — no data, no network. `zendriver` core supplies the host
//! list (bundled `trackers.txt` and/or user sources) and builds one
//! [`HostMatcher`], shared across tabs via [`std::sync::Arc`]. Matching is
//! host-only, case-insensitive, and suffix-on-dot: a listed `evil.com` blocks
//! `evil.com` and every `*.evil.com` subdomain, but never `notevil.com`.

use std::collections::HashSet;

/// A set of blocked hosts with parent-domain suffix matching.
#[derive(Debug, Clone, Default)]
pub struct HostMatcher {
    hosts: HashSet<String>,
}

impl HostMatcher {
    /// Build from an iterator of domains. Each is trimmed, lower-cased, and
    /// stripped of a trailing dot; empties are dropped.
    #[must_use]
    pub fn new(domains: impl IntoIterator<Item = String>) -> Self {
        let hosts = domains
            .into_iter()
            .map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|d| !d.is_empty())
            .collect();
        Self { hosts }
    }

    /// True if `host` is listed exactly, or is a subdomain of a listed host
    /// (suffix-on-dot). `a.b.evil.com` matches a listed `evil.com`;
    /// `notevil.com` does not.
    #[must_use]
    pub fn is_blocked(&self, host: &str) -> bool {
        if self.hosts.is_empty() {
            return false;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.hosts.contains(&host) {
            return true;
        }
        // Strip the leftmost label repeatedly: a.b.evil.com -> b.evil.com -> evil.com.
        let mut rest = host.as_str();
        while let Some((_, parent)) = rest.split_once('.') {
            if self.hosts.contains(parent) {
                return true;
            }
            rest = parent;
        }
        false
    }

    /// Number of distinct listed hosts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    /// True if no hosts are listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}

/// Extract the host (no port, no userinfo) from a URL string.
///
/// Deliberately dependency-free — this crate stays parser/network-light, and
/// `Fetch.requestPaused` only surfaces `http(s)`/`ws(s)` URLs (which always
/// carry an authority). Returns `None` when no non-empty host is present.
pub(crate) fn host_of(url: &str) -> Option<&str> {
    // Drop the scheme (`https://`); tolerate scheme-relative `//host`.
    let after_scheme = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => url.strip_prefix("//").unwrap_or(url),
    };
    // Authority ends at the first `/`, `?`, or `#`.
    let authority = after_scheme
        .find(['/', '?', '#'])
        .map_or(after_scheme, |i| &after_scheme[..i]);
    // Strip userinfo (`user:pass@host`).
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // Strip the port. IPv6 literals are bracketed (`[::1]:443`) — keep the
    // bracketed host and drop a trailing `:port`.
    let host = if host_port.starts_with('[') {
        match host_port.find(']') {
            Some(end) => &host_port[..=end],
            None => host_port,
        }
    } else if let Some(i) = host_port.find(':') {
        &host_port[..i]
    } else {
        host_port
    };
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(m.is_blocked("evil.com"));
    }

    #[test]
    fn subdomain_walk_matches() {
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(m.is_blocked("a.b.evil.com"));
        assert!(m.is_blocked("tracker.evil.com"));
    }

    #[test]
    fn unrelated_host_misses() {
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(!m.is_blocked("good.com"));
    }

    #[test]
    fn partial_label_is_not_a_match() {
        // `notevil.com` must NOT be blocked by a listed `evil.com` — matching
        // is on dot boundaries, not raw suffixes.
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(!m.is_blocked("notevil.com"));
    }

    #[test]
    fn does_not_over_match_parent_tld() {
        // Listing `evil.com` must not block the bare root `com`.
        let m = HostMatcher::new(["evil.com".to_string()]);
        assert!(!m.is_blocked("com"));
    }

    #[test]
    fn case_insensitive_and_trailing_dot() {
        let m = HostMatcher::new(["Evil.COM".to_string()]);
        assert!(m.is_blocked("EVIL.com"));
        assert!(m.is_blocked("evil.com."));
        assert!(m.is_blocked("a.EVIL.com."));
    }

    #[test]
    fn empty_matcher_blocks_nothing() {
        let m = HostMatcher::new(Vec::<String>::new());
        assert!(m.is_empty());
        assert!(!m.is_blocked("evil.com"));
    }

    #[test]
    fn host_of_extracts_host() {
        assert_eq!(host_of("https://a.b.evil.com/path?x=1#y"), Some("a.b.evil.com"));
        assert_eq!(host_of("http://user:pass@evil.com:8080/"), Some("evil.com"));
        assert_eq!(host_of("//cdn.example.com/x"), Some("cdn.example.com"));
        assert_eq!(host_of("evil.com/x"), Some("evil.com"));
        assert_eq!(host_of("https://[::1]:443/x"), Some("[::1]"));
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

In `crates/zendriver-interception/src/lib.rs`, add the module declaration alongside the others (after `pub mod actor;` … block):

```rust
pub mod host_matcher;
```

and add to the `pub use` exports (next to `pub use rule::Rule;`):

```rust
pub use host_matcher::HostMatcher;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p zendriver-interception host_matcher:: --lib`
Expected: PASS — all 8 tests green.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-interception/src/host_matcher.rs crates/zendriver-interception/src/lib.rs
git commit -m "feat(interception): HostMatcher + host_of for tracker blocklist"
```

---

## Task 2: `Rule::BlockHosts` variant + actor dispatch + `block_hosts` builder

**Files:**
- Modify: `crates/zendriver-interception/src/rule.rs`
- Modify: `crates/zendriver-interception/src/actor.rs`
- Modify: `crates/zendriver-interception/src/builder.rs`
- Test: inline in `rule.rs` and `actor.rs`

- [ ] **Step 1: Write the failing rule test**

Add to the `tests` module in `rule.rs`:

```rust
#[test]
fn block_hosts_matches_on_host_and_subdomain() {
    use crate::host_matcher::HostMatcher;
    let rule = Rule::BlockHosts {
        matcher: Arc::new(HostMatcher::new(["evil.com".to_string()])),
    };
    assert!(rule.matches("https://evil.com/track.js"));
    assert!(rule.matches("https://a.b.evil.com/x?y=1"));
    assert!(!rule.matches("https://good.com/app.js"));
    assert!(!rule.matches("https://notevil.com/app.js"));

    // Debug renders the variant name + the host count (the Arc<HostMatcher>
    // is summarized, not dumped).
    let dbg = format!("{rule:?}");
    assert!(dbg.contains("BlockHosts"), "got: {dbg}");
    assert!(dbg.contains("hosts"), "got: {dbg}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zendriver-interception rule::tests::block_hosts_matches_on_host_and_subdomain --lib`
Expected: FAIL — `Rule::BlockHosts` does not exist.

- [ ] **Step 3: Add the `BlockHosts` variant + import**

In `rule.rs`, add the import near the top (after `use crate::url_pattern::UrlPattern;`):

```rust
use crate::host_matcher::HostMatcher;
```

Add the variant to `enum Rule` (after the `ModifyResponse { … }` variant, before the closing `}`):

```rust
    /// Abort requests whose **host** is in a [`HostMatcher`] with
    /// `Fetch.failRequest { errorReason: "BlockedByClient" }` — the same
    /// `net::ERR_BLOCKED_BY_CLIENT` a real adblocker / Brave raises.
    ///
    /// Unlike [`Block`](Rule::Block), which globs the full URL, this matches
    /// host-set membership (exact + parent-domain suffix on dot boundaries),
    /// so a curated list of thousands of hosts is one O(1) set lookup per
    /// request rather than N glob comparisons. Powers the tracker blocklist.
    BlockHosts {
        /// Shared host set; cheap to clone across tabs/rules.
        matcher: Arc<HostMatcher>,
    },
```

Add the `matches` arm — append a new match arm in `Rule::matches` (the host
extraction differs from the pattern-based arms, so it can't fold in):

```rust
    pub fn matches(&self, url: &str) -> bool {
        match self {
            Self::Block { pattern }
            | Self::Respond { pattern, .. }
            | Self::Modify { pattern, .. }
            | Self::ModifyResponse { pattern, .. } => pattern.matches(url),
            Self::Redirect { from, .. } => from.matches(url),
            Self::BlockHosts { matcher } => {
                crate::host_matcher::host_of(url).is_some_and(|h| matcher.is_blocked(h))
            }
        }
    }
```

Add the `Debug` arm — append to the `match self` in `impl fmt::Debug for Rule`:

```rust
            Self::BlockHosts { matcher } => f
                .debug_struct("BlockHosts")
                .field("hosts", &matcher.len())
                .finish(),
```

- [ ] **Step 4: Run the rule test**

Run: `cargo test -p zendriver-interception rule:: --lib`
Expected: PASS — new test + existing rule tests green.

- [ ] **Step 5: Add the actor dispatch arm + failing actor test**

In `actor.rs`, fold `BlockHosts` into the existing `Block` dispatch arm (identical action) — change line ~324:

```rust
        Some(Rule::Block { .. }) | Some(Rule::BlockHosts { .. }) => {
            fail_request(session, &ev.request_id, "BlockedByClient").await
        }
```

Add this actor test to the `tests` module in `actor.rs` (it mirrors
`block_rule_dispatches_fail_request_with_blocked_by_client`, swapping the rule
and the request URL):

```rust
    /// Same end-to-end drive as the `Block` test, but the rule is a
    /// `BlockHosts` matcher and the request matches by host (subdomain walk).
    #[tokio::test]
    async fn block_hosts_rule_dispatches_fail_request() {
        use crate::host_matcher::HostMatcher;
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let rules = vec![Rule::BlockHosts {
            matcher: std::sync::Arc::new(HostMatcher::new(["evil.com".to_string()])),
        }];
        let patterns = vec![RequestPattern {
            url_pattern: Some("*".into()),
            ..RequestPattern::default()
        }];
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let actor_cancel = cancel.clone();
        let _actor = tokio::spawn(async move {
            run_actor(sess, rules, patterns, None, actor_cancel, done_tx).await;
        });

        let enable_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.enable"))
                .await
                .expect("actor did not send Fetch.enable within 2s");
        mock.reply(enable_id, json!({})).await;

        // Subdomain of a listed host → must be failed.
        mock.emit_event_for_session(
            "Fetch.requestPaused",
            json!({
                "requestId": "REQ-1",
                "request": {
                    "url": "https://cdn.evil.com/fp.js",
                    "method": "GET",
                    "headers": {},
                },
                "resourceType": "Script",
            }),
            "S1",
        )
        .await;

        let fail_id =
            tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Fetch.failRequest"))
                .await
                .expect("actor did not send Fetch.failRequest within 2s");
        let fail_params = mock.last_sent()["params"].clone();
        assert_eq!(fail_params["requestId"], "REQ-1");
        assert_eq!(fail_params["errorReason"], "BlockedByClient");
        mock.reply(fail_id, json!({})).await;

        cancel.cancel();
        let _ = done_rx.await;
    }
```

- [ ] **Step 6: Add `block_hosts` to the builder**

In `builder.rs`, add the import (near the other `use crate::` lines):

```rust
use crate::host_matcher::HostMatcher;
```

Add the method to `impl<'tab> InterceptBuilder<'tab>` (after `block`):

```rust
    /// Register a [`Rule::BlockHosts`] backed by `matcher`.
    ///
    /// Every request whose host is in `matcher` (exact, or a parent domain on
    /// a dot boundary) is failed with `BlockedByClient`. Composes with other
    /// rules in registration order. `zendriver` core's tracker-blocklist
    /// wiring uses this; most callers reach it via `BrowserBuilder::block_trackers`.
    #[must_use]
    pub fn block_hosts(mut self, matcher: Arc<HostMatcher>) -> Self {
        self.rules.push(Rule::BlockHosts { matcher });
        self
    }
```

- [ ] **Step 7: Run the interception suite**

Run: `cargo test -p zendriver-interception --lib`
Expected: PASS — rule + actor + builder tests green; existing tests unaffected.

- [ ] **Step 8: Commit**

```bash
git add crates/zendriver-interception/src/rule.rs crates/zendriver-interception/src/actor.rs crates/zendriver-interception/src/builder.rs
git commit -m "feat(interception): Rule::BlockHosts variant + block_hosts builder"
```

---

## Task 3: Core feature, bundled list, parser, cache, builder API

**Files:**
- Modify: `crates/zendriver/Cargo.toml`
- Create: `crates/zendriver/src/trackers.txt`
- Create: `crates/zendriver/src/tracker.rs`
- Modify: `crates/zendriver/src/lib.rs`
- Modify: `crates/zendriver/src/browser.rs` (builder fields + methods + `build_tracker_matcher`)
- Test: inline in `tracker.rs` + `browser.rs`

- [ ] **Step 1: Add the `tracker-blocking` feature + deps**

In `crates/zendriver/Cargo.toml`, under `[features]` add (and extend `integration-tests` so the integration test in Task 6 compiles):

```toml
# Opt-in third-party tracker/fingerprinter host blocking (bundled curated
# list + runtime BYO). Implies interception; the bundled list (`include_str!`)
# only adds binary size when this feature is on.
tracker-blocking = ["interception", "dep:reqwest", "dep:dirs"]
```

Change the existing `integration-tests` line to append `tracker-blocking`:

```toml
integration-tests = ["dep:wiremock", "dep:serial_test", "interception", "expect", "cloudflare", "imperva", "datadome", "monitor", "tracker-blocking"]
```

Under `[dependencies]`, add the two optional deps (both are workspace deps already):

```toml
reqwest                       = { workspace = true, optional = true }
dirs                          = { workspace = true, optional = true }
```

- [ ] **Step 2: Author the bundled `trackers.txt`**

Create `crates/zendriver/src/trackers.txt` with exactly this content (authored
by us → license-clean; passive trackers/fingerprinters only, no anti-bot
challenge vendors):

```text
# zendriver-rs bundled tracker/fingerprinter blocklist
#
# Curation principle (spec §4.4): third-party PASSIVE fingerprinters and
# cross-site trackers ONLY. We DELIBERATELY EXCLUDE active anti-bot challenge
# providers (DataDome, Cloudflare challenge, PerimeterX/HUMAN, Imperva/
# Incapsula, Akamai Bot Manager, Kasada, Arkose/FunCaptcha, hCaptcha,
# reCAPTCHA) — blocking those breaks access to the very sites we want to reach.
#
# Authored by the zendriver-rs project (no third-party list vendored), so the
# bundle carries no upstream license. Point `tracker_blocklist_url` at an
# external list under your own acceptance of its terms for broader coverage.
#
# Format: one host per line; `#` comments and blank lines ignored. Matching is
# host-only, case-insensitive, suffix-on-dot (a listed `evil.com` also blocks
# `*.evil.com`).

# -- Device fingerprinting / fraud-device services (passive) --------------
fingerprintjs.com
fpjs.io
fpcdn.io
api.fpjs.io
online-metrix.net
iesnare.com
iovation.com
sift.com
cdn.sift.com
seon.io
augur.io
deviceatlas.com
device.maxmind.com

# -- Web analytics (cross-site) -------------------------------------------
google-analytics.com
ssl.google-analytics.com
analytics.google.com
googletagmanager.com
app-measurement.com
mc.yandex.ru
scorecardresearch.com
quantserve.com
quantcount.com
chartbeat.com
chartbeat.net
heapanalytics.com
mixpanel.com
cdn.mxpnl.com
segment.com
segment.io
cdn.segment.com
amplitude.com
api.amplitude.com
api2.amplitude.com
cdn.amplitude.com
kissmetrics.io
kissmetrics.com
woopra.com
statcounter.com

# -- Session-replay / behavior fingerprinting -----------------------------
hotjar.com
hotjar.io
fullstory.com
mouseflow.com
crazyegg.com
inspectlet.com
smartlook.com
logrocket.io
logrocket.com
clarity.ms
luckyorange.com
luckyorange.net

# -- Cross-site ad / RTB / DMP trackers -----------------------------------
doubleclick.net
stats.g.doubleclick.net
googlesyndication.com
googleadservices.com
adnxs.com
criteo.com
criteo.net
taboola.com
outbrain.com
pubmatic.com
rubiconproject.com
openx.net
adsrvr.org
casalemedia.com
33across.com
bluekai.com
demdex.net
everesttech.net
omtrdc.net
2o7.net
rlcdn.com
crwdcntrl.net
agkn.com
addthis.com
sharethis.com

# -- Social pixels (cross-site) -------------------------------------------
connect.facebook.net
bat.bing.com
sc-static.net
tr.snapchat.com
analytics.tiktok.com
ct.pinterest.com
static.ads-twitter.com
analytics.twitter.com
snap.licdn.com
px.ads.linkedin.com
```

- [ ] **Step 3: Write `tracker.rs` with failing tests first**

Create `crates/zendriver/src/tracker.rs`:

```rust
//! Tracker/fingerprinter blocklist sourcing for the `tracker-blocking`
//! feature.
//!
//! Holds the bundled curated list (`trackers.txt`), a tolerant parser, and a
//! download-on-first-use cache for `tracker_blocklist_url` sources. The pure
//! matching lives in `zendriver-interception` ([`HostMatcher`]); this module
//! only produces the host strings that feed it.
//!
//! [`HostMatcher`]: zendriver_interception::HostMatcher

use std::path::PathBuf;

/// Bundled curated list, embedded at compile time (feature-gated, so it only
/// costs binary size when `tracker-blocking` is enabled).
const BUNDLED: &str = include_str!("trackers.txt");

/// Parse the bundled list into hosts.
pub(crate) fn bundled_hosts() -> Vec<String> {
    parse_blocklist(BUNDLED)
}

/// Parse a blocklist text into hostnames.
///
/// Tolerant of two common formats so user `_file`/`_url` sources work as-is:
/// plain `host` per line, and hosts-file `0.0.0.0 host` / `127.0.0.1 host`
/// (the last whitespace token on the line is taken as the host). `#` starts a
/// comment (whole-line or inline); blank lines are ignored. Hosts are
/// lower-cased.
pub(crate) fn parse_blocklist(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(|line| line.split_whitespace().last().unwrap_or(line).to_ascii_lowercase())
        .collect()
}

/// Cache file path for a downloaded `_url` source — keyed by a hash of the URL
/// under the same cache root as `zendriver-fetcher`/`zendriver-fingerprints`.
fn cache_path(url: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zendriver/trackers")
        .join(format!("{:016x}.txt", h.finish()))
}

/// Load a host list from local cache, or download from `url` and cache it.
///
/// Mirrors the atomic-write download-on-first-use pattern in
/// `zendriver-fingerprints` `pool::load_or_download` (write `<path>.tmp`, then
/// `rename`). `reqwest`/IO failures are surfaced as [`std::io::Error`] so the
/// caller folds them into `ZendriverError::Io` without a new public error
/// variant.
pub(crate) async fn load_or_download_blocklist(url: &str) -> Result<Vec<String>, std::io::Error> {
    let cache = cache_path(url);

    // Fast path: cache hit.
    if let Ok(text) = std::fs::read_to_string(&cache) {
        tracing::debug!(path = %cache.display(), "tracker blocklist cache hit");
        return Ok(parse_blocklist(&text));
    }

    tracing::debug!(url, "tracker blocklist cache miss — downloading");
    let body = reqwest::get(url)
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(std::io::Error::other)?
        .text()
        .await
        .map_err(std::io::Error::other)?;

    // Atomic write: tmp -> rename.
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = cache.with_extension("tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, &cache)?;

    tracing::debug!(path = %cache.display(), "tracker blocklist cached");
    Ok(parse_blocklist(&body))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parser_ignores_comments_and_blanks() {
        let text = "# header\n\n  evil.com  \nbad.net # inline comment\n   \n";
        let hosts = parse_blocklist(text);
        assert_eq!(hosts, vec!["evil.com".to_string(), "bad.net".to_string()]);
    }

    #[test]
    fn parser_accepts_hosts_file_format() {
        // uBlock / Peter Lowe's hosts format: "0.0.0.0 host" or "127.0.0.1 host".
        let text = "0.0.0.0 tracker.com\n127.0.0.1 fp.example.org\n";
        let hosts = parse_blocklist(text);
        assert_eq!(
            hosts,
            vec!["tracker.com".to_string(), "fp.example.org".to_string()]
        );
    }

    #[test]
    fn parser_lowercases() {
        assert_eq!(parse_blocklist("EVIL.COM\n"), vec!["evil.com".to_string()]);
    }

    #[test]
    fn bundled_list_parses_to_many_hosts() {
        let hosts = bundled_hosts();
        // Spec target is ~50-150; assert a sane floor + a couple of known
        // entries are present and a known anti-bot vendor is ABSENT.
        assert!(hosts.len() >= 50, "bundled list too small: {}", hosts.len());
        assert!(hosts.iter().any(|h| h == "fingerprintjs.com"));
        assert!(hosts.iter().any(|h| h == "doubleclick.net"));
        // Curation principle: no active anti-bot challenge vendors.
        for banned in ["datadome.co", "hcaptcha.com", "perimeterx.net", "imperva.com"] {
            assert!(
                !hosts.iter().any(|h| h == banned),
                "anti-bot vendor {banned} must NOT be in the bundled list"
            );
        }
    }
}
```

- [ ] **Step 4: Declare the module in `lib.rs` + re-export `HostMatcher`**

In `crates/zendriver/src/lib.rs`, declare the module near the other `mod`
declarations:

```rust
#[cfg(feature = "tracker-blocking")]
mod tracker;
```

Add `HostMatcher` to the interception re-export list (the `#[cfg(feature = "interception")] pub use zendriver_interception::{ … }` block):

```rust
#[cfg(feature = "interception")]
pub use zendriver_interception::{
    AbortReason, HostMatcher, InterceptBuilder, InterceptHandle, InterceptionError, PausedRequest,
    RequestInfo, RequestOverrides, RequestStage, ResourceType, ResponseInfo, ResponseOverrides,
};
```

- [ ] **Step 5: Run the parser tests**

Run: `cargo test -p zendriver --features tracker-blocking tracker:: --lib`
Expected: PASS — parser + bundled-list tests green. (If `bundled_list_parses_to_many_hosts` fails on count, you mis-edited `trackers.txt`.)

- [ ] **Step 6: Add builder fields + methods + `build_tracker_matcher`**

In `browser.rs`, add fields to `struct BrowserBuilder` (near `proxy_auth` at
~line 376), gated on the feature:

```rust
    /// Enable the bundled curated tracker/fingerprinter blocklist.
    #[cfg(feature = "tracker-blocking")]
    pub(crate) block_trackers: bool,
    /// Extra hostnames to block (inline), accumulated across calls.
    #[cfg(feature = "tracker-blocking")]
    pub(crate) tracker_blocklist_domains: Vec<String>,
    /// Local files (newline host lists) to block, accumulated across calls.
    #[cfg(feature = "tracker-blocking")]
    pub(crate) tracker_blocklist_files: Vec<std::path::PathBuf>,
    /// Remote URLs (newline host lists, fetched+cached at launch).
    #[cfg(feature = "tracker-blocking")]
    pub(crate) tracker_blocklist_urls: Vec<String>,
```

Add the field defaults to the `Default`/builder-construction site (near line
434 where `stealth: Some(StealthProfile::native())` is set):

```rust
            #[cfg(feature = "tracker-blocking")]
            block_trackers: false,
            #[cfg(feature = "tracker-blocking")]
            tracker_blocklist_domains: Vec::new(),
            #[cfg(feature = "tracker-blocking")]
            tracker_blocklist_files: Vec::new(),
            #[cfg(feature = "tracker-blocking")]
            tracker_blocklist_urls: Vec::new(),
```

Add the builder methods to `impl BrowserBuilder` (near `preference`/`stealth`,
~line 800–900), all gated:

```rust
    /// Enable the bundled curated tracker/fingerprinter blocklist for this
    /// browser. Blocks third-party passive fingerprinters and cross-site
    /// trackers (host-only, suffix-on-dot) by failing their requests with
    /// `net::ERR_BLOCKED_BY_CLIENT` — the same error a real adblocker raises.
    ///
    /// Opt-in (off by default — least-opinionated). Combine with
    /// [`tracker_blocklist_add`](Self::tracker_blocklist_add) /
    /// [`tracker_blocklist_file`](Self::tracker_blocklist_file) /
    /// [`tracker_blocklist_url`](Self::tracker_blocklist_url) for custom hosts.
    #[cfg(feature = "tracker-blocking")]
    #[must_use]
    pub fn block_trackers(mut self, enable: bool) -> Self {
        self.block_trackers = enable;
        self
    }

    /// Add inline hostnames to the tracker blocklist. Supplying any custom
    /// source implicitly enables blocking (you do not also need
    /// [`block_trackers(true)`](Self::block_trackers) unless you also want the
    /// bundled list). Repeatable.
    #[cfg(feature = "tracker-blocking")]
    #[must_use]
    pub fn tracker_blocklist_add(mut self, domains: impl IntoIterator<Item = String>) -> Self {
        self.tracker_blocklist_domains.extend(domains);
        self
    }

    /// Add a local file (newline-delimited host list; `#` comments and
    /// hosts-file `0.0.0.0 host` lines tolerated) to the tracker blocklist.
    /// Implicitly enables blocking. Read at launch. Repeatable.
    #[cfg(feature = "tracker-blocking")]
    #[must_use]
    pub fn tracker_blocklist_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.tracker_blocklist_files.push(path.into());
        self
    }

    /// Add a remote URL (newline-delimited host list) to the tracker
    /// blocklist. Fetched once at launch and cached on disk
    /// (download-on-first-use). Implicitly enables blocking. Repeatable.
    ///
    /// Use this to point at an external list (uBlock, Peter Lowe's, …) under
    /// your own acceptance of that list's license — the bundle ships only our
    /// own clean list.
    #[cfg(feature = "tracker-blocking")]
    #[must_use]
    pub fn tracker_blocklist_url(mut self, url: impl Into<String>) -> Self {
        self.tracker_blocklist_urls.push(url.into());
        self
    }
```

Add the matcher-builder method to `impl BrowserBuilder` (gated):

```rust
    /// Build the combined [`HostMatcher`] from all configured sources, or
    /// `None` if nothing was requested. Called once at launch.
    #[cfg(feature = "tracker-blocking")]
    async fn build_tracker_matcher(
        &self,
    ) -> Result<Option<std::sync::Arc<HostMatcher>>, ZendriverError> {
        let mut domains: Vec<String> = Vec::new();
        if self.block_trackers {
            domains.extend(crate::tracker::bundled_hosts());
        }
        domains.extend(self.tracker_blocklist_domains.iter().cloned());
        for path in &self.tracker_blocklist_files {
            let text = std::fs::read_to_string(path)?; // -> ZendriverError::Io
            domains.extend(crate::tracker::parse_blocklist(&text));
        }
        for url in &self.tracker_blocklist_urls {
            let hosts = crate::tracker::load_or_download_blocklist(url).await?; // -> Io
            domains.extend(hosts);
        }
        if domains.is_empty() {
            return Ok(None);
        }
        Ok(Some(std::sync::Arc::new(HostMatcher::new(domains))))
    }
```

(The `HostMatcher` symbol is in scope via the crate-root re-export from Step 4;
if `browser.rs` does not already glob the crate root, add `use crate::HostMatcher;`
under a `#[cfg(feature = "tracker-blocking")]` near the other imports.)

- [ ] **Step 7: Write a builder-accumulation unit test**

Add to the inline `#[cfg(test)]` module in `browser.rs` (find the existing
`mod tests` block). This test is gated so it only compiles with the feature:

```rust
    #[cfg(feature = "tracker-blocking")]
    #[tokio::test]
    async fn tracker_sources_accumulate_and_build_a_matcher() {
        let b = Browser::builder()
            .tracker_blocklist_add(["custom-tracker.test".to_string()])
            .tracker_blocklist_add(["another.test".to_string()]);
        let matcher = b.build_tracker_matcher().await.unwrap().expect("matcher built");
        assert!(matcher.is_blocked("custom-tracker.test"));
        assert!(matcher.is_blocked("sub.another.test"));
        assert!(!matcher.is_blocked("not-listed.test"));

        // No sources, no bundled toggle -> None (blocking stays off).
        let none = Browser::builder().build_tracker_matcher().await.unwrap();
        assert!(none.is_none());

        // Bundled toggle alone builds a matcher containing a known entry.
        let bundled = Browser::builder()
            .block_trackers(true)
            .build_tracker_matcher()
            .await
            .unwrap()
            .expect("bundled matcher");
        assert!(bundled.is_blocked("doubleclick.net"));
    }
```

- [ ] **Step 8: Run core unit tests**

Run: `cargo test -p zendriver --features tracker-blocking tracker:: --lib` and
`cargo test -p zendriver --features tracker-blocking tracker_sources_accumulate --lib`
Expected: PASS — parser, bundled-list, and builder-accumulation tests green.

- [ ] **Step 9: Commit**

```bash
git add crates/zendriver/Cargo.toml crates/zendriver/src/trackers.txt crates/zendriver/src/tracker.rs crates/zendriver/src/lib.rs crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): tracker-blocking feature, bundled list, builder API"
```

---

## Task 4: Wire the matcher onto the main tab and every new tab

**Files:**
- Modify: `crates/zendriver/src/browser.rs` (`BrowserInner` fields, `FinishConnect`, `finish_connect`, `TabRegistrar`, `launch`)

**Approach:** Store the `Arc<HostMatcher>` on `BrowserInner` (threaded in via
`FinishConnect`) and a `tracker_handles` map keyed by `session_id`. Install on
the main tab inside `finish_connect` (after the registrar weak-ref is wired),
and on every subsequently-attached page tab inside `TabRegistrar::on_target_attached`;
prune on detach. Mirrors the existing `proxy_auth_handle` lifetime pattern.

- [ ] **Step 1: Add `BrowserInner` fields**

In `browser.rs`, add to `struct BrowserInner` (next to `proxy_auth_handle` at
~line 1141), gated on the feature:

```rust
    /// Combined tracker/fingerprinter [`HostMatcher`] (bundled + custom),
    /// built once at launch. `None` when blocking is not configured. Read by
    /// the [`TabRegistrar`] to install a `BlockHosts` interception on each new
    /// page tab.
    #[cfg(feature = "tracker-blocking")]
    pub(crate) tracker_matcher: Option<std::sync::Arc<HostMatcher>>,
    /// Live tracker-blocking interception handles keyed by `sessionId`
    /// (main tab + each page tab). Inserted on attach, removed on detach;
    /// dropping a handle stops that tab's actor. Held here so the actors live
    /// as long as the browser.
    #[cfg(feature = "tracker-blocking")]
    pub(crate) tracker_handles:
        tokio::sync::Mutex<HashMap<String, zendriver_interception::InterceptHandle>>,
```

Add the field inits to **both** test constructors `test_only_inner_from_conn`
(at ~line 1209, next to the `proxy_auth_handle` init) — gated, defaulting to
no blocking:

```rust
            #[cfg(feature = "tracker-blocking")]
            tracker_matcher: None,
            #[cfg(feature = "tracker-blocking")]
            tracker_handles: tokio::sync::Mutex::new(HashMap::new()),
```

- [ ] **Step 2: Thread the matcher through `FinishConnect`**

Add the field to `struct FinishConnect` (~line 1369), gated:

```rust
    /// Combined tracker matcher built by `launch` (`None` for `connect` and
    /// when blocking is unconfigured). Stored on `BrowserInner` and installed
    /// on the main tab + future tabs.
    #[cfg(feature = "tracker-blocking")]
    pub(crate) tracker_matcher: Option<std::sync::Arc<HostMatcher>>,
```

Set it on the `BrowserInner` built inside `finish_connect` (the `Arc::new_cyclic`
block, ~line 1470–1500) — destructure `tracker_matcher` from the `FinishConnect`
binding and move it in, plus init the handles map:

```rust
            #[cfg(feature = "tracker-blocking")]
            tracker_matcher,
            #[cfg(feature = "tracker-blocking")]
            tracker_handles: tokio::sync::Mutex::new(HashMap::new()),
```

(Add `tracker_matcher` to the `let FinishConnect { … } = args;` destructure at
the top of `finish_connect`, gated.)

- [ ] **Step 3: Install on the main tab inside `finish_connect`**

Immediately after `registrar.set_browser(Arc::downgrade(&inner));` (~line 1500),
add a gated block:

```rust
    // Install tracker blocking on the main tab's session (the main tab
    // attached before the registrar weak-ref existed, so the registrar skipped
    // it — install explicitly here, mirroring the proxy-auth wiring).
    #[cfg(feature = "tracker-blocking")]
    if let Some(matcher) = inner.tracker_matcher.clone() {
        let session = inner.main_tab.session().clone();
        let sid = session.session_id().to_string();
        let handle = zendriver_interception::InterceptBuilder::new(&session)
            .block_hosts(matcher)
            .start();
        inner.tracker_handles.lock().await.insert(sid, handle);
    }
```

- [ ] **Step 4: Install on each new page tab in `TabRegistrar`**

In `TabRegistrar::on_target_attached`, the `"page"` branch (~line 1292), after
the tab is inserted into the registry (`tabs.insert(...)` / `drop(tabs)` /
`notify_waiters()`), add a gated block. `browser` is the upgraded
`Arc<BrowserInner>`; `new_session` is already in scope:

```rust
                // Tracker blocking: if configured, install a BlockHosts
                // interception on this new page tab's session and park the
                // handle (keyed by sessionId) so it lives with the browser.
                #[cfg(feature = "tracker-blocking")]
                if let Some(matcher) = browser.tracker_matcher.clone() {
                    let handle =
                        zendriver_interception::InterceptBuilder::new(&new_session)
                            .block_hosts(matcher)
                            .start();
                    browser
                        .tracker_handles
                        .lock()
                        .await
                        .insert(session.session_id.to_string(), handle);
                }
```

> Note: `new_session` is moved into `Tab::new` earlier in the branch. Clone it
> for the interception install **before** the `Tab::new` call — change
> `let new_session = SessionHandle::new(conn, session.session_id.to_string());`
> so a clone is available, or move the `InterceptBuilder::new(&new_session)`
> install ahead of `Tab::new`. Either way, install off the same session the tab
> uses. The `start()` call only borrows the session, so cloning once and
> installing before `Tab::new(new_session, …)` is cleanest.

Prune on detach — in `TabRegistrar::on_target_detached`, after the tab removal
block (~line 1349), add:

```rust
        // Drop any tracker-blocking handle for this session (stops its actor).
        #[cfg(feature = "tracker-blocking")]
        {
            browser.tracker_handles.lock().await.remove(session_id);
        }
```

- [ ] **Step 5: Build the matcher in `launch` and pass it in**

In `BrowserBuilder::launch`, locate the `finish_connect(FinishConnect { … })`
call (~line 1934). Just before it, build the matcher; add the field to the
struct literal, both gated:

```rust
        #[cfg(feature = "tracker-blocking")]
        let tracker_matcher = self.build_tracker_matcher().await?;

        let inner = finish_connect(FinishConnect {
            // … existing fields …
            #[cfg(feature = "tracker-blocking")]
            tracker_matcher,
        })
        .await?;
```

In `BrowserBuilder::connect` (the other production `finish_connect` call,
~line 2084) and the **two test** call sites (~lines 4445, 4513), pass `None`:

```rust
            #[cfg(feature = "tracker-blocking")]
            tracker_matcher: None,
```

(`connect` ignores spawn-only builder fields; tracker blocking on the attach
path is a documented v1 non-goal.)

- [ ] **Step 6: Build the workspace (feature on + default) to confirm wiring compiles**

Run (parallel-safe, batch in one shell call):

```bash
cargo build -p zendriver --features tracker-blocking
cargo build -p zendriver
cargo test -p zendriver --features tracker-blocking --lib
```

Expected: both builds succeed; lib unit tests PASS. (Default build proves the
`#[cfg]` gating is correct — no tracker symbols leak when the feature is off.)

- [ ] **Step 7: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): install tracker BlockHosts on main + new tabs"
```

---

## Task 5: MCP `browser_open` surface

**Files:**
- Modify: `crates/zendriver-mcp/Cargo.toml`
- Modify: `crates/zendriver-mcp/src/tools/lifecycle.rs`

- [ ] **Step 1: Add the MCP feature**

In `crates/zendriver-mcp/Cargo.toml`, under `[features]`:

```toml
tracker-blocking  = ["zendriver/tracker-blocking"]
```

and add it to `default`:

```toml
default = ["interception", "expect", "cloudflare", "imperva", "datadome", "fetcher", "monitor", "tracker-blocking"]
```

- [ ] **Step 2: Add input fields + the `TrackerBlocklist` enum**

In `lifecycle.rs`, add the wire enum (near `OpenInput`, after the struct).
Match the internally-tagged style used elsewhere in MCP (`state.rs` uses
`#[serde(tag = "kind", rename_all = "snake_case")]`):

```rust
/// A custom tracker-blocklist source for `browser_open`.
///
/// One of: a remote `url` (fetched+cached at launch), a local `file` path, or
/// inline `domains`. Supplying any source implicitly enables blocking.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrackerBlocklist {
    /// Fetch + cache a newline-delimited host list from this URL.
    Url { url: String },
    /// Read a newline-delimited host list from this local file path.
    File { path: String },
    /// Block these hostnames directly.
    Domains { domains: Vec<String> },
}
```

Add the two fields to `struct OpenInput` (after `persona`):

```rust
    /// Enable the bundled curated tracker/fingerprinter blocklist (passive
    /// third-party fingerprinters + cross-site trackers; excludes anti-bot
    /// challenge vendors). Off by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_trackers: Option<bool>,
    /// Custom tracker-blocklist source (url | file | inline domains). Supplying
    /// this implicitly enables blocking; combine with `block_trackers: true`
    /// to also include the bundled list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_blocklist: Option<TrackerBlocklist>,
```

- [ ] **Step 3: Wire into the builder in `browser_open`**

In the `browser_open` handler (after the `persona` wiring, ~line 86), add a
gated block plus a `not(feature)` guard so a request that asks for blocking on a
build without the feature fails loudly rather than silently no-op'ing:

```rust
    #[cfg(feature = "tracker-blocking")]
    {
        if input.block_trackers.unwrap_or(false) {
            builder = builder.block_trackers(true);
        }
        if let Some(bl) = &input.tracker_blocklist {
            builder = match bl {
                TrackerBlocklist::Url { url } => builder.tracker_blocklist_url(url.clone()),
                TrackerBlocklist::File { path } => {
                    builder.tracker_blocklist_file(std::path::PathBuf::from(path))
                }
                TrackerBlocklist::Domains { domains } => {
                    builder.tracker_blocklist_add(domains.clone())
                }
            };
        }
    }
    #[cfg(not(feature = "tracker-blocking"))]
    if input.block_trackers.unwrap_or(false) || input.tracker_blocklist.is_some() {
        return Err(ErrorData::invalid_params(
            "tracker blocking requested but this server was built without the `tracker-blocking` feature".to_string(),
            None,
        ));
    }
```

- [ ] **Step 4: Build + run MCP unit tests**

Run:

```bash
cargo build -p zendriver-mcp --features tracker-blocking
cargo test -p zendriver-mcp --features tracker-blocking lifecycle:: --lib
```

Expected: builds and unit tests PASS.

- [ ] **Step 5: Regenerate schema snapshots**

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```

Review the `browser_open` snapshot diff: it should add `block_trackers`
(boolean) and `tracker_blocklist` (a tagged `source` enum with `url`/`file`/
`domains` variants), and nothing else should change.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-mcp/Cargo.toml crates/zendriver-mcp/src/tools/lifecycle.rs crates/zendriver-mcp/tests/snapshots/
git commit -m "feat(mcp): browser_open block_trackers + tracker_blocklist"
```

---

## Task 6: Real-Chrome integration test

**Files:**
- Create: `crates/zendriver/tests/tracker_blocklist_integration.rs`

`#[ignore]` + `#[cfg(feature = "integration-tests")]` (same gate as
`fingerprint_integration.rs`); needs a local Chrome + network egress, so it is
not in the normal matrix.

- [ ] **Step 1: Write the test**

```rust
//! Headful integration: a listed host is blocked (net::ERR_BLOCKED_BY_CLIENT)
//! while an unlisted host loads. Needs a local Chrome + outbound network.
//!
//! Run with:
//! ```sh
//! cargo test -p zendriver --test tracker_blocklist_integration \
//!     --features integration-tests -- --ignored
//! ```
#![cfg(feature = "integration-tests")]

use serial_test::serial;
use zendriver::Browser;

// Fetches `url` from the page and reports "ok" or "blocked" — a blocked
// request rejects the fetch promise with a TypeError.
const FETCH_PROBE: &str = r#"(async (u) => {
    try {
        await fetch(u, { mode: 'no-cors', cache: 'no-store' });
        return 'ok';
    } catch (e) {
        return 'blocked:' + e;
    }
})"#;

#[tokio::test]
#[serial]
#[ignore] // run with: cargo test ... -- --ignored
async fn listed_host_is_blocked_unlisted_loads() {
    // Inline custom blocklist — deterministic, no dependence on bundled-list
    // contents. example.com is the unlisted control; example.org is "blocked".
    let browser = Browser::builder()
        .tracker_blocklist_add(["example.org".to_string()])
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("https://example.com/").await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Listed host -> blocked.
    let blocked: String = tab
        .evaluate::<String>(&format!("({FETCH_PROBE})('https://example.org/')"))
        .await
        .unwrap();
    assert!(
        blocked.starts_with("blocked:"),
        "listed host should be blocked, got: {blocked}"
    );

    // Unlisted host -> loads (a same-origin/CORS-opaque fetch resolves).
    let ok: String = tab
        .evaluate::<String>(&format!("({FETCH_PROBE})('https://example.com/')"))
        .await
        .unwrap();
    assert_eq!(ok, "ok", "unlisted host should load, got: {ok}");

    browser.close().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn subdomain_of_listed_host_is_blocked() {
    let browser = Browser::builder()
        .tracker_blocklist_add(["example.org".to_string()])
        .launch()
        .await
        .unwrap();
    let tab = browser.main_tab();
    tab.goto("https://example.com/").await.unwrap();
    tab.wait_for_load().await.unwrap();

    let blocked: String = tab
        .evaluate::<String>(&format!("({FETCH_PROBE})('https://www.example.org/')"))
        .await
        .unwrap();
    assert!(
        blocked.starts_with("blocked:"),
        "subdomain of a listed host should be blocked, got: {blocked}"
    );

    browser.close().await.unwrap();
}
```

- [ ] **Step 2: Run the test (needs local Chrome + network)**

Run: `cargo test -p zendriver --test tracker_blocklist_integration --features integration-tests -- --ignored`
Expected: PASS — listed/subdomain hosts blocked, unlisted host loads. (If the
environment is egress-starved — see the `wait_for_idle` NTP-flake note — this
test cannot pass; it is local-only by design, like `fingerprint_integration.rs`.)

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/tracker_blocklist_integration.rs
git commit -m "test(zendriver): real-Chrome tracker blocklist integration"
```

---

## Task 7: MCP coverage ledger + public-API baseline

**Files:**
- Modify: `crates/zendriver-mcp/mcp-coverage-ledger.toml`
- Modify: `crates/zendriver-mcp/public-api-baseline.txt` (regen)

- [ ] **Step 1: Regenerate the public-API baseline**

```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
```

Inspect `git diff` on the baseline. Expect NEW lines for: the four
`BrowserBuilder` methods (`block_trackers`, `tracker_blocklist_add`,
`tracker_blocklist_file`, `tracker_blocklist_url`); the re-exported
`zendriver::HostMatcher` (+ its inherent methods); `zendriver::Rule::BlockHosts`;
and `InterceptBuilder::block_hosts` reachable via the re-export.

- [ ] **Step 2: Add ledger entries**

Append to `crates/zendriver-mcp/mcp-coverage-ledger.toml` (use the exact API
paths the diff surfaced — adjust strings to match `cargo-public-api` output):

```toml
# -- #2 Tracker/fingerprinter blocklist --------------------------------------
[[entry]]
api = "zendriver::browser::BrowserBuilder::block_trackers"
covered = "browser_open.block_trackers"

[[entry]]
api = "zendriver::browser::BrowserBuilder::tracker_blocklist_add"
covered = "browser_open.tracker_blocklist (source=domains)"

[[entry]]
api = "zendriver::browser::BrowserBuilder::tracker_blocklist_file"
covered = "browser_open.tracker_blocklist (source=file)"

[[entry]]
api = "zendriver::browser::BrowserBuilder::tracker_blocklist_url"
covered = "browser_open.tracker_blocklist (source=url)"

[[entry]]
api = "zendriver::HostMatcher"
excluded = "internal matching mechanism; reached via BrowserBuilder::block_trackers / browser_open, not directly"

[[entry]]
api = "zendriver::Rule::BlockHosts"
excluded = "internal interception rule variant installed by the tracker-blocklist wiring; reached via the builder/tool, not directly"
```

> If the diff shows additional reachable paths (e.g. `HostMatcher::new` /
> `is_blocked` / `len` / `is_empty`, or `InterceptBuilder::block_hosts`, or
> module-qualified duplicates like `zendriver::host_matcher::HostMatcher`), add a
> matching `excluded` entry for each with the same rationale ("internal matching
> mechanism / interception primitive; reached via the builder/tool"). The
> `public_api` test fails listing exactly which paths still lack an entry — let
> it drive the list.

- [ ] **Step 3: Run the public-API coverage check**

```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```

Expected: PASS — no uncovered new API items. (Iterate Step 2 until green.)

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-mcp/public-api-baseline.txt crates/zendriver-mcp/mcp-coverage-ledger.toml
git commit -m "chore(mcp): tracker-blocklist public-API baseline + coverage ledger"
```

---

## Task 8: Pre-push gates (CLAUDE.md)

- [ ] **Step 1: Format, then lint + test in parallel**

```bash
cargo fmt --all
```

then (batch in one shell message — independent):

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
cargo test -p zendriver-interception --lib
cargo test -p zendriver --features tracker-blocking --lib
```

Expected: fmt clean; clippy no warnings on default **and** all-features; all
unit tests PASS. Fix anything flagged; re-stage. (Default-feature clippy proves
the `#[cfg]` gating leaves no dead/unused code when the feature is off.)

- [ ] **Step 2: Schema snapshots clean (no pending)**

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
```

Expected: PASS, no pending snapshots (Task 5 already accepted them).

- [ ] **Step 3: Public-API check**

```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```

Expected: PASS (Task 7).

- [ ] **Step 4: Final fmt/clippy commit (only if anything changed)**

```bash
git add -A
git commit -m "chore: fmt + clippy for tracker blocklist"
```

---

## Self-Review

- **Spec §4 coverage:**
  - §4.1 mechanism — `HostMatcher` + `host_of` (Task 1); `Rule::BlockHosts` reusing the `failRequest(BlockedByClient)` path + composing in registration order (Task 2). ✓
  - §4.2 opt-in & sourcing — bundled `include_str!` list + parser (Task 3); `block_trackers` / `tracker_blocklist_add` / `_file` / `_url` builder methods (Task 3); runtime fetch+cache mirroring `pool::load_or_download` (Task 3); one `Arc<HostMatcher>` built at launch, installed per tab (Task 4). ✓
  - §4.3 license — own clean list bundled; URL source is a runtime fetch under user acceptance (Task 3 `trackers.txt` header + `tracker_blocklist_url`). ✓
  - §4.4 curation — passive trackers/fingerprinters only; anti-bot vendors excluded + asserted absent in a unit test (Task 3). ✓
  - §4.5 matching — host-only, suffix-on-dot, unconditional (Task 1 `is_blocked` + tests). ✓
  - §4.6 feature-gating — `tracker-blocking` implies `interception`; bundled list costs size only when on (Task 3); MCP mirror (Task 5). ✓
  - §4.7 MCP — `browser_open` extended; schema snapshots + baseline regenerated; ledger excludes internals (Tasks 5, 7). ✓
  - §4.9 tests — `HostMatcher` unit (T1); `BlockHosts` actor test (T2); bundled-list parse + builder wiring + matcher build (T3); integration listed-blocked/unlisted-loads (T6). ✓
- **Open items resolved:** (1) `trackers.txt` curated in Task 3 (~85 hosts, exclusion principle applied + asserted). (2) Tab-creation seam = `TabRegistrar` `"page"` branch + main-tab explicit install in `finish_connect` (Task 4). (3) Fetch+cache helper duplicated into `core::tracker` (acceptable per handoff). ✓
- **Placeholder scan:** none — every code step shows complete code; `trackers.txt` is given in full; ledger/baseline steps name expected paths and are driven by the failing test's own output.
- **Type/name consistency:** `HostMatcher::{new,is_blocked,len,is_empty}`, `host_of`, `Rule::BlockHosts { matcher }`, `InterceptBuilder::block_hosts`, `BrowserBuilder::{block_trackers,tracker_blocklist_add,tracker_blocklist_file,tracker_blocklist_url}`, `build_tracker_matcher`, `BrowserInner::{tracker_matcher,tracker_handles}`, `FinishConnect::tracker_matcher`, `tracker::{bundled_hosts,parse_blocklist,load_or_download_blocklist}`, MCP `OpenInput::{block_trackers,tracker_blocklist}` + `TrackerBlocklist::{Url,File,Domains}` — used identically across tasks. `tracker-blocking` feature name is consistent in both `Cargo.toml`s and every `#[cfg]`.
- **Feature gating audit:** all core tracker code (`mod tracker`, builder fields/methods, `BrowserInner`/`FinishConnect` fields, install blocks) is `#[cfg(feature = "tracker-blocking")]`; `FinishConnect` field init is gated at all 4 call sites; MCP wiring has a `not(feature)` guard. Default-feature clippy in Task 8 step 1 is the backstop.
