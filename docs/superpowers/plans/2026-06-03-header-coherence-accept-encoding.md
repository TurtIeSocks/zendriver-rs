# Header coherence (Accept-Encoding) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Chrome's `Accept-Encoding` request header coherent with the *claimed* stealth Chrome major, so a profile that pins an identity across the `zstd`/Chrome-123 boundary no longer leaks the real binary's encoding set.

**Architecture:** A pure `accept_encoding_for(major)` rule in `zendriver-stealth`; `StealthProfile::resolve_fingerprint` detects a claimed-vs-binary skew and returns an `Option<String>` override; `StealthObserver` applies it via `Network.setExtraHTTPHeaders` (only on the skew path, so the default profile adds zero CDP traffic). No header network port, no download, no new carrier type, no new MCP tool. See spec: `docs/superpowers/specs/2026-06-02-header-coherence-design.md`.

**Tech Stack:** Rust 2024, `serde_json`, `zendriver-transport` CDP actor + its `MockConnection` test harness, `tokio` tests.

---

## File Structure

- **Create** `crates/zendriver-stealth/src/headers.rs` — the `accept_encoding_for` rule + unit tests. One responsibility: map a Chrome major to its coherent `Accept-Encoding`.
- **Create** `crates/zendriver/tests/accept_encoding_coherence.rs` — `#[ignore]`d real-Chrome probe verifying `setExtraHTTPHeaders` *replaces* (not appends) `Accept-Encoding`.
- **Modify** `crates/zendriver-stealth/src/lib.rs` — declare `mod headers;`.
- **Modify** `crates/zendriver-stealth/src/observer.rs` — add an `accept_encoding: Option<String>` field, a `with_accept_encoding` setter, and the apply block; add a Some-path test.
- **Modify** `crates/zendriver-stealth/src/profile.rs` — `resolve_fingerprint` returns `(Fingerprint, Option<String>)`; add skew unit tests; fix its existing test.
- **Modify** `crates/zendriver/src/browser.rs:1796,2017` — destructure the tuple and wire the override into the observer.

---

## Task 0: Pre-flight — verify `setExtraHTTPHeaders` replaces `Accept-Encoding`

**Why first:** the entire apply surface (Task 2) assumes `Network.setExtraHTTPHeaders` *replaces* `Accept-Encoding`. If Chrome *appends* instead, we'd ship a duplicated header (worse than nothing) and must pivot to the `Fetch` interception path. Settle this empirically before wiring.

**Files:**
- Create: `crates/zendriver/tests/accept_encoding_coherence.rs`

- [ ] **Step 1: Write the `#[ignore]`d real-Chrome probe**

```rust
//! Real-Chrome probe (run with `--ignored`): confirms that
//! `Network.setExtraHTTPHeaders` REPLACES `Accept-Encoding` rather than
//! appending to it. This is the empirical gate behind the header-coherence
//! design (`docs/superpowers/specs/2026-06-02-header-coherence-design.md`).
//!
//! Launches a stealth browser with a forced claimed-major skew, navigates to a
//! local TCP listener that captures the raw request, and asserts the outbound
//! `Accept-Encoding` is exactly our injected value — no `zstd`, no duplication.
#![cfg(feature = "stealth")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

use zendriver::Browser;
use zendriver_stealth::StealthProfile;

#[tokio::test]
#[ignore = "requires a real Chrome binary; run with --ignored"]
async fn set_extra_http_headers_replaces_accept_encoding() {
    // Local one-shot HTTP server that records the request headers it receives.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(&stream);
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
                headers.push(line.trim_end().to_string());
            }
            let mut w = &stream;
            let _ = w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            let _ = tx.send(headers);
        }
    });

    // Force a skew: claim Chrome 120 (no zstd). If the real binary is >=123 the
    // observer must inject `Accept-Encoding: gzip, deflate, br`.
    let profile = StealthProfile::spoofed().chrome_version(120);
    let browser = Browser::builder()
        .stealth(profile)
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.new_tab().await.expect("tab");
    let _ = tab
        .goto(&format!("http://127.0.0.1:{port}/"))
        .await;

    let headers = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("server saw a request");
    let ae: Vec<&String> = headers
        .iter()
        .filter(|h| h.to_ascii_lowercase().starts_with("accept-encoding:"))
        .collect();
    // Exactly one Accept-Encoding header (no duplication = replace, not append).
    assert_eq!(ae.len(), 1, "expected one Accept-Encoding header, got {ae:?}");
    assert_eq!(
        ae[0].to_ascii_lowercase(),
        "accept-encoding: gzip, deflate, br",
        "header was not replaced cleanly: {ae:?}",
    );
    browser.close().await.ok();
}
```

> **API names (verified against current surface):** `Browser::builder()` → `BrowserBuilder` (`browser.rs:2257`), `.stealth(profile)` (`:819`), `.headless(bool)`, `.launch()` (`:1749`); `browser.new_tab()` (`:2490`); `tab.goto(url)` (`tab.rs:855`); `tab.close()` (`:1336`) and `browser.close()` (`:2771`) both consume `self`.

- [ ] **Step 2: Run it (main thread, if a Chrome binary is available)**

Run: `cargo test -p zendriver --test accept_encoding_coherence -- --ignored --nocapture`
Expected: PASS — one `Accept-Encoding` header, value `gzip, deflate, br`.

- [ ] **Step 3: Decision gate**

- **PASS (clean replace):** proceed with Task 2 as written (`Network.setExtraHTTPHeaders`).
- **FAIL (duplicated/append):** STOP. The lightweight path is unsafe. Re-spec to apply via the `Fetch` path (`zendriver-interception` `paused.rs:198` / `actor.rs:454`) gated behind the `interception` feature, then resume. Record the observed behavior in this test's doc comment.
- **Chrome unavailable in this environment:** leave the test committed (`#[ignore]`), record in the task notes that the probe is pending, and proceed with `setExtraHTTPHeaders` on the documented high-confidence assumption that it replaces (the common stealth-tooling behavior). Flag for the user to run `--ignored` once.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver/tests/accept_encoding_coherence.rs
git commit -m "test(zendriver): ignored real-Chrome probe for Accept-Encoding replacement"
```

---

## Task 1: `accept_encoding_for` rule

**Files:**
- Create: `crates/zendriver-stealth/src/headers.rs`
- Modify: `crates/zendriver-stealth/src/lib.rs` (add `mod headers;`)

- [ ] **Step 1: Write the failing test**

Create `crates/zendriver-stealth/src/headers.rs` with the test first:

```rust
//! Coherent request-header values derived from the claimed stealth identity.
//!
//! A real Chrome over CDP already emits coherent request headers; the one value
//! that can silently skew from the *claimed* identity is `Accept-Encoding`,
//! because Chrome's network stack advertises the *real binary's* supported
//! encodings regardless of the UA / UA-CH overrides we apply. `zstd` shipped
//! enabled-by-default in Chrome 123 (2024-03); `br` (brotli) since Chrome 50.
//! Two branches therefore suffice — zendriver does not drive pre-`br` Chrome.
//!
//! See `docs/superpowers/specs/2026-06-02-header-coherence-design.md`.

/// The `Accept-Encoding` a real Chrome of `major` sends for a top-level HTTPS
/// navigation.
pub(crate) fn accept_encoding_for(major: u32) -> &'static str {
    if major >= 123 {
        "gzip, deflate, br, zstd"
    } else {
        "gzip, deflate, br"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_encoding_straddles_the_zstd_boundary() {
        assert_eq!(accept_encoding_for(122), "gzip, deflate, br");
        assert_eq!(accept_encoding_for(123), "gzip, deflate, br, zstd");
        assert_eq!(accept_encoding_for(148), "gzip, deflate, br, zstd");
    }
}
```

Add to `crates/zendriver-stealth/src/lib.rs` after `pub mod flags;` (keep the module list alphabetical — insert between `flags` and `input_profile`):

```rust
mod headers;
```

- [ ] **Step 2: Run test to verify it fails, then passes**

Run: `cargo test -p zendriver-stealth headers::tests`
Expected: compiles and PASSES (the rule is implemented alongside the test — there is no separate red phase for a 2-branch pure fn). If `mod headers;` is missing you'll get `unresolved module`; add it.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-stealth/src/headers.rs crates/zendriver-stealth/src/lib.rs
git commit -m "feat(stealth): accept_encoding_for(major) coherence rule"
```

---

## Task 2: Observer applies the `Accept-Encoding` override

**Files:**
- Modify: `crates/zendriver-stealth/src/observer.rs`

- [ ] **Step 1: Add the field + setter**

In the `StealthObserver` struct (after `bootstrap: String,`), add:

```rust
    /// Coherent `Accept-Encoding` override — `Some` only under a claimed-vs-binary
    /// Chrome-major skew (see [`crate::headers`]). Applied via
    /// `Network.setExtraHTTPHeaders` on the skew path only.
    accept_encoding: Option<String>,
```

In `with_persona`, set it in the returned struct literal (it currently builds `Self { profile, fingerprint, bootstrap }`):

```rust
        Self {
            profile,
            fingerprint,
            bootstrap,
            accept_encoding: None,
        }
```

Add the setter in the `impl StealthObserver` block (after `with_persona`):

```rust
    /// Set the coherent `Accept-Encoding` override applied to each page target
    /// (see [`crate::headers`]). `None` (the default) sends no header.
    #[must_use]
    pub fn with_accept_encoding(mut self, accept_encoding: Option<String>) -> Self {
        self.accept_encoding = accept_encoding;
        self
    }
```

- [ ] **Step 2: Write the failing test**

In `observer.rs`'s `#[cfg(test)] mod tests`, add (alongside the existing sequence test):

```rust
    #[tokio::test]
    async fn accept_encoding_override_injects_network_headers() {
        let fp = Fingerprint {
            platform: Platform::MacIntel,
            chrome_major: 120,
            chrome_full: "120.0.6099.234".into(),
            cpu_count: 10,
            memory_gb: 8,
            ua_string: crate::ua::compose_ua_string(Platform::MacIntel, "120.0.6099.234"),
            ua_metadata: crate::UserAgentMetadata::realistic(
                Platform::MacIntel,
                120,
                "120.0.6099.234",
            ),
            timezone: None,
            locale: None,
        };
        let observer = std::sync::Arc::new(
            StealthObserver::new(StealthProfile::spoofed(), fp)
                .with_accept_encoding(Some("gzip, deflate, br".into())),
        );
        let (mut mock, conn) = MockConnection::pair_with_observers(vec![observer]);

        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S1",
                "targetInfo": {
                    "targetId": "T1",
                    "type": "page",
                    "url": "about:blank",
                    "attached": true,
                },
                "waitingForDebugger": true,
            }),
        )
        .await;

        // Exact ordered sequence — the Accept-Encoding block sits between the
        // Emulation overrides and the spoofed bootstrap block.
        for expected in [
            "Page.enable",
            "Emulation.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setFocusEmulationEnabled",
            "Network.enable",
            "Network.setExtraHTTPHeaders",
            "Page.setBypassCSP",
            "Page.addScriptToEvaluateOnNewDocument",
            "Runtime.runIfWaitingForDebugger",
        ] {
            let id =
                tokio::time::timeout(std::time::Duration::from_secs(2), mock.expect_cmd(expected))
                    .await
                    .unwrap_or_else(|_| panic!("did not see {expected} within 2s"));
            if expected == "Network.setExtraHTTPHeaders" {
                assert_eq!(
                    mock.last_sent()["params"]["headers"]["Accept-Encoding"],
                    "gzip, deflate, br",
                );
            }
            mock.reply(id, json!({})).await;
        }
        conn.shutdown();
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zendriver-stealth observer::tests::accept_encoding_override_injects_network_headers`
Expected: FAIL/timeout — the observer does not yet emit `Network.enable`.

- [ ] **Step 4: Add the apply block**

In `on_target_attached`, immediately after the `setLocaleOverride` block (the `if let Some(ref locale) = self.fingerprint.locale { … }`) and **before** the `if self.profile.kind() == ProfileKind::Spoofed {` block:

```rust
        // Accept-Encoding coherence: when the claimed Chrome major straddles the
        // `zstd` boundary vs the real binary, override the header so it matches
        // the *claimed* identity (Chrome's network stack would otherwise leak the
        // binary's encodings). `Network.enable` is idempotent + invisible to the
        // page; both calls fire only on this skew path, so the default profile is
        // unaffected.
        if let Some(ref ae) = self.accept_encoding {
            session.call("Network.enable", json!({})).await?;
            session
                .call(
                    "Network.setExtraHTTPHeaders",
                    json!({ "headers": { "Accept-Encoding": ae } }),
                )
                .await?;
        }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zendriver-stealth observer::tests`
Expected: PASS — both the new Some-path test and the existing
`spoofed_observer_sends_expected_sequence_for_page_target` (the latter proves the
None path emits **no** `Network.*` commands: a wrongful injection would block the
observer awaiting an unanswered reply and time the test out).

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-stealth/src/observer.rs
git commit -m "feat(stealth): observer injects coherent Accept-Encoding on skew"
```

---

## Task 3: Detect the skew in `resolve_fingerprint` + wire it through

**Files:**
- Modify: `crates/zendriver-stealth/src/profile.rs` (the `resolve_fingerprint` fn ~280 + its test ~463)
- Modify: `crates/zendriver/src/browser.rs:1796,2017`

- [ ] **Step 1: Write the failing skew tests**

In `profile.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn resolve_returns_accept_encoding_only_on_zstd_skew() {
        // Pin a fingerprint override so the "binary" baseline is deterministic
        // (major 143 -> sends zstd), then claim a pre-zstd major.
        let mut base = crate::Fingerprint::auto_detect(std::path::Path::new("/nonexistent"))
            .expect("fallback fingerprint");
        base.chrome_major = 143;
        base.chrome_full = "143.0.0.0".into();
        base.recompose();

        // Claimed 120 (< 123) vs binary 143 (>= 123) -> skew -> Some.
        let skewed = StealthProfile::spoofed()
            .fingerprint(base.clone())
            .chrome_version(120);
        let (fp, ae) = skewed
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .expect("resolve");
        assert_eq!(fp.chrome_major, 120);
        assert_eq!(ae.as_deref(), Some("gzip, deflate, br"));

        // Claimed 140 (>= 123) vs binary 143 (>= 123) -> same set -> None.
        let no_skew = StealthProfile::spoofed()
            .fingerprint(base.clone())
            .chrome_version(140);
        let (_fp, ae) = no_skew
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .expect("resolve");
        assert_eq!(ae, None);

        // No claimed-major override -> None (default path adds nothing).
        let plain = StealthProfile::spoofed().fingerprint(base);
        let (_fp, ae) = plain
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .expect("resolve");
        assert_eq!(ae, None);
    }
```

> Confirm the builder names against `profile.rs`: the per-field override is `.chrome_version(u32)` (verified at `profile.rs:202`); the full-fingerprint override builder is whatever sets `fingerprint_override` — check the method name (likely `.fingerprint(Fingerprint)`) and adjust the test if it differs.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zendriver-stealth profile::tests::resolve_returns_accept_encoding_only_on_zstd_skew`
Expected: FAIL to compile — `resolve_fingerprint` still returns `Fingerprint`, not a tuple.

- [ ] **Step 3: Change `resolve_fingerprint` to detect the skew**

Change the signature (`profile.rs:280`):

```rust
    pub fn resolve_fingerprint(
        &self,
        chrome_exe: &Path,
    ) -> Result<(Fingerprint, Option<String>), StealthError> {
```

Capture the binary major right after `fp` is built, before any override — insert immediately after the `let mut fp = match … };` block (before the `if let Some(p) = self.per_field.platform`):

```rust
        // The encodings the *real* binary (or override-supplied fingerprint)
        // advertises, captured before the claimed-major override below.
        let binary_major = fp.chrome_major;
```

At the end, replace `Ok(fp)` with:

```rust
        // Accept-Encoding coherence: override only when the claimed major's
        // encoding set differs from the binary's (straddles the zstd/Chrome-123
        // boundary). Otherwise Chrome's native header is already coherent.
        let claimed_major = fp.chrome_major;
        let accept_encoding = (crate::headers::accept_encoding_for(claimed_major)
            != crate::headers::accept_encoding_for(binary_major))
        .then(|| crate::headers::accept_encoding_for(claimed_major).to_string());

        Ok((fp, accept_encoding))
```

- [ ] **Step 4: Fix the existing `resolve_fingerprint` test**

At `profile.rs:480`, the existing test binds `let fp = …resolve_fingerprint(…)`. Update it to destructure:

```rust
        let (fp, _ae) = profile
            .resolve_fingerprint(std::path::Path::new("/nonexistent"))
            .unwrap();
```

(Keep the rest of that test's assertions on `fp` unchanged.)

- [ ] **Step 5: Wire both `browser.rs` call sites**

At `browser.rs:1796`:

```rust
                let (fp, accept_encoding) = profile.resolve_fingerprint(&exe)?;
                let stealth_obs: Arc<dyn TargetObserver> = Arc::new(
                    StealthObserver::with_persona(profile.clone(), fp, self.resolved_persona())
                        .with_accept_encoding(accept_encoding),
                );
```

At `browser.rs:2017` (the connect path):

```rust
            let (fp, accept_encoding) = profile.resolve_fingerprint(Path::new(""))?;
            let stealth_obs: Arc<dyn TargetObserver> = Arc::new(
                StealthObserver::with_persona(profile.clone(), fp, self.resolved_persona())
                    .with_accept_encoding(accept_encoding),
            );
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zendriver-stealth profile::tests` then `cargo build -p zendriver`
Expected: PASS + clean build (both call sites destructure the tuple).

- [ ] **Step 7: Commit**

```bash
git add crates/zendriver-stealth/src/profile.rs crates/zendriver/src/browser.rs
git commit -m "feat(stealth): resolve_fingerprint detects claimed-vs-binary Accept-Encoding skew"
```

---

## Task 4: Full gates + final verification

**Files:** none (verification only)

- [ ] **Step 1: Format + lint (auto-fix), exactly as CI runs them**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```
Expected: both `--check` and `-D warnings` gates clean. Re-stage any auto-fixes.

- [ ] **Step 2: Feature-gated clippy (stealth touches no new feature, but mcp re-exposes stealth)**

```bash
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 3: Targeted test run**

```bash
cargo test -p zendriver-stealth
cargo test -p zendriver --lib
```
Expected: PASS. (The `#[ignore]`d Task 0 probe is skipped by default.)

- [ ] **Step 4: Public-API check (expected no diff)**

`resolve_fingerprint` is `pub` in `zendriver-stealth` but is **not** surfaced in
`zendriver`'s public baseline (only the `stealth()` builder + the `StealthProfile`
re-export are), so the tuple-return change should not move the baseline.

```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked
```
Expected: PASS with no baseline diff. If it *does* report a new/changed item,
regenerate per `CLAUDE.md`:
`cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt`
— then confirm the diff is only the intended internal change and commit it. No new
MCP tool and no new capability ⇒ no `mcp-coverage-ledger.toml` entry.

- [ ] **Step 5: Final commit (if Steps 1–4 produced fmt/lint/baseline changes)**

```bash
git add -A
git commit -m "chore: fmt + lint + baseline for Accept-Encoding coherence"
```

---

## Self-Review

**Spec coverage** (against `2026-06-02-header-coherence-design.md`):
- §4.1 `accept_encoding_for` → Task 1. ✓
- §4.2 skew detection in `resolve_fingerprint` → Task 3. ✓
- §4.3 observer apply (`Network.enable` + `setExtraHTTPHeaders`, skew-only) → Task 2. ✓
- §4.4 empirical replace-vs-append gate → Task 0. ✓
- §5 files touched → all mapped (headers.rs, lib.rs, observer.rs, profile.rs, browser.rs, the probe test). ✓
- §6 testing (boundary, skew detection, observer sequence, CDP probe) → Tasks 1/3/2/0. ✓
- §7 gates (fmt, clippy, feature clippy, tests, public-api) → Task 4. ✓
- §8 MCP coverage (no new tool/ledger) → Task 4 Step 4. ✓
- §9 assumptions are design-level (no extra tasks needed); A8's empirical fork is Task 0's decision gate. ✓

**Placeholder scan:** no TBD/TODO; every code step shows complete code. The two "confirm the API name" notes (Task 0 builder/tab names, Task 3 `.fingerprint()` builder) are verification prompts against real symbols, not missing content.

**Type consistency:** `accept_encoding_for(u32) -> &'static str` (Task 1) used identically in Task 3. `with_accept_encoding(Option<String>)` defined in Task 2, called in Task 3. `resolve_fingerprint → Result<(Fingerprint, Option<String>), StealthError>` defined in Task 3, destructured at both call sites + the fixed test. Observer field `accept_encoding: Option<String>` consistent across struct, setter, and apply block.
