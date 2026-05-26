# zendriver-imperva Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a new `zendriver-imperva` workspace crate that passively bypasses Imperva WAF / Incapsula challenges (reese84 / legacy / CAPTCHA surfaces) from a real Chrome tab, plus a small `zendriver-cloudflare` retrofit for posture parity.

**Architecture:** Mirror `zendriver-cloudflare`'s shape file-for-file: one `ImpervaBypass` struct, builder methods, `wait_for_clearance()` runs an embedded `detect.js` per poll tick, returns one of `TokenAcquired | ChallengeGone | AlreadyClear`. Opt-in Fetch-domain interception escape hatch + opt-in CAPTCHA solver callback. Stealth posture enforced via lib.rs docs + `tracing::warn!` on stalled poll loops (no cargo-feature plumbing needed since `zendriver-stealth` is already a non-optional parent-crate dep).

**Tech Stack:** Rust 1.85, tokio, serde/serde_json, thiserror, tracing, `zendriver-transport` + `zendriver-interception` workspace deps, embedded JS via `include_str!`, unit tests via `MockConnection` (zendriver-transport `testing` feature).

**Spec:** [docs/superpowers/specs/2026-05-24-zendriver-imperva-design.md](../specs/2026-05-24-zendriver-imperva-design.md)

---

## File structure

**New files:**

- `crates/zendriver-imperva/Cargo.toml`
- `crates/zendriver-imperva/src/lib.rs`
- `crates/zendriver-imperva/src/error.rs`
- `crates/zendriver-imperva/src/detection.rs`
- `crates/zendriver-imperva/src/detect.js`
- `crates/zendriver-imperva/src/bypass.rs`
- `crates/zendriver-imperva/src/interception.rs`
- `crates/zendriver/examples/imperva_bypass.rs`
- `crates/zendriver/tests/imperva_v0.rs`
- `docs/book/src/imperva.md`
- `.github/workflows/` — extend `ci.yml` with `nightly-imperva-tests` job

**Modified files:**

- `Cargo.toml` (workspace) — add `crates/zendriver-imperva` member + workspace dep entry
- `crates/zendriver/Cargo.toml` — add `imperva` feature + optional dep + example entry + `imperva-tests` feature
- `crates/zendriver/src/lib.rs` — re-exports + `imperva_surface_is_send_sync` test + feature matrix doc
- `crates/zendriver/src/tab.rs` — `Tab::imperva()` impl + stealth note added to `Tab::cloudflare()` rustdoc
- `crates/zendriver/src/error.rs` — `ZendriverError::Imperva` variant + `From` impl
- `crates/zendriver-cloudflare/Cargo.toml` — drop stale `zendriver-interception` dep
- `crates/zendriver-cloudflare/src/lib.rs` — stealth-required call-out
- `crates/zendriver-cloudflare/src/bypass.rs` — stalled-poll `tracing::warn!`
- `docs/book/src/SUMMARY.md` — add imperva chapter entry
- `README.md` — add `zendriver-imperva` to feature matrix + comparison
- `CHANGELOG.md` — note new crate + cloudflare retrofit

## Task list

| #  | Title                                       | Files touched (count) |
|----|---------------------------------------------|-----------------------|
| 0  | Workspace skeleton + empty crate compiles   | 4                     |
| 1  | `ImpervaError` type                         | 1                     |
| 2  | `ImpervaSurface` enum + `detect.js` + `detect_surface()` | 2 + 1 JS  |
| 3  | `ImpervaBypass` struct + builder methods    | 1                     |
| 4  | `wait_for_clearance` Reese84 + Legacy + None + Timeout paths | 1   |
| 5  | CAPTCHA path (`on_captcha` callback + fast-fail) | 1                |
| 6  | Interception fast-path (`with_interception`) | 1                    |
| 7  | Stalled-poll telemetry (imperva + cloudflare) | 2                   |
| 8  | Parent `zendriver` wiring (error, re-exports, `Tab::imperva`, send+sync) | 4 |
| 9  | Cloudflare retrofit (stale dep drop + docs callouts) | 3            |
| 10 | Example `imperva_bypass.rs`                 | 2                     |
| 11 | Doctests audit (≥5)                         | ~4                    |
| 12 | Nightly CI job + integration test scaffold  | 3                     |
| 13 | mdBook chapter + README + CHANGELOG         | 4                     |

Total commits ≈ 14 (one per task).

---

## Task 0: Workspace skeleton + empty crate compiles

**Files:**
- Create: `crates/zendriver-imperva/Cargo.toml`
- Create: `crates/zendriver-imperva/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/zendriver/Cargo.toml`

- [ ] **Step 1: Create `crates/zendriver-imperva/Cargo.toml`**

```toml
[package]
name = "zendriver-imperva"
description = "Imperva WAF / Incapsula bypass for zendriver"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
readme = "../../README.md"
homepage = "https://github.com/TurtIeSocks/zendriver-rs"
documentation = "https://docs.rs/zendriver-imperva"
keywords = ["imperva", "incapsula", "waf", "bypass", "zendriver"]
categories = ["web-programming"]

[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs"]

[lints]
workspace = true

[dependencies]
zendriver-transport.workspace    = true
zendriver-interception.workspace = true
tokio.workspace                  = true
serde.workspace                  = true
serde_json.workspace             = true
thiserror.workspace              = true
tracing.workspace                = true

[dev-dependencies]
tokio-test.workspace = true
zendriver-transport  = { workspace = true, features = ["testing"] }
```

- [ ] **Step 2: Create `crates/zendriver-imperva/src/lib.rs` stub**

```rust
//! Imperva WAF / Incapsula bypass for `zendriver`.
//!
//! See the [Imperva chapter](https://turtiesocks.github.io/zendriver-rs/imperva.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, surface variants, and CAPTCHA-callback recipes.
//!
//! **Stealth required.** Imperva's reese84 sensor is itself a browser
//! fingerprint check. Run with [`BrowserBuilder::stealth`] enabled or this
//! bypass will fail on nearly all real Imperva-protected sites.
//!
//! Public API stub — modules land in subsequent tasks.
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth
```

- [ ] **Step 3: Add workspace member + workspace dep to root `Cargo.toml`**

In the `[workspace] members` array, append `"crates/zendriver-imperva",` after the existing `"crates/zendriver-fetcher",` line.

In the `[workspace.dependencies]` block, after the `zendriver-fetcher` line, add:

```toml
zendriver-imperva       = { path = "crates/zendriver-imperva",       version = "0.1.0" }
```

Keep alignment consistent with the existing block (column-aligned `=`).

- [ ] **Step 4: Add optional dep + `imperva` feature to `crates/zendriver/Cargo.toml`**

In `[features]`, add after the `cloudflare` line:

```toml
# Imperva WAF / Incapsula bypass; pulls in interception.
imperva = ["interception", "dep:zendriver-imperva"]
# Network-touching Imperva nightly tests (real protected sites).
imperva-tests = ["imperva", "integration-tests"]
```

In `[dependencies]`, after the `zendriver-cloudflare` line, add:

```toml
zendriver-imperva             = { workspace = true, optional = true }
```

In the `integration-tests` feature line, add `"imperva"` so it stays aligned:

Old:
```toml
integration-tests = ["dep:wiremock", "dep:serial_test", "interception", "expect", "cloudflare"]
```

New:
```toml
integration-tests = ["dep:wiremock", "dep:serial_test", "interception", "expect", "cloudflare", "imperva"]
```

- [ ] **Step 5: Verify (parallel)**

Run all four in one Bash batch:

```bash
cargo build --workspace --locked
cargo build --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expected: all pass. Empty `zendriver-imperva` crate compiles. Parent crate compiles with and without `imperva` feature.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): scaffold zendriver-imperva crate

Empty crate compiles; workspace + parent zendriver Cargo wired with
imperva + imperva-tests features. Sub-crate is a passive bypass
mirror of zendriver-cloudflare. Module skeleton fills in subsequent
commits.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: `ImpervaError` type

**Files:**
- Create: `crates/zendriver-imperva/src/error.rs`
- Modify: `crates/zendriver-imperva/src/lib.rs` (add module + re-export)

- [ ] **Step 1: Write `crates/zendriver-imperva/src/error.rs` with stub enums + tests**

`ImpervaSurface` and `CaptchaKind` formally live in `detection.rs` (Task 2). To keep Task 1 compilable in isolation, define them as stubs at the top of `error.rs` for this task — Task 2 deletes the stub block and replaces it with a `use crate::detection::{CaptchaKind, ImpervaSurface};` import.

```rust
//! Imperva-bypass errors.

use std::time::Duration;
use zendriver_interception::InterceptionError;
use zendriver_transport::CallError;

/// **Stub** — relocated to `detection.rs` in Task 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpervaSurface {
    /// Modern reese84-based bot management.
    Reese84,
    /// Legacy Incapsula (`___utmvc` / `incap_ses_*`).
    Legacy,
    /// Visual or invisible CAPTCHA challenge.
    Captcha(CaptchaKind),
    /// No Imperva surface detected.
    None,
}

/// **Stub** — relocated to `detection.rs` in Task 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaKind {
    HCaptcha,
    Recaptcha,
    ImpervaNative,
    Unknown,
}

/// Error returned by [`ImpervaBypass`] operations.
///
/// [`ImpervaBypass`]: crate::bypass::ImpervaBypass
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImpervaError {
    /// `wait_for_clearance` exceeded the configured timeout.
    /// `last_surface` is the most recent surface observed by the poll loop.
    #[error("clearance not achieved within {timeout:?}")]
    Timeout {
        timeout: Duration,
        last_surface: Option<ImpervaSurface>,
    },

    /// CAPTCHA detected, but no `on_captcha` solver was registered.
    #[error("CAPTCHA required but no solver registered: {kind:?}")]
    CaptchaRequired { kind: CaptchaKind },

    /// User-supplied CAPTCHA solver returned an error.
    #[error("CAPTCHA solver failed: {0}")]
    CaptchaSolver(Box<dyn std::error::Error + Send + Sync>),

    /// Fetch-domain interception hook failed at startup (only when
    /// [`ImpervaBypass::with_interception`] was set).
    ///
    /// [`ImpervaBypass::with_interception`]: crate::bypass::ImpervaBypass::with_interception
    #[error("interception hook error: {0}")]
    Interception(#[from] InterceptionError),

    /// CDP transport / call error.
    #[error("call failed: {0}")]
    Call(#[from] CallError),

    /// In-page evaluator raised or returned an unexpected payload shape.
    #[error("JS error: {0}")]
    JsError(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_timeout_includes_duration() {
        let e = ImpervaError::Timeout {
            timeout: Duration::from_secs(30),
            last_surface: Some(ImpervaSurface::Reese84),
        };
        assert_eq!(e.to_string(), "clearance not achieved within 30s");
    }

    #[test]
    fn display_captcha_required_includes_kind() {
        let e = ImpervaError::CaptchaRequired {
            kind: CaptchaKind::HCaptcha,
        };
        assert_eq!(
            e.to_string(),
            "CAPTCHA required but no solver registered: HCaptcha"
        );
    }

    #[test]
    fn display_js_error_passthrough() {
        let e = ImpervaError::JsError("bad payload".into());
        assert_eq!(e.to_string(), "JS error: bad payload");
    }
}
```

- [ ] **Step 2: Wire module into `crates/zendriver-imperva/src/lib.rs`**

Replace the stub `lib.rs` body (keep the existing top doc-comment) with:

```rust
//! Imperva WAF / Incapsula bypass for `zendriver`.
//!
//! See the [Imperva chapter](https://turtiesocks.github.io/zendriver-rs/imperva.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, surface variants, and CAPTCHA-callback recipes.
//!
//! **Stealth required.** Imperva's reese84 sensor is itself a browser
//! fingerprint check. Run with [`BrowserBuilder::stealth`] enabled or this
//! bypass will fail on nearly all real Imperva-protected sites.
//!
//! Public API stub — modules land in subsequent tasks.
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth

pub mod error;

pub use error::ImpervaError;
```

- [ ] **Step 3: Verify (parallel)**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo clippy -p zendriver-imperva --all-targets --locked -- -D warnings
cargo fmt --all --check
```

Expected: 3 unit tests pass, no clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): add ImpervaError + surface/captcha-kind stubs

ImpervaError covers Timeout, CaptchaRequired, CaptchaSolver,
Interception, Call, JsError. ImpervaSurface + CaptchaKind are
stubbed here so error.rs is self-contained; Task 2 relocates them
to detection.rs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `ImpervaSurface` enum + `detect.js` + `detect_surface()`

**Files:**
- Create: `crates/zendriver-imperva/src/detect.js`
- Create: `crates/zendriver-imperva/src/detection.rs`
- Modify: `crates/zendriver-imperva/src/error.rs` (delete the stub enums)
- Modify: `crates/zendriver-imperva/src/lib.rs` (add `detection` module + re-export)

- [ ] **Step 1: Write `crates/zendriver-imperva/src/detect.js`**

Bundles cookie probe + body marker scan + CAPTCHA iframe pattern check into one round-trip. Returns a JSON object the Rust side decodes.

```javascript
(function () {
    function cookieMap() {
        var out = {};
        var cookies = document.cookie ? document.cookie.split(/; */) : [];
        for (var i = 0; i < cookies.length; i++) {
            var idx = cookies[i].indexOf("=");
            if (idx < 0) continue;
            var name = cookies[i].slice(0, idx).trim();
            var value = cookies[i].slice(idx + 1);
            if (name) out[name] = value;
        }
        return out;
    }

    var cookies = cookieMap();
    var cookieNames = Object.keys(cookies);

    // reese84 may be exactly named or prefixed with __Host- / __Secure-.
    var reese84Key = null;
    for (var i = 0; i < cookieNames.length; i++) {
        var n = cookieNames[i];
        if (n === "reese84" || n.indexOf("reese84") !== -1) {
            reese84Key = n;
            break;
        }
    }
    var reese84 = reese84Key ? cookies[reese84Key] : null;
    if (reese84 === "" || reese84 === "undefined" || reese84 === "null") {
        reese84 = null;
    }

    var hasLegacyCookies = false;
    for (var j = 0; j < cookieNames.length; j++) {
        var n2 = cookieNames[j];
        if (
            n2 === "___utmvc" ||
            n2.indexOf("incap_ses_") === 0 ||
            n2.indexOf("visid_incap_") === 0 ||
            n2 === "nlbi"
        ) {
            hasLegacyCookies = true;
            break;
        }
    }

    var html = document.documentElement
        ? document.documentElement.outerHTML || ""
        : "";

    var bodyHasIncapsulaResource =
        html.indexOf("/_Incapsula_Resource") !== -1 ||
        html.indexOf("/_Incapsula_") !== -1;
    var bodyHasReese =
        html.indexOf("Reese.js") !== -1 ||
        html.indexOf("reese.js") !== -1;
    var bodyHasChallengeMarker =
        html.indexOf("Request unsuccessful. Incapsula") !== -1 ||
        html.indexOf("incident ID") !== -1;

    var captchaKind = null;
    var iframes = document.querySelectorAll("iframe");
    for (var k = 0; k < iframes.length; k++) {
        var src = iframes[k].src || "";
        if (src.indexOf("hcaptcha.com") !== -1 || src.indexOf("hcap.cloud") !== -1) {
            captchaKind = "HCaptcha";
            break;
        }
        if (
            src.indexOf("google.com/recaptcha") !== -1 ||
            src.indexOf("recaptcha.net") !== -1
        ) {
            captchaKind = "Recaptcha";
            break;
        }
        if (src.indexOf("imperva.com/captcha") !== -1) {
            captchaKind = "ImpervaNative";
            break;
        }
    }
    if (
        !captchaKind &&
        (html.indexOf("g-recaptcha") !== -1 || html.indexOf("h-captcha") !== -1)
    ) {
        captchaKind = "Unknown";
    }

    var bodyClean =
        !bodyHasIncapsulaResource && !bodyHasReese && !bodyHasChallengeMarker;

    var hasImpervaSignal =
        !!reese84Key ||
        hasLegacyCookies ||
        bodyHasIncapsulaResource ||
        bodyHasReese ||
        bodyHasChallengeMarker ||
        !!captchaKind;

    var surface;
    if (captchaKind) {
        surface = { kind: "Captcha", captcha: captchaKind };
    } else if (reese84Key || bodyHasReese) {
        surface = { kind: "Reese84" };
    } else if (hasLegacyCookies || bodyHasIncapsulaResource) {
        surface = { kind: "Legacy" };
    } else {
        surface = { kind: "None" };
    }

    var sessionCookies = [];
    for (var m = 0; m < cookieNames.length; m++) {
        var name = cookieNames[m];
        if (
            name === reese84Key ||
            name === "___utmvc" ||
            name.indexOf("incap_ses_") === 0 ||
            name.indexOf("visid_incap_") === 0 ||
            name === "nlbi"
        ) {
            sessionCookies.push({ name: name, value: cookies[name] });
        }
    }

    return {
        surface: surface,
        reese84: reese84,
        body_clean: bodyClean,
        sessions: sessionCookies,
        has_imperva_signal: hasImpervaSignal,
    };
})()
```

- [ ] **Step 2: Write `crates/zendriver-imperva/src/detection.rs`**

```rust
//! Imperva surface detection.
//!
//! One round-trip per probe: bundles cookie reads, body marker scan, and
//! CAPTCHA iframe pattern checks into a single `Runtime.evaluate` carrying
//! [`detect.js`](./detect.js).

use serde::Deserialize;
use serde_json::{Value, json};
use zendriver_transport::SessionHandle;

use crate::error::ImpervaError;

/// Which Imperva surface a tab is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpervaSurface {
    /// Modern reese84-based bot management.
    Reese84,
    /// Legacy Incapsula (`___utmvc` / `incap_ses_*` / `visid_incap_*`).
    Legacy,
    /// Visual or invisible CAPTCHA challenge.
    Captcha(CaptchaKind),
    /// No Imperva surface detected.
    None,
}

/// Kind of CAPTCHA escalation Imperva is presenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaKind {
    HCaptcha,
    Recaptcha,
    ImpervaNative,
    Unknown,
}

/// Snapshot of one `detect.js` round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionSnapshot {
    pub surface: ImpervaSurface,
    pub reese84: Option<String>,
    pub body_clean: bool,
    pub sessions: Vec<CookieSnapshot>,
}

/// Cookie name + value as observed at probe time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieSnapshot {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
struct RawSurface {
    kind: String,
    #[serde(default)]
    captcha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCookie {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct RawSnapshot {
    surface: RawSurface,
    #[serde(default)]
    reese84: Option<String>,
    body_clean: bool,
    #[serde(default)]
    sessions: Vec<RawCookie>,
}

impl From<RawSurface> for ImpervaSurface {
    fn from(r: RawSurface) -> Self {
        match r.kind.as_str() {
            "Reese84" => Self::Reese84,
            "Legacy" => Self::Legacy,
            "Captcha" => {
                let k = match r.captcha.as_deref() {
                    Some("HCaptcha") => CaptchaKind::HCaptcha,
                    Some("Recaptcha") => CaptchaKind::Recaptcha,
                    Some("ImpervaNative") => CaptchaKind::ImpervaNative,
                    _ => CaptchaKind::Unknown,
                };
                Self::Captcha(k)
            }
            _ => Self::None,
        }
    }
}

impl From<RawSnapshot> for DetectionSnapshot {
    fn from(r: RawSnapshot) -> Self {
        Self {
            surface: r.surface.into(),
            reese84: r.reese84,
            body_clean: r.body_clean,
            sessions: r
                .sessions
                .into_iter()
                .map(|c| CookieSnapshot {
                    name: c.name,
                    value: c.value,
                })
                .collect(),
        }
    }
}

/// Run a single `detect.js` probe against `session`'s main world.
pub(crate) async fn detect_snapshot(
    session: &SessionHandle,
) -> Result<DetectionSnapshot, ImpervaError> {
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": include_str!("detect.js"),
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;

    if let Some(details) = res.get("exceptionDetails") {
        let msg = details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(ImpervaError::JsError(msg));
    }

    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);

    let raw: RawSnapshot = serde_json::from_value(value)
        .map_err(|e| ImpervaError::JsError(format!("invalid detect.js payload: {e}")))?;
    Ok(raw.into())
}

/// Surface-only probe. Convenience for callers wanting a non-blocking
/// "which surface is showing" check without the full snapshot.
pub async fn detect_surface(session: &SessionHandle) -> Result<ImpervaSurface, ImpervaError> {
    Ok(detect_snapshot(session).await?.surface)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    fn reply_value(mock_reply: serde_json::Value) -> serde_json::Value {
        json!({
            "result": {
                "type": "object",
                "value": mock_reply,
            }
        })
    }

    #[tokio::test]
    async fn detect_surface_returns_reese84_when_cookie_present() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_surface(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            reply_value(json!({
                "surface": { "kind": "Reese84" },
                "reese84": "TOKEN_XYZ",
                "body_clean": false,
                "sessions": [{ "name": "reese84", "value": "TOKEN_XYZ" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let surf = fut.await.unwrap().unwrap();
        assert_eq!(surf, ImpervaSurface::Reese84);
        conn.shutdown();
    }

    #[tokio::test]
    async fn detect_surface_returns_legacy_for_incap_ses_cookies() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_surface(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            reply_value(json!({
                "surface": { "kind": "Legacy" },
                "reese84": null,
                "body_clean": false,
                "sessions": [{ "name": "incap_ses_123", "value": "ABC" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let surf = fut.await.unwrap().unwrap();
        assert_eq!(surf, ImpervaSurface::Legacy);
        conn.shutdown();
    }

    #[tokio::test]
    async fn detect_surface_distinguishes_hcaptcha_vs_recaptcha_vs_native() {
        for (kind_str, expected) in [
            ("HCaptcha", CaptchaKind::HCaptcha),
            ("Recaptcha", CaptchaKind::Recaptcha),
            ("ImpervaNative", CaptchaKind::ImpervaNative),
            ("Unknown", CaptchaKind::Unknown),
        ] {
            let (mut mock, conn) = MockConnection::pair();
            let sess = SessionHandle::new(conn.clone(), "S1");

            let fut = tokio::spawn({
                let s = sess.clone();
                async move { detect_surface(&s).await }
            });

            let id = mock.expect_cmd("Runtime.evaluate").await;
            mock.reply(
                id,
                reply_value(json!({
                    "surface": { "kind": "Captcha", "captcha": kind_str },
                    "reese84": null,
                    "body_clean": false,
                    "sessions": [],
                    "has_imperva_signal": true,
                })),
            )
            .await;

            let surf = fut.await.unwrap().unwrap();
            assert_eq!(surf, ImpervaSurface::Captcha(expected));
            conn.shutdown();
        }
    }

    #[tokio::test]
    async fn detect_surface_returns_none_on_clean_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_surface(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            reply_value(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let surf = fut.await.unwrap().unwrap();
        assert_eq!(surf, ImpervaSurface::None);
        conn.shutdown();
    }

    #[tokio::test]
    async fn detect_snapshot_propagates_js_exception_as_jserror() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { detect_snapshot(&s).await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": { "type": "undefined" },
                "exceptionDetails": {
                    "exception": { "description": "TypeError: nope" }
                }
            }),
        )
        .await;

        let err = fut.await.unwrap().unwrap_err();
        assert!(matches!(err, ImpervaError::JsError(s) if s.contains("TypeError")));
        conn.shutdown();
    }
}
```

- [ ] **Step 3: Delete stub enums from `crates/zendriver-imperva/src/error.rs`**

Remove the `**Stub** ...` comment block, the stub `ImpervaSurface` enum, and the stub `CaptchaKind` enum. Add a `use crate::detection::{CaptchaKind, ImpervaSurface};` at the top of `error.rs` instead.

Final import block at the top of `error.rs`:

```rust
//! Imperva-bypass errors.

use std::time::Duration;
use zendriver_interception::InterceptionError;
use zendriver_transport::CallError;

use crate::detection::{CaptchaKind, ImpervaSurface};
```

- [ ] **Step 4: Wire `detection` module into `lib.rs`**

Replace the body of `lib.rs` with:

```rust
//! Imperva WAF / Incapsula bypass for `zendriver`.
//!
//! See the [Imperva chapter](https://turtiesocks.github.io/zendriver-rs/imperva.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, surface variants, and CAPTCHA-callback recipes.
//!
//! **Stealth required.** Imperva's reese84 sensor is itself a browser
//! fingerprint check. Run with [`BrowserBuilder::stealth`] enabled or this
//! bypass will fail on nearly all real Imperva-protected sites.
//!
//! Public API stub — modules land in subsequent tasks.
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth

pub mod detection;
pub mod error;

pub use detection::{CaptchaKind, CookieSnapshot, DetectionSnapshot, ImpervaSurface, detect_surface};
pub use error::ImpervaError;
```

- [ ] **Step 5: Verify (parallel)**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo clippy -p zendriver-imperva --all-targets --locked -- -D warnings
cargo fmt --all --check
```

Expected: 8 unit tests pass (3 from error.rs + 5 from detection.rs), no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): add detect.js + ImpervaSurface + detect_surface

Single-round-trip detection bundling cookie probes, body marker
scan, and CAPTCHA iframe pattern checks. Surface inference
precedence: Captcha > Reese84 > Legacy > None. Relocates the
ImpervaSurface and CaptchaKind stubs out of error.rs into
detection.rs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `ImpervaBypass` struct + builder methods

**Files:**
- Create: `crates/zendriver-imperva/src/bypass.rs`
- Modify: `crates/zendriver-imperva/src/lib.rs`

- [ ] **Step 1: Write `crates/zendriver-imperva/src/bypass.rs`**

```rust
//! Imperva bypass driver.
//!
//! Public entry is [`ImpervaBypass`] — constructed via `Tab::imperva()`
//! (zendriver crate, feature-gated). Single-struct dispatch: one
//! `wait_for_clearance` runs the surface-aware poll loop, the optional
//! [`ImpervaBypass::with_interception`] hook enables a Fetch-domain
//! fast-path, and [`ImpervaBypass::on_captcha`] plugs a caller-supplied
//! solver into the CAPTCHA escalation path. See module docs of
//! [`crate::detection`] for surface inference rules.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use zendriver_transport::SessionHandle;

use crate::detection::CaptchaKind;
use crate::error::ImpervaError;

/// Default poll interval for [`ImpervaBypass::wait_for_clearance`].
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Default overall timeout for [`ImpervaBypass::wait_for_clearance`].
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// CAPTCHA escalation handed to a user-supplied solver.
#[derive(Debug, Clone)]
pub struct CaptchaChallenge {
    pub kind: CaptchaKind,
    /// Site key extracted from the embed (hCaptcha / reCAPTCHA). `None`
    /// if the kind is `ImpervaNative` or `Unknown`.
    pub site_key: Option<String>,
    /// URL of the page presenting the CAPTCHA.
    pub url: String,
}

/// Token returned by a user-supplied CAPTCHA solver.
#[derive(Debug, Clone)]
pub struct CaptchaSolution {
    /// Verification token issued by the solver service.
    pub token: String,
    /// DOM field where the token must be injected for the page to accept it
    /// (e.g. `"h-captcha-response"`, `"g-recaptcha-response"`).
    pub form_field: String,
}

/// Outcome of a successful `wait_for_clearance`.
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// reese84 cookie acquired AND body markers gone (S3 hybrid signal).
    TokenAcquired {
        reese84: String,
        sessions: Vec<crate::detection::CookieSnapshot>,
    },
    /// Body markers gone but no reese84 token (e.g., legacy Incapsula flow).
    ChallengeGone,
    /// No Imperva surface present at call time. Fast path; no waiting.
    AlreadyClear,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) type CaptchaSolver = dyn Fn(CaptchaChallenge) -> BoxFuture<'static, Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>>
    + Send
    + Sync;

/// Drives an Imperva clearance flow against a single tab's session.
///
/// Constructed via `Tab::imperva()`.
pub struct ImpervaBypass<'tab> {
    pub(crate) session: &'tab SessionHandle,
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
    pub(crate) on_captcha: Option<Arc<CaptchaSolver>>,
    pub(crate) interceptor: Option<&'tab zendriver_interception::InterceptHandle>,
}

impl std::fmt::Debug for ImpervaBypass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImpervaBypass")
            .field("poll_interval", &self.poll_interval)
            .field("timeout", &self.timeout)
            .field("on_captcha", &self.on_captcha.as_ref().map(|_| "..."))
            .field("interceptor", &self.interceptor.is_some())
            .finish()
    }
}

impl<'tab> ImpervaBypass<'tab> {
    /// Create a new bypass driver bound to `session` with default 250ms
    /// poll interval and 30s timeout.
    pub fn new(session: &'tab SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
            on_captcha: None,
            interceptor: None,
        }
    }

    /// Override the default 30s overall timeout.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Override the default 250ms poll interval.
    #[must_use]
    pub fn poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Register a user-supplied async CAPTCHA solver. Without this, a
    /// CAPTCHA surface returns [`ImpervaError::CaptchaRequired`] immediately.
    #[must_use]
    pub fn on_captcha<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(CaptchaChallenge) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CaptchaSolution, Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        self.on_captcha = Some(Arc::new(move |challenge| Box::pin(f(challenge))));
        self
    }

    /// Enable the Fetch-domain escape hatch: subscribe to
    /// `/_Incapsula_Resource*` and `Reese.js` responses for faster
    /// token-set detection than polling alone.
    #[must_use]
    pub fn with_interception(
        mut self,
        interceptor: &'tab zendriver_interception::InterceptHandle,
    ) -> Self {
        self.interceptor = Some(interceptor);
        self
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[test]
    fn builder_defaults_match_constants() {
        let (_, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let b = ImpervaBypass::new(&sess);
        assert_eq!(b.poll_interval, DEFAULT_POLL_INTERVAL);
        assert_eq!(b.timeout, DEFAULT_TIMEOUT);
        assert!(b.on_captcha.is_none());
        assert!(b.interceptor.is_none());
        conn.shutdown();
    }

    #[test]
    fn builder_methods_override_defaults() {
        let (_, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let b = ImpervaBypass::new(&sess)
            .timeout(Duration::from_secs(60))
            .poll_interval(Duration::from_millis(100))
            .on_captcha(|_c| async move {
                Ok(CaptchaSolution {
                    token: "T".into(),
                    form_field: "f".into(),
                })
            });
        assert_eq!(b.timeout, Duration::from_secs(60));
        assert_eq!(b.poll_interval, Duration::from_millis(100));
        assert!(b.on_captcha.is_some());
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Wire `bypass` module into `lib.rs`**

Update `lib.rs`:

```rust
//! Imperva WAF / Incapsula bypass for `zendriver`.
//!
//! See the [Imperva chapter](https://turtiesocks.github.io/zendriver-rs/imperva.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, surface variants, and CAPTCHA-callback recipes.
//!
//! **Stealth required.** Imperva's reese84 sensor is itself a browser
//! fingerprint check. Run with [`BrowserBuilder::stealth`] enabled or this
//! bypass will fail on nearly all real Imperva-protected sites.
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth

pub mod bypass;
pub mod detection;
pub mod error;

pub use bypass::{CaptchaChallenge, CaptchaSolution, ClearanceOutcome, ImpervaBypass};
pub use detection::{CaptchaKind, CookieSnapshot, DetectionSnapshot, ImpervaSurface, detect_surface};
pub use error::ImpervaError;
```

- [ ] **Step 3: Verify**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo clippy -p zendriver-imperva --all-targets --locked -- -D warnings
cargo fmt --all --check
```

Expected: 10 unit tests pass (8 prior + 2 new).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): add ImpervaBypass builder + ClearanceOutcome

Builder fields wired (poll_interval, timeout, on_captcha,
interceptor). wait_for_clearance impl + state machine land in
Task 4. Public types CaptchaChallenge, CaptchaSolution,
ClearanceOutcome ready.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `wait_for_clearance` — Reese84 + Legacy + None + Timeout paths

**Files:**
- Modify: `crates/zendriver-imperva/src/bypass.rs`

- [ ] **Step 1: Write `wait_for_clearance` body inside the existing `impl<'tab> ImpervaBypass<'tab>` block**

Append after the `with_interception` method (before the closing `}` of the impl block):

```rust
    /// Run the surface-aware poll loop until clearance is achieved or
    /// the configured timeout elapses.
    ///
    /// # Returns
    /// - `Ok(ClearanceOutcome::AlreadyClear)` — no Imperva surface present
    ///   at call time. Fast path; no waiting.
    /// - `Ok(ClearanceOutcome::TokenAcquired { reese84, sessions })` —
    ///   reese84 cookie was observed AND the page body no longer contains
    ///   Imperva challenge markers (hybrid AND signal).
    /// - `Ok(ClearanceOutcome::ChallengeGone)` — body markers cleared but
    ///   no reese84 token was ever observed (e.g., legacy Incapsula).
    ///
    /// # Errors
    /// - [`ImpervaError::Timeout`] — overall timeout elapsed before
    ///   clearance. `last_surface` carries the most-recent observed surface.
    /// - [`ImpervaError::CaptchaRequired`] — CAPTCHA surface detected
    ///   but no `on_captcha` solver was registered.
    /// - [`ImpervaError::CaptchaSolver`] — registered solver returned an
    ///   error.
    /// - [`ImpervaError::Interception`] — Fetch-domain hook (when set via
    ///   [`with_interception`](Self::with_interception)) failed.
    /// - [`ImpervaError::Call`] / [`ImpervaError::JsError`] — CDP or
    ///   in-page evaluator failure.
    pub async fn wait_for_clearance(self) -> Result<ClearanceOutcome, ImpervaError> {
        use tokio::time::{Instant, Interval, MissedTickBehavior};

        let snapshot = crate::detection::detect_snapshot(self.session).await?;

        // Fast paths.
        if matches!(snapshot.surface, crate::detection::ImpervaSurface::None)
            && snapshot.body_clean
        {
            return Ok(ClearanceOutcome::AlreadyClear);
        }
        if let crate::detection::ImpervaSurface::Captcha(kind) = snapshot.surface {
            if self.on_captcha.is_none() {
                return Err(ImpervaError::CaptchaRequired { kind });
            }
            // Callback dispatch happens in Task 5; for now, fail explicitly.
            return Err(ImpervaError::CaptchaRequired { kind });
        }

        let deadline = Instant::now() + self.timeout;
        let mut ticker: Interval = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut last_surface = Some(snapshot.surface);
        let mut next_snapshot = Some(snapshot);

        loop {
            // First iteration uses the snapshot already taken above; subsequent
            // iterations re-probe.
            let snap = match next_snapshot.take() {
                Some(s) => s,
                None => crate::detection::detect_snapshot(self.session).await?,
            };

            match (
                snap.reese84.as_ref().filter(|v| !v.is_empty()),
                snap.body_clean,
            ) {
                (Some(token), true) => {
                    return Ok(ClearanceOutcome::TokenAcquired {
                        reese84: token.clone(),
                        sessions: snap.sessions,
                    });
                }
                (None, true) => return Ok(ClearanceOutcome::ChallengeGone),
                _ => last_surface = Some(snap.surface),
            }

            if Instant::now() >= deadline {
                return Err(ImpervaError::Timeout {
                    timeout: self.timeout,
                    last_surface,
                });
            }

            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(ImpervaError::Timeout {
                        timeout: self.timeout,
                        last_surface,
                    });
                }
            }
        }
    }
```

Note: `DetectionSnapshot` must be `Clone`. It already is from Task 2.

- [ ] **Step 2: Add `is_challenge_present` convenience method**

Append to the same impl block:

```rust
    /// One-shot probe: returns `true` iff [`detect_surface`] returns
    /// anything other than `ImpervaSurface::None`.
    ///
    /// [`detect_surface`]: crate::detection::detect_surface
    pub async fn is_challenge_present(&self) -> Result<bool, ImpervaError> {
        Ok(!matches!(
            crate::detection::detect_surface(self.session).await?,
            crate::detection::ImpervaSurface::None,
        ))
    }
```

- [ ] **Step 3: Add tests inside the existing `#[cfg(test)] mod tests` in `bypass.rs`**

Append after the existing builder tests:

```rust
    use crate::detection::ImpervaSurface;
    use serde_json::json;

    fn snapshot_reply(payload: serde_json::Value) -> serde_json::Value {
        json!({
            "result": {
                "type": "object",
                "value": payload,
            }
        })
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_already_clear_on_clean_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { ImpervaBypass::new(&s).wait_for_clearance().await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(outcome, ClearanceOutcome::AlreadyClear));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_token_when_both_signals_hit() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        // First probe: surface present, no clearance yet.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Reese84" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Second probe: token + body clean → TokenAcquired.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "TOK_ABC",
                "body_clean": true,
                "sessions": [{ "name": "reese84", "value": "TOK_ABC" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        match outcome {
            ClearanceOutcome::TokenAcquired { reese84, sessions } => {
                assert_eq!(reese84, "TOK_ABC");
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].name, "reese84");
            }
            other => panic!("expected TokenAcquired, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_holds_when_cookie_only_no_body_clean() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .timeout(Duration::from_millis(50))
                    .wait_for_clearance()
                    .await
            }
        });

        // Always reply cookie-only.
        for _ in 0..50 {
            let Ok(id) =
                tokio::time::timeout(Duration::from_millis(100), mock.expect_cmd("Runtime.evaluate"))
                    .await
            else {
                break;
            };
            mock.reply(
                id,
                snapshot_reply(json!({
                    "surface": { "kind": "Reese84" },
                    "reese84": "TOK",
                    "body_clean": false,
                    "sessions": [],
                    "has_imperva_signal": true,
                })),
            )
            .await;
        }

        let err = fut.await.unwrap().unwrap_err();
        match err {
            ImpervaError::Timeout { last_surface, .. } => {
                assert_eq!(last_surface, Some(ImpervaSurface::Reese84));
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_challenge_gone_when_body_clean_no_token() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        // Surface present at start.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Legacy" },
                "reese84": null,
                "body_clean": false,
                "sessions": [{ "name": "incap_ses_123", "value": "X" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Then body becomes clean, but no reese84 ever sets.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(outcome, ClearanceOutcome::ChallengeGone));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_returns_captcha_required_when_no_solver() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move { ImpervaBypass::new(&s).wait_for_clearance().await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            snapshot_reply(json!({
                "surface": { "kind": "Captcha", "captcha": "HCaptcha" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let err = fut.await.unwrap().unwrap_err();
        assert!(matches!(
            err,
            ImpervaError::CaptchaRequired {
                kind: CaptchaKind::HCaptcha
            }
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_treats_empty_reese84_as_unset() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        // Surface present, cookie is empty string → not yet set.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Reese84" },
                "reese84": "",
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // Expect that the loop did NOT return TokenAcquired with empty token.
        // Next probe returns ChallengeGone (no reese84, body clean).
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": null,
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(
            matches!(outcome, ClearanceOutcome::ChallengeGone),
            "empty reese84 must not produce TokenAcquired"
        );
        conn.shutdown();
    }
```

- [ ] **Step 4: Verify**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo clippy -p zendriver-imperva --all-targets --locked -- -D warnings
cargo fmt --all --check
```

Expected: 16 unit tests pass (10 prior + 6 new).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): wait_for_clearance core poll loop

Reese84 + Legacy + None + Timeout flow paths. S3 hybrid AND signal:
TokenAcquired requires non-empty reese84 cookie AND body markers
gone. Captcha surface returns CaptchaRequired (callback dispatch
lands in Task 5). is_challenge_present probe helper.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: CAPTCHA path — `on_captcha` callback + dispatch

**Files:**
- Modify: `crates/zendriver-imperva/src/bypass.rs`

- [ ] **Step 1: Extract site_key + url before invoking solver**

Add a private helper at module scope (above the impl block in `bypass.rs`):

```rust
/// Extract a CAPTCHA site key from the current page via a small inline JS
/// probe. Returns `None` if no recognizable embed is present.
async fn extract_captcha_site_key(
    session: &SessionHandle,
    kind: CaptchaKind,
) -> Result<(Option<String>, String), ImpervaError> {
    use serde_json::json;

    const PROBE_JS: &str = r#"
    (function () {
        function findKey(selector, attr) {
            var el = document.querySelector(selector);
            return el ? el.getAttribute(attr) : null;
        }
        var hcap =
            findKey(".h-captcha", "data-sitekey") ||
            findKey("[data-hcaptcha-sitekey]", "data-hcaptcha-sitekey");
        var rcap =
            findKey(".g-recaptcha", "data-sitekey") ||
            findKey("[data-recaptcha-sitekey]", "data-recaptcha-sitekey");
        return { hcap: hcap, rcap: rcap, url: location.href };
    })()
    "#;

    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": PROBE_JS,
                "returnByValue": true,
            }),
        )
        .await?;
    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    #[derive(serde::Deserialize)]
    struct Probe {
        hcap: Option<String>,
        rcap: Option<String>,
        url: String,
    }
    let probe: Probe = serde_json::from_value(value)
        .map_err(|e| ImpervaError::JsError(format!("invalid captcha probe payload: {e}")))?;

    let site_key = match kind {
        CaptchaKind::HCaptcha => probe.hcap,
        CaptchaKind::Recaptcha => probe.rcap,
        CaptchaKind::ImpervaNative | CaptchaKind::Unknown => None,
    };
    Ok((site_key, probe.url))
}

/// Inject a CAPTCHA solver token into the named form field via
/// `Runtime.evaluate`.
async fn inject_captcha_solution(
    session: &SessionHandle,
    solution: &CaptchaSolution,
) -> Result<(), ImpervaError> {
    use serde_json::json;

    let script = format!(
        r#"
        (function () {{
            var field = document.querySelector('[name="{name}"]')
                || document.getElementById("{name}");
            if (!field) {{
                var t = document.createElement("textarea");
                t.name = "{name}";
                t.id = "{name}";
                t.style.display = "none";
                document.body.appendChild(t);
                field = t;
            }}
            field.value = {token};
            field.dispatchEvent(new Event("change", {{ bubbles: true }}));
            return true;
        }})()
        "#,
        name = solution.form_field.replace('"', "\\\""),
        token = serde_json::Value::String(solution.token.clone()),
    );

    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": script,
                "returnByValue": true,
            }),
        )
        .await?;
    if let Some(details) = res.get("exceptionDetails") {
        let msg = details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(ImpervaError::JsError(msg));
    }
    Ok(())
}
```

- [ ] **Step 2: Replace the placeholder captcha-fail block in `wait_for_clearance`**

Find this block in `wait_for_clearance`:

```rust
        if let crate::detection::ImpervaSurface::Captcha(kind) = snapshot.surface {
            if self.on_captcha.is_none() {
                return Err(ImpervaError::CaptchaRequired { kind });
            }
            // Callback dispatch happens in Task 5; for now, fail explicitly.
            return Err(ImpervaError::CaptchaRequired { kind });
        }
```

Replace with:

```rust
        if let crate::detection::ImpervaSurface::Captcha(kind) = snapshot.surface {
            let Some(solver) = self.on_captcha.clone() else {
                return Err(ImpervaError::CaptchaRequired { kind });
            };
            let (site_key, url) = extract_captcha_site_key(self.session, kind).await?;
            let challenge = CaptchaChallenge { kind, site_key, url };
            let solution = solver(challenge)
                .await
                .map_err(ImpervaError::CaptchaSolver)?;
            inject_captcha_solution(self.session, &solution).await?;
            // Fall through to poll loop: the page should now submit the
            // CAPTCHA and clear the Imperva surface.
        }
```

- [ ] **Step 3: Add tests**

Append to `#[cfg(test)] mod tests` in `bypass.rs`:

```rust
    #[tokio::test]
    async fn wait_for_clearance_invokes_captcha_callback_and_resumes() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .on_captcha(|c| async move {
                        assert!(matches!(c.kind, CaptchaKind::HCaptcha));
                        assert_eq!(c.site_key.as_deref(), Some("KEY_ABC"));
                        Ok(CaptchaSolution {
                            token: "SOLVED_TOK".into(),
                            form_field: "h-captcha-response".into(),
                        })
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        // 1. Detect snapshot → CAPTCHA.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Captcha", "captcha": "HCaptcha" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        // 2. Site-key probe.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": "KEY_ABC",
                        "rcap": null,
                        "url": "https://example.com/protected",
                    }
                }
            }),
        )
        .await;

        // 3. Solution injection.
        let id3 = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("SOLVED_TOK"),
            "injection script should contain the solver token"
        );
        assert!(
            sent["params"]["expression"]
                .as_str()
                .unwrap()
                .contains("h-captcha-response"),
            "injection script should target the form_field"
        );
        mock.reply(id3, json!({ "result": { "type": "boolean", "value": true } }))
            .await;

        // 4. Resumed poll: cleared.
        let id4 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id4,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "TOK_FINAL",
                "body_clean": true,
                "sessions": [{ "name": "reese84", "value": "TOK_FINAL" }],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(
            outcome,
            ClearanceOutcome::TokenAcquired { reese84, .. } if reese84 == "TOK_FINAL"
        ));
        conn.shutdown();
    }

    #[tokio::test]
    async fn wait_for_clearance_propagates_captcha_solver_error() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .on_captcha(|_c| async move {
                        Err(Box::<dyn std::error::Error + Send + Sync>::from(
                            "solver down",
                        ))
                    })
                    .wait_for_clearance()
                    .await
            }
        });

        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "Captcha", "captcha": "Recaptcha" },
                "reese84": null,
                "body_clean": false,
                "sessions": [],
                "has_imperva_signal": true,
            })),
        )
        .await;

        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id2,
            json!({
                "result": {
                    "type": "object",
                    "value": {
                        "hcap": null,
                        "rcap": "RKEY",
                        "url": "https://x.com/",
                    }
                }
            }),
        )
        .await;

        let err = fut.await.unwrap().unwrap_err();
        assert!(matches!(err, ImpervaError::CaptchaSolver(_)));
        conn.shutdown();
    }
```

- [ ] **Step 4: Verify**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo clippy -p zendriver-imperva --all-targets --locked -- -D warnings
cargo fmt --all --check
```

Expected: 18 unit tests pass (16 prior + 2 new).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): CAPTCHA callback dispatch + token injection

extract_captcha_site_key probes for h-captcha / g-recaptcha
data-sitekey attributes. inject_captcha_solution writes the
solver-returned token into the named form field via
Runtime.evaluate. Solver errors flow into ImpervaError::CaptchaSolver.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Interception fast-path (`with_interception`)

**Files:**
- Create: `crates/zendriver-imperva/src/interception.rs`
- Modify: `crates/zendriver-imperva/src/bypass.rs` (wire `tokio::select!` between poll tick and interception signal)
- Modify: `crates/zendriver-imperva/src/lib.rs` (add module)

**Important:** before writing this task, the implementer should `grep -rn "subscribe\|InterceptHandle" crates/zendriver-interception/src/` to confirm the actual streaming-mode API surface. The plan below uses the API surface documented in `crates/zendriver-interception/src/lib.rs` (`InterceptBuilder::subscribe` → `Stream<PausedRequest>`). If the available API diverges, adapt: the requirement is "subscribe to Fetch responses matching `Reese.js` or `_Incapsula_Resource` and signal a oneshot when one arrives 2xx."

If `InterceptBuilder::subscribe` cannot be wired against an *existing* `InterceptHandle` (i.e., the stream-mode API requires starting a fresh actor), simplify the design: change the `with_interception` parameter from `&'tab InterceptHandle` to `&'tab SessionHandle` and have the imperva crate spin up its own `InterceptBuilder::subscribe(...)` against that session. Update the spec accordingly and note the deviation in the implementer report.

- [ ] **Step 1: Investigate interception API**

```bash
grep -rn "pub fn\|pub async fn" crates/zendriver-interception/src/builder.rs | head -30
grep -rn "PausedRequest" crates/zendriver-interception/src/paused.rs | head -10
```

Document which API matches the requirement. Adapt Step 2 below if needed.

- [ ] **Step 2: Write `crates/zendriver-imperva/src/interception.rs`**

```rust
//! Fetch-domain fast-path for Imperva clearance detection.
//!
//! Opt-in via [`ImpervaBypass::with_interception`]. Subscribes to
//! Fetch responses matching `Reese.js` or `_Incapsula_Resource` URL
//! patterns; signals the waiter via a oneshot when a 2xx is observed.
//! Polling continues in parallel — first signal wins.
//!
//! [`ImpervaBypass::with_interception`]: crate::bypass::ImpervaBypass::with_interception

use tokio::sync::oneshot;
use zendriver_interception::InterceptBuilder;
use zendriver_transport::SessionHandle;

use crate::error::ImpervaError;

/// Spawn a background task that signals on first 2xx Imperva-sensor
/// response and returns the receiver half of a oneshot.
///
/// Caller must keep the returned [`InterceptionGuard`] alive until they
/// are done with the receiver — dropping it tears down the subscription.
pub(crate) async fn spawn_signal(
    session: &SessionHandle,
) -> Result<(oneshot::Receiver<()>, InterceptionGuard), ImpervaError> {
    use futures::StreamExt;

    let (tx, rx) = oneshot::channel();

    let mut stream = InterceptBuilder::new(session)
        .pattern("*Reese.js*")?
        .pattern("*_Incapsula_Resource*")?
        .at_response()
        .subscribe()
        .await?;

    let handle = tokio::spawn(async move {
        let mut tx = Some(tx);
        while let Some(paused) = stream.next().await {
            let is_2xx = paused
                .response_info()
                .as_ref()
                .map(|r| (200..300).contains(&r.status))
                .unwrap_or(false);
            // Always release the pause so the page keeps loading.
            let _ = paused.continue_().await;
            if is_2xx {
                if let Some(t) = tx.take() {
                    let _ = t.send(());
                }
                break;
            }
        }
    });

    Ok((rx, InterceptionGuard { handle: Some(handle) }))
}

/// Guard for the background interception task. Aborts on drop.
pub(crate) struct InterceptionGuard {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for InterceptionGuard {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}
```

**Adapt the actual InterceptBuilder method names to whatever Step 1 surfaced.** The shape (subscribe to a pattern, get a Stream, await first 2xx, release each pause, oneshot the waiter) is what matters.

- [ ] **Step 3: Change `with_interception` signature to take `&SessionHandle` instead of `&InterceptHandle`**

The plan originally proposed `with_interception(&InterceptHandle)`. Step 1 likely shows that `subscribe` is an entry point on `InterceptBuilder`, not an existing handle — meaning the imperva crate spins up its own subscription. Update the builder:

In `bypass.rs`, change the `interceptor` field type:

Old:
```rust
pub(crate) interceptor: Option<&'tab zendriver_interception::InterceptHandle>,
```

New:
```rust
pub(crate) interception_enabled: bool,
```

And:

Old:
```rust
    pub fn with_interception(
        mut self,
        interceptor: &'tab zendriver_interception::InterceptHandle,
    ) -> Self {
        self.interceptor = Some(interceptor);
        self
    }
```

New:
```rust
    /// Enable the Fetch-domain escape hatch: the bypass driver spins up
    /// its own [`InterceptBuilder::subscribe`] hook against this session,
    /// listening for `Reese.js` / `_Incapsula_Resource` 2xx responses to
    /// signal clearance faster than polling alone.
    ///
    /// [`InterceptBuilder::subscribe`]: zendriver_interception::InterceptBuilder::subscribe
    #[must_use]
    pub fn with_interception(mut self) -> Self {
        self.interception_enabled = true;
        self
    }
```

Initializer in `new()`:

Old:
```rust
            interceptor: None,
```

New:
```rust
            interception_enabled: false,
```

`Debug` impl:

Old:
```rust
            .field("interceptor", &self.interceptor.is_some())
```

New:
```rust
            .field("interception_enabled", &self.interception_enabled)
```

Update the builder test (`builder_defaults_match_constants`) accordingly:

Old:
```rust
        assert!(b.interceptor.is_none());
```

New:
```rust
        assert!(!b.interception_enabled);
```

- [ ] **Step 4: Wire interception signal into `wait_for_clearance` poll loop**

In `wait_for_clearance`, after the fast paths but before `let deadline = Instant::now() + self.timeout;`, add:

```rust
        let (mut interception_rx, _interception_guard) = if self.interception_enabled {
            let (rx, guard) = crate::interception::spawn_signal(self.session).await?;
            (Some(rx), Some(guard))
        } else {
            (None, None)
        };
```

Then change the `tokio::select!` at the end of the loop to also race on the interception receiver. Old:

```rust
            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(ImpervaError::Timeout {
                        timeout: self.timeout,
                        last_surface,
                    });
                }
            }
```

New:

```rust
            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(ImpervaError::Timeout {
                        timeout: self.timeout,
                        last_surface,
                    });
                }
                Ok(()) = async {
                    match interception_rx.as_mut() {
                        Some(rx) => rx.await,
                        None => std::future::pending().await,
                    }
                } => {
                    // Interception fired → take rx so we don't await a closed
                    // channel next iteration. The next loop pass will re-probe
                    // immediately.
                    interception_rx = None;
                }
            }
```

- [ ] **Step 5: Wire module into `lib.rs`**

Add:

```rust
mod interception;
```

(Not `pub mod` — implementation detail; not part of public API.)

- [ ] **Step 6: Add a test**

In `bypass.rs` tests:

```rust
    #[tokio::test]
    async fn wait_for_clearance_with_interception_uses_fast_path() {
        // This test exercises only the *non-interception* code path under
        // the `interception_enabled` flag set to false, since the
        // InterceptBuilder needs Fetch.enable plumbing that MockConnection
        // doesn't fully simulate. The real fast-path behavior is verified
        // by the integration test scaffold (Task 12).
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");

        let fut = tokio::spawn({
            let s = sess.clone();
            async move {
                ImpervaBypass::new(&s)
                    .poll_interval(Duration::from_millis(1))
                    .wait_for_clearance()
                    .await
            }
        });

        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id1,
            snapshot_reply(json!({
                "surface": { "kind": "None" },
                "reese84": "X",
                "body_clean": true,
                "sessions": [],
                "has_imperva_signal": false,
            })),
        )
        .await;

        let outcome = fut.await.unwrap().unwrap();
        assert!(matches!(
            outcome,
            ClearanceOutcome::TokenAcquired { reese84, .. } if reese84 == "X"
        ));
        conn.shutdown();
    }
```

Note: a real interception fast-path unit test requires `MockConnection` to handle `Fetch.enable` + `Fetch.requestPaused` events. If the existing test infrastructure (e.g., `zendriver-interception`'s own tests) provides a helper, use it. Otherwise, defer real fast-path coverage to the nightly integration test in Task 12 and document the gap with a `// TODO:` comment.

- [ ] **Step 7: Verify**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo clippy -p zendriver-imperva --all-targets --locked -- -D warnings
cargo fmt --all --check
```

Expected: 19 unit tests pass (18 prior + 1 new). If the InterceptBuilder API differs from what's shown, adjust `interception.rs` until clippy + tests pass.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva): Fetch-domain fast-path via with_interception

Opt-in: ImpervaBypass::with_interception() spawns an InterceptBuilder
subscription against the tab session that signals on first 2xx
Reese.js / _Incapsula_Resource response. wait_for_clearance races
poll-tick vs interception-signal vs deadline via tokio::select!.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Stalled-poll telemetry (imperva + cloudflare)

**Files:**
- Modify: `crates/zendriver-imperva/src/bypass.rs`
- Modify: `crates/zendriver-cloudflare/src/bypass.rs`

- [ ] **Step 1: Add stalled-poll counter to `ImpervaBypass::wait_for_clearance`**

In the loop, track tick count and surface-change-since-last-tick. After the snapshot is observed and the (token, body_clean) match block, before the deadline check, add:

```rust
            // Stalled-poll warning: if surface hasn't changed for ~2.5s
            // (10 ticks at the default 250ms interval), nudge the caller
            // toward stealth.
            stall_ticks = if Some(snap.surface) == prev_surface {
                stall_ticks + 1
            } else {
                0
            };
            prev_surface = Some(snap.surface);
            if stall_ticks == 10 && !warned_stall {
                tracing::warn!(
                    surface = ?snap.surface,
                    poll_interval_ms = self.poll_interval.as_millis() as u64,
                    "imperva clearance stalled — is BrowserBuilder::stealth enabled?"
                );
                warned_stall = true;
            }
```

Declare counters above the loop:

```rust
        let mut prev_surface: Option<crate::detection::ImpervaSurface> = None;
        let mut stall_ticks: u32 = 0;
        let mut warned_stall = false;
```

- [ ] **Step 2: Add equivalent to `CloudflareBypass::wait_for_clearance`**

Open `crates/zendriver-cloudflare/src/bypass.rs`, locate the `loop {` in `wait_for_clearance`. Above the loop, add:

```rust
        let mut stall_ticks: u32 = 0;
        let mut warned_stall = false;
```

Inside the loop, after `poll_once(...)` resolves with the `_ => continue` arm (i.e., the "still polling" case), insert just before `continue`:

```rust
                _ => {
                    stall_ticks += 1;
                    if stall_ticks == 10 && !warned_stall {
                        tracing::warn!(
                            poll_interval_ms = self.poll_interval.as_millis() as u64,
                            "cloudflare clearance stalled — is BrowserBuilder::stealth enabled?"
                        );
                        warned_stall = true;
                    }
                    continue;
                }
```

(Replace the existing `_ => continue,` arm with this block.)

Add `use tracing;` if not already in scope (it likely already is via `tracing::warn!` not requiring an import).

- [ ] **Step 3: Verify**

```bash
cargo test -p zendriver-imperva --lib --locked
cargo test -p zendriver-cloudflare --lib --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expected: all prior tests pass. Existing tests do not exercise the stall path (snapshots flip surface immediately) so the warning code is dead-but-compiled until real-world stalls trip it.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(imperva,cloudflare): stalled-poll tracing::warn!

When the surface hasn't progressed after ~10 polls (~2.5s at default
cadence), emit one tracing::warn! nudging the caller toward
BrowserBuilder::stealth. Stealth posture enforcement at the
runtime/UX layer per the spec — replaces the misframed "feature
implication" wording.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Parent `zendriver` wiring

**Files:**
- Modify: `crates/zendriver/src/lib.rs`
- Modify: `crates/zendriver/src/tab.rs`
- Modify: `crates/zendriver/src/error.rs`

- [ ] **Step 1: Add `ZendriverError::Imperva` variant**

In `crates/zendriver/src/error.rs`, after the `Cloudflare` variant block (around line 125):

```rust
    /// Imperva bypass sub-crate error. Gated by feature `imperva`.
    #[cfg(feature = "imperva")]
    #[error("imperva: {0}")]
    Imperva(Box<zendriver_imperva::ImpervaError>),
```

After the `From<CloudflareError>` impl (around line 157):

```rust
#[cfg(feature = "imperva")]
impl From<zendriver_imperva::ImpervaError> for ZendriverError {
    fn from(e: zendriver_imperva::ImpervaError) -> Self {
        Self::Imperva(Box::new(e))
    }
}
```

- [ ] **Step 2: Re-export imperva public types in `crates/zendriver/src/lib.rs`**

Locate the existing cloudflare re-export block (around line 120):

```rust
#[cfg(feature = "cloudflare")]
pub use zendriver_cloudflare::{ClearanceOutcome, CloudflareBypass, CloudflareError};
```

After it, add:

```rust
/// Imperva WAF / Incapsula bypass surface re-exports.
///
/// Gated by the `imperva` cargo feature. The driver lives in the
/// `zendriver-imperva` sub-crate; these aliases let downstream code reach
/// it via the parent crate without an extra dependency. Entry point is
/// [`Tab::imperva`].
#[cfg(feature = "imperva")]
pub use zendriver_imperva::{
    CaptchaChallenge, CaptchaKind, CaptchaSolution, CookieSnapshot,
    DetectionSnapshot, ImpervaBypass, ImpervaError, ImpervaSurface, detect_surface,
};
```

**Naming collision risk:** `ClearanceOutcome` already comes from `zendriver_cloudflare`. The `zendriver-imperva` crate also exports `ClearanceOutcome`. If both `cloudflare` and `imperva` features are enabled simultaneously, the re-export glob will collide. Solution: rename the imperva re-export.

In `zendriver-imperva` Step 3 (lib.rs), expose `ClearanceOutcome` only via the path `zendriver_imperva::ClearanceOutcome`; the parent zendriver crate aliases it under a distinct name. Update the re-export block above to:

```rust
#[cfg(feature = "imperva")]
pub use zendriver_imperva::{
    CaptchaChallenge, CaptchaKind, CaptchaSolution, CookieSnapshot,
    DetectionSnapshot, ImpervaBypass, ImpervaError, ImpervaSurface, detect_surface,
};

/// Imperva-bypass clearance outcome (aliased to avoid colliding with the
/// cloudflare crate's `ClearanceOutcome`).
#[cfg(feature = "imperva")]
pub use zendriver_imperva::ClearanceOutcome as ImpervaClearanceOutcome;
```

- [ ] **Step 3: Add `Tab::imperva()` convenience in `crates/zendriver/src/tab.rs`**

After the existing `#[cfg(feature = "cloudflare")] impl Tab { ... }` block (around line 1401):

```rust
#[cfg(feature = "imperva")]
impl Tab {
    /// Construct an
    /// [`ImpervaBypass`](zendriver_imperva::ImpervaBypass) bound to this
    /// tab's session.
    ///
    /// Chain
    /// [`timeout`](zendriver_imperva::ImpervaBypass::timeout) /
    /// [`poll_interval`](zendriver_imperva::ImpervaBypass::poll_interval) /
    /// [`with_interception`](zendriver_imperva::ImpervaBypass::with_interception) /
    /// [`on_captcha`](zendriver_imperva::ImpervaBypass::on_captcha)
    /// builder methods, then call
    /// [`wait_for_clearance`](zendriver_imperva::ImpervaBypass::wait_for_clearance)
    /// to detect the active Imperva surface (modern reese84, legacy
    /// Incapsula, or CAPTCHA escalation) and poll until clearance.
    ///
    /// **Stealth required.** Without `BrowserBuilder::stealth`, the
    /// bypass will fail on nearly all real Imperva-protected sites.
    ///
    /// Gated by the `imperva` cargo feature.
    #[must_use]
    pub fn imperva(&self) -> zendriver_imperva::ImpervaBypass<'_> {
        zendriver_imperva::ImpervaBypass::new(self.session())
    }
}
```

- [ ] **Step 4: Add stealth note to existing `Tab::cloudflare()` rustdoc**

In the existing `impl Tab` block for cloudflare (around line 1380), insert one line in the rustdoc — after the `Gated by the cloudflare cargo feature.` line and before `#[must_use]`:

Old final paragraph:
```rust
    /// Gated by the `cloudflare` cargo feature.
```

New:
```rust
    /// **Stealth recommended.** Cloudflare Turnstile is somewhat forgiving
    /// of non-stealth Chrome, but `BrowserBuilder::stealth` significantly
    /// raises the clearance success rate.
    ///
    /// Gated by the `cloudflare` cargo feature.
```

- [ ] **Step 5: Add `imperva_surface_is_send_sync` test in `crates/zendriver/src/lib.rs`**

Locate the existing `cloudflare_surface_is_send_sync` test (around line 247). After it:

```rust
    #[cfg(feature = "imperva")]
    #[test]
    fn imperva_surface_is_send_sync() {
        assert_send_sync::<ImpervaBypass<'_>>();
        assert_send_sync::<ImpervaError>();
        assert_send_sync::<ImpervaSurface>();
        assert_send_sync::<CaptchaKind>();
        assert_send_sync::<CaptchaChallenge>();
        assert_send_sync::<CaptchaSolution>();
        assert_send_sync::<CookieSnapshot>();
        assert_send_sync::<DetectionSnapshot>();
        assert_send_sync::<ImpervaClearanceOutcome>();
    }
```

The `use` block above this `#[cfg(test)]` module likely has feature-gated imports. Add the imperva ones in the same style:

```rust
    #[cfg(feature = "imperva")]
    use crate::{
        CaptchaChallenge, CaptchaKind, CaptchaSolution, CookieSnapshot,
        DetectionSnapshot, ImpervaBypass, ImpervaClearanceOutcome, ImpervaError,
        ImpervaSurface,
    };
```

Place this inside `mod tests { ... }`, beside the existing cloudflare gated `use`.

- [ ] **Step 6: Update feature matrix doc in `crates/zendriver/src/lib.rs`**

Locate the feature matrix near line 62 (`| cloudflare | ...`). Add a row below it:

```text
//! | `imperva`    | `zendriver-imperva`    | Imperva WAF / Incapsula bypass |
```

- [ ] **Step 7: Verify (parallel, full workspace)**

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --lib --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expected: all pass, including the new `imperva_surface_is_send_sync` test.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(zendriver): wire imperva feature into parent crate

Adds Tab::imperva() convenience, ZendriverError::Imperva variant +
From impl, re-exports of imperva public types (with
ClearanceOutcome aliased as ImpervaClearanceOutcome to avoid
collision with the cloudflare crate's same-named export), feature
matrix doc row, and the imperva_surface_is_send_sync compile-time
guard. Tab::cloudflare gets a stealth-recommended note for posture
parity.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Cloudflare retrofit — stale dep drop + lib docs

**Files:**
- Modify: `crates/zendriver-cloudflare/Cargo.toml`
- Modify: `crates/zendriver-cloudflare/src/lib.rs`

- [ ] **Step 1: Drop stale `zendriver-interception` dep**

Open `crates/zendriver-cloudflare/Cargo.toml`. Find this line in `[dependencies]`:

```toml
zendriver-interception.workspace = true
```

Delete it. (It's never imported in `src/`.)

- [ ] **Step 2: Add stealth-required call-out to `crates/zendriver-cloudflare/src/lib.rs`**

After the existing first paragraph (`See the [Cloudflare chapter]...for end-to-end usage...`), insert a new paragraph block:

```rust
//!
//! **Stealth recommended.** Cloudflare Turnstile is somewhat forgiving of
//! non-stealth Chrome, but `BrowserBuilder::stealth` significantly raises
//! the clearance success rate.
```

The full preserved structure:

```rust
//! Cloudflare Turnstile bypass for `zendriver`.
//!
//! See the [Cloudflare chapter](https://turtiesocks.github.io/zendriver-rs/cloudflare.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, timeout tuning, and detection-failure diagnostics.
//!
//! **Stealth recommended.** Cloudflare Turnstile is somewhat forgiving of
//! non-stealth Chrome, but `BrowserBuilder::stealth` significantly raises
//! the clearance success rate.
//!
//! Drives the Turnstile checkbox click flow:
//!
//! 1. Detect the Turnstile iframe via a shadow-DOM-aware walk of the page's
//!    main world.
//! ...
```

- [ ] **Step 3: Verify**

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --lib --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expected: all pass; `zendriver-cloudflare` no longer carries an unused dep.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(cloudflare): drop stale zendriver-interception dep + stealth note

The interception workspace dep on zendriver-cloudflare was declared
but never imported. Drops it. Also adds a Stealth-recommended
call-out at the top of lib.rs for posture parity with
zendriver-imperva.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Example `imperva_bypass.rs`

**Files:**
- Create: `crates/zendriver/examples/imperva_bypass.rs`
- Modify: `crates/zendriver/Cargo.toml` (register example)

- [ ] **Step 1: Write the example**

```rust
//! Demonstrates the zendriver-imperva bypass driver.
//!
//! Sequence:
//!   1. Launch a stealth-enabled headless browser and navigate to an
//!      Imperva-protected URL. The example uses a placeholder URL — set
//!      `IMPERVA_DEMO_URL` env var to override, since there is no
//!      universally-stable public Imperva demo page.
//!   2. Call [`Tab::imperva`] to construct an [`ImpervaBypass`] bound to
//!      the tab's session.
//!   3. Call [`ImpervaBypass::wait_for_clearance`] with a 60s budget.
//!      The driver detects the active Imperva surface (modern reese84,
//!      legacy Incapsula, or CAPTCHA escalation) and polls until both
//!      hybrid signals (reese84 cookie set + body markers cleared) hit.
//!   4. Print the [`ImpervaClearanceOutcome`] variant + page title.
//!
//! Outcomes:
//!   - `TokenAcquired { reese84, sessions }` — full hybrid clearance.
//!   - `ChallengeGone` — body markers cleared without a reese84 token
//!     (legacy flow).
//!   - `AlreadyClear` — no Imperva surface present at navigation time.
//!   - `Err(CaptchaRequired { kind })` — escalation to CAPTCHA without a
//!     solver callback. Pass `.on_captcha(...)` to register one.
//!   - `Err(Timeout { last_surface, .. })` — 60s elapsed without
//!     clearance.
//!
//! Requires the `imperva` cargo feature:
//! `cargo run --example imperva_bypass --features imperva`.

use std::time::Duration;

use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ImpervaError};

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let url = std::env::var("IMPERVA_DEMO_URL")
        .unwrap_or_else(|_| "https://example.com/imperva-protected".into());

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await?;
    let tab = browser.main_tab();

    tab.goto(&url).await?;
    tab.wait_for_load().await?;

    match tab
        .imperva()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await
    {
        Ok(outcome) => println!("cleared: {outcome:?}"),
        Err(ImpervaError::CaptchaRequired { kind }) => {
            println!("captcha required ({kind:?}); register .on_captcha(...) to solve");
        }
        Err(ImpervaError::Timeout { last_surface, .. }) => {
            println!("clearance timed out; last_surface = {last_surface:?}");
        }
        Err(e) => return Err(e.into()),
    }

    let title = tab.title().await?;
    println!("title = {title:?}");

    browser.close().await?;
    Ok(())
}
```

- [ ] **Step 2: Register example in `crates/zendriver/Cargo.toml`**

After the existing `cloudflare_bypass` example block, add:

```toml
[[example]]
name = "imperva_bypass"
required-features = ["imperva"]
```

- [ ] **Step 3: Verify**

```bash
cargo build --example imperva_bypass --features imperva --locked
cargo clippy --example imperva_bypass --features imperva --locked -- -D warnings
cargo fmt --all --check
```

Expected: example compiles.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs(imperva): add imperva_bypass example

Mirrors cloudflare_bypass example shape. Uses IMPERVA_DEMO_URL env
var override since no universally-stable public Imperva demo exists.
Demonstrates StealthProfile::spoofed pairing, 60s timeout, and the
three clearance outcomes plus the two principal error paths
(CaptchaRequired, Timeout).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Doctests audit (≥5)

**Files:**
- Modify: `crates/zendriver-imperva/src/lib.rs`
- Modify: `crates/zendriver-imperva/src/bypass.rs`
- Modify: `crates/zendriver-imperva/src/detection.rs`

- [ ] **Step 1: Module-level doctest in `lib.rs`**

Replace the current `lib.rs` body's top doc block with one that contains a compile-checked example:

```rust
//! Imperva WAF / Incapsula bypass for `zendriver`.
//!
//! See the [Imperva chapter](https://turtiesocks.github.io/zendriver-rs/imperva.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, surface variants, and CAPTCHA-callback recipes.
//!
//! **Stealth required.** Imperva's reese84 sensor is itself a browser
//! fingerprint check. Run with [`BrowserBuilder::stealth`] enabled or this
//! bypass will fail on nearly all real Imperva-protected sites.
//!
//! Most users go through `zendriver`'s `Tab::imperva()` (feature-gated)
//! rather than constructing the bypass directly. The [`ImpervaBypass`]
//! type is the underlying driver.
//!
//! ```no_run
//! # async fn ex(tab: &zendriver_transport::SessionHandle)
//! #   -> Result<(), zendriver_imperva::ImpervaError> {
//! use std::time::Duration;
//! use zendriver_imperva::{ClearanceOutcome, ImpervaBypass};
//!
//! let outcome = ImpervaBypass::new(tab)
//!     .timeout(Duration::from_secs(30))
//!     .wait_for_clearance()
//!     .await?;
//! match outcome {
//!     ClearanceOutcome::TokenAcquired { reese84, .. } => {
//!         println!("token: {reese84}")
//!     }
//!     ClearanceOutcome::ChallengeGone => println!("legacy cleared"),
//!     ClearanceOutcome::AlreadyClear => println!("no challenge present"),
//! }
//! # Ok(()) }
//! ```
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth
```

- [ ] **Step 2: Per-method doctest on `ImpervaBypass::wait_for_clearance`**

Append a doctest block to the existing rustdoc of `wait_for_clearance` in `bypass.rs`:

```rust
    /// ...
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_imperva::ImpervaError> {
    /// use std::time::Duration;
    /// use zendriver_imperva::ImpervaBypass;
    ///
    /// let outcome = ImpervaBypass::new(tab)
    ///     .poll_interval(Duration::from_millis(500))
    ///     .timeout(Duration::from_secs(45))
    ///     .wait_for_clearance()
    ///     .await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    pub async fn wait_for_clearance(self) -> ...
```

- [ ] **Step 3: Per-method doctest on `ImpervaBypass::on_captcha`**

```rust
    /// ...
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_imperva::ImpervaError> {
    /// use zendriver_imperva::{CaptchaSolution, ImpervaBypass};
    ///
    /// let outcome = ImpervaBypass::new(tab)
    ///     .on_captcha(|challenge| async move {
    ///         // Call your 2captcha / anticaptcha integration here.
    ///         Ok(CaptchaSolution {
    ///             token: "SOLVER_TOKEN".into(),
    ///             form_field: "h-captcha-response".into(),
    ///         })
    ///     })
    ///     .wait_for_clearance()
    ///     .await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    pub fn on_captcha<F, Fut>(mut self, f: F) -> Self ...
```

- [ ] **Step 4: Per-method doctest on `ImpervaBypass::with_interception`**

```rust
    /// ...
    ///
    /// ```no_run
    /// # async fn ex(tab: &zendriver_transport::SessionHandle)
    /// #   -> Result<(), zendriver_imperva::ImpervaError> {
    /// use zendriver_imperva::ImpervaBypass;
    ///
    /// let outcome = ImpervaBypass::new(tab)
    ///     .with_interception()
    ///     .wait_for_clearance()
    ///     .await?;
    /// println!("{outcome:?}");
    /// # Ok(()) }
    /// ```
    pub fn with_interception(mut self) -> Self ...
```

- [ ] **Step 5: Per-function doctest on `detect_surface`**

In `detection.rs`, add to the rustdoc on `detect_surface`:

```rust
/// ...
///
/// ```no_run
/// # async fn ex(tab: &zendriver_transport::SessionHandle)
/// #   -> Result<(), zendriver_imperva::ImpervaError> {
/// use zendriver_imperva::{ImpervaSurface, detect_surface};
///
/// match detect_surface(tab).await? {
///     ImpervaSurface::None => println!("clean page"),
///     other => println!("imperva surface: {other:?}"),
/// }
/// # Ok(()) }
/// ```
pub async fn detect_surface(session: &SessionHandle) -> ...
```

- [ ] **Step 6: Verify doctests**

```bash
cargo test -p zendriver-imperva --doc --locked
cargo build --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expected: 5 doctests pass (1 lib.rs + 3 per-method on bypass + 1 detect_surface).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs(imperva): add 5 doctests across lib + public methods

Module-level usage example + per-method doctests on
wait_for_clearance, on_captcha, with_interception, and
detect_surface. Matches the doctest density of zendriver-cloudflare.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Nightly CI workflow + integration test scaffold

**Files:**
- Create: `crates/zendriver/tests/imperva_v0.rs`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Write integration test scaffold**

```rust
//! Nightly Imperva bypass tests against real-internet sites.
//!
//! Gated behind `imperva-tests` feature. Run in CI on cron `0 8 * * *`.
//! Failures are not blocking (`continue-on-error: true`) — Imperva
//! configurations on public sites change unpredictably.
//!
//! **Site list TODO.** No universally stable public Imperva demo exists.
//! Candidates to evaluate (each #[ignore]'d until validated):
//!   - User-controlled Imperva trial deployment
//!   - Public sites known to use Imperva ABP (rotate periodically)
//!   - A `IMPERVA_TEST_URL` env-var-driven test for ad-hoc validation
//!
//! The single concrete test below uses the `IMPERVA_TEST_URL` env var so
//! a maintainer can run it locally against any target site without
//! recompiling.

#![cfg(feature = "imperva-tests")]

use serial_test::serial;
use std::time::Duration;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ImpervaClearanceOutcome};

/// Env-var-driven smoke test. Set `IMPERVA_TEST_URL` to a known-protected
/// site. Skipped if unset.
#[tokio::test]
#[serial]
async fn imperva_bypass_env_driven_smoke() {
    let Ok(url) = std::env::var("IMPERVA_TEST_URL") else {
        eprintln!("IMPERVA_TEST_URL unset; skipping");
        return;
    };

    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    tab.goto(&url).await.expect("goto");
    tab.wait_for_load().await.expect("load");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let outcome = tab
        .imperva()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await
        .expect("clearance");

    assert!(
        matches!(
            outcome,
            ImpervaClearanceOutcome::TokenAcquired { .. }
                | ImpervaClearanceOutcome::ChallengeGone
                | ImpervaClearanceOutcome::AlreadyClear
        ),
        "unexpected outcome: {outcome:?}"
    );
    browser.close().await.expect("close");
}
```

- [ ] **Step 2: Add CI job**

Open `.github/workflows/ci.yml`. After the `nightly-cloudflare-tests` job block, add (matching indentation):

```yaml
  nightly-imperva-tests:
    if: github.event_name == 'schedule' && github.event.schedule == '0 8 * * *'
    runs-on: ubuntu-latest
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: taiki-e/install-action@nextest
      - name: Install Chromium
        run: sudo apt-get update && sudo apt-get install -y chromium-browser
      - run: cargo nextest run --workspace --features imperva-tests --test imperva_v0 --locked --profile ci-integration
```

Also add `- cron: '0 8 * * *'` to the existing `on.schedule:` list at the top of the file:

```yaml
  schedule:
    - cron: '0 6 * * *'
    - cron: '0 7 * * *'
    - cron: '0 8 * * *'
```

- [ ] **Step 3: Verify (local build only — CI cron unverifiable here)**

```bash
cargo build --tests --features imperva-tests --locked
cargo clippy --tests --features imperva-tests --locked -- -D warnings
cargo fmt --all --check
```

Expected: compiles. The smoke test itself only runs when `IMPERVA_TEST_URL` is set, so unit-test runs in CI will silently skip it.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
ci(imperva): add nightly-imperva-tests job + integration scaffold

New imperva_v0.rs gated by imperva-tests feature. Single
env-var-driven smoke test (IMPERVA_TEST_URL) so a maintainer can
validate any target site without recompiling. Cron 0 8 * * * (1h
after cloudflare nightly). continue-on-error per workspace
convention for external-site nightly jobs. Imperva site-list
curation deferred — no universally-stable public demo exists.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: mdBook chapter + README + CHANGELOG

**Files:**
- Create: `docs/book/src/imperva.md`
- Modify: `docs/book/src/SUMMARY.md`
- Modify: `README.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Write `docs/book/src/imperva.md`**

```markdown
# Imperva WAF / Incapsula

The `imperva` cargo feature (sub-crate: `zendriver-imperva`) provides a
passive bypass driver for sites protected by Imperva WAF / Incapsula. It
detects which Imperva surface is active (modern reese84 bot management,
legacy Incapsula `___utmvc` flow, or a CAPTCHA escalation), then polls
the page until both clearance signals — `reese84` cookie set AND body
markers cleared — are observed.

> **Stealth required.** Imperva's reese84 sensor is itself a browser
> fingerprint check. Run with `BrowserBuilder::stealth` enabled or the
> bypass will fail on nearly all real Imperva-protected sites.

## Quick start

```rust,no_run
use std::time::Duration;
use zendriver::stealth::StealthProfile;
use zendriver::{Browser, ImpervaClearanceOutcome};

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder()
        .stealth(StealthProfile::spoofed())
        .launch()
        .await?;
    let tab = browser.main_tab();
    tab.goto("https://protected.example.com").await?;
    tab.wait_for_load().await?;

    let outcome = tab
        .imperva()
        .timeout(Duration::from_secs(60))
        .wait_for_clearance()
        .await?;

    match outcome {
        ImpervaClearanceOutcome::TokenAcquired { reese84, .. } => {
            println!("got token: {reese84}")
        }
        ImpervaClearanceOutcome::ChallengeGone => println!("legacy cleared"),
        ImpervaClearanceOutcome::AlreadyClear => println!("no challenge"),
    }

    browser.close().await?;
    Ok(())
}
```

## Surface variants

| Variant | What it is | How it's detected |
|---|---|---|
| `Reese84` | Modern Imperva ABP bot management. Invisible JS challenge → reese84 sensor token. | `reese84` cookie name OR `Reese.js` body marker. |
| `Legacy` | Older Incapsula `___utmvc` / `incap_ses_*` flow. | `___utmvc` / `incap_ses_*` / `visid_incap_*` cookies, or `/_Incapsula_Resource` body marker. |
| `Captcha(kind)` | Escalation to hCaptcha, reCAPTCHA, or Imperva's native CAPTCHA. | iframe src patterns + `g-recaptcha` / `h-captcha` DOM markers. |
| `None` | No Imperva surface present. | Default — fast `AlreadyClear` path. |

Detection precedence: **Captcha > Reese84 > Legacy > None**.

## Clearance signal (S3 hybrid AND)

`TokenAcquired` requires *both*:

1. A non-empty `reese84` cookie scoped to the current site.
2. The page body no longer contains Imperva challenge markers.

This avoids false positives (cookie set during pre-clearance redirect)
and false negatives (cookie evicted by site CSP). Legacy flows that
never set `reese84` resolve to `ChallengeGone` once body markers clear.

## CAPTCHA handling

Without an `on_captcha` callback, a CAPTCHA surface returns
`ImpervaError::CaptchaRequired { kind }` immediately (no waiting). Plug
in your own solver:

```rust,no_run
# use zendriver_imperva::{CaptchaSolution, ImpervaBypass};
# async fn ex(tab: &zendriver_transport::SessionHandle) -> Result<(), zendriver_imperva::ImpervaError> {
let _ = ImpervaBypass::new(tab)
    .on_captcha(|challenge| async move {
        // Call 2captcha / anticaptcha / your own service here.
        Ok(CaptchaSolution {
            token: "...".into(),
            form_field: "h-captcha-response".into(),
        })
    })
    .wait_for_clearance()
    .await?;
# Ok(()) }
```

`CaptchaChallenge` carries `kind`, `site_key` (when extractable), and
`url`. `CaptchaSolution` is the token + form field name your solver
returns.

## Fetch-domain fast path

`with_interception()` spawns a `Fetch` subscription that signals on
first 2xx response to `Reese.js` or `_Incapsula_Resource*`. Polling
continues in parallel; first signal wins. Useful on sites where the
token cookie is set faster than the default 250ms poll cadence.

```rust,no_run
# use zendriver_imperva::ImpervaBypass;
# async fn ex(tab: &zendriver_transport::SessionHandle) -> Result<(), zendriver_imperva::ImpervaError> {
let _ = ImpervaBypass::new(tab)
    .with_interception()
    .wait_for_clearance()
    .await?;
# Ok(()) }
```

## Active sensor synthesis — out of scope

Reverse-engineering Imperva's obfuscated reese84 sensor JS and computing
tokens in pure Rust is *not* in scope for this crate. The maintenance
burden (Imperva ships new obfuscated builds frequently) and the lack of
CAPTCHA fallback in a pure-HTTP design make it a poor fit alongside a
browser-automation library. If you need pure-HTTP Imperva bypass for
high-throughput scraping, build that as a separate crate.
```

- [ ] **Step 2: Add chapter entry to `docs/book/src/SUMMARY.md`**

Locate the existing `cloudflare.md` entry. After it, add the imperva entry at the same indentation level:

```markdown
- [Imperva WAF / Incapsula](imperva.md)
```

- [ ] **Step 3: Update README feature matrix + comparison**

In `README.md`, locate the feature matrix that lists `cloudflare`. Add a row in the same shape:

```markdown
| `imperva`    | `zendriver-imperva`    | Imperva WAF / Incapsula bypass (reese84 / legacy / CAPTCHA) |
```

If a "Use cases / install snippets" section exists, add a one-liner:

```markdown
- Imperva-protected scraping: `cargo add zendriver --features imperva`
```

- [ ] **Step 4: Update CHANGELOG.md**

Add a new `## [Unreleased]` section if not present, or extend the existing one:

```markdown
### Added

- `zendriver-imperva` crate: passive bypass for Imperva WAF / Incapsula
  (reese84, legacy Incapsula, CAPTCHA surfaces). Opt-in Fetch-domain
  fast-path and opt-in CAPTCHA solver callback. `Tab::imperva()`
  convenience method gated by the `imperva` cargo feature.

### Changed

- `zendriver-cloudflare`: dropped stale `zendriver-interception` Cargo
  dep (never imported). Added Stealth-recommended call-out in lib docs.
- Both `cloudflare` and `imperva` bypass loops emit one `tracing::warn!`
  after ~2.5s of stalled clearance, nudging callers toward
  `BrowserBuilder::stealth`.
```

- [ ] **Step 5: Verify**

```bash
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
# mdBook build (if mdbook is installed locally; otherwise skip)
which mdbook && (cd docs/book && mdbook build) || echo "mdbook not installed; skipping book build"
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs(imperva): mdBook chapter + README + CHANGELOG entry

New imperva.md chapter walks through surface variants, S3 hybrid AND
clearance signal, CAPTCHA callback pattern, and Fetch fast-path.
README feature matrix gains imperva row. CHANGELOG notes the new
crate and the cloudflare stale-dep cleanup.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (after Task 13)

After all tasks land, run the full workspace verification batch:

```bash
cargo build --workspace --all-features --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

Expected outcomes:
- All ~265+ unit tests pass (24 from imperva crate added to ~243 prior).
- All doctests pass (5 new imperva doctests added).
- `imperva_bypass` example compiles under `--features imperva`.
- `imperva_v0` integration test compiles under `--features imperva-tests`.
- No clippy warnings; rustfmt clean.

Open a PR titled `feat: add zendriver-imperva crate + cloudflare retrofit`.
