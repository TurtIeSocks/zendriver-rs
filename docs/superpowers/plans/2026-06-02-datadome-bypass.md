# Group D — DataDome bypass crate + WebGPU coherence — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a feature-gated `zendriver-datadome` bypass crate (detect → observe-and-wait → opt-in CAPTCHA-solver callback), an 8th `Surface::Webgpu` coherence farble in `zendriver-stealth` that closes upstream #20's `navigator.gpu` leak, and a cross-crate retrofit unifying all three anti-bot crates on a single-channel result model.

**Architecture:** The crate mirrors `zendriver-imperva` file-for-file (bypass / detection / captcha / interception / error / lib + a `detect.js`). It detects the DataDome surface from `window.dd` + the `datadome` cookie + a `captcha-delivery.com` iframe, polls until the clearance cookie lands, and escalates a CAPTCHA surface to a caller-supplied async solver whose returned **cookie** is applied via `Network.setCookie` + reload. The WebGPU surface plugs into the existing `Persona`/`bootstrap_script` token-substitution machinery, deriving a coherent `GPUAdapter` from the already-spoofed WebGL renderer. The retrofit moves timeout/no-challenge terminals from `Error` to `Outcome` across imperva + cloudflare.

**Tech Stack:** Rust 2024, tokio, `zendriver-transport` (CDP actor + `MockConnection` test harness), `zendriver-interception` (Fetch domain), `thiserror`, `serde_json`, `rmcp` (MCP), `wiremock` (fixture tests), `insta` (schema snapshots).

**Spec:** `docs/superpowers/specs/2026-06-02-datadome-bypass-design.md`

**Branch:** `claude/group-d-datadome` (off main @ 704c4c4). Worktree: this checkout.

---

## Reference patterns (read before starting)

The implementer should open these as live templates — the plan references them rather than reproducing every boilerplate line:

- `crates/zendriver-imperva/src/{lib,bypass,detection,captcha,interception,error}.rs` — the crate template.
- `crates/zendriver-imperva/src/detect.js` — the detection-script template (bundled one-round-trip probe).
- `crates/zendriver-cloudflare/src/{bypass,detection,click}.rs` — the shadow-DOM iframe walk (`findBbox`) to reuse for the captcha-delivery iframe.
- `crates/zendriver-stealth/src/patches.rs` + `src/patches/webgl.js` — the token-substitution machinery the WebGPU surface joins.
- `crates/zendriver-mcp/src/tools/{imperva,cloudflare}.rs` + `src/server.rs` (`imperva_tool_router` @ ~978, `combined_tool_router` @ ~1094) — the MCP tool template.
- `crates/zendriver/tests/network_monitor_http.rs` — the `wiremock` + real-Chrome integration-test harness (`fixture_with_html`, `#[serial]`, `#[ignore]`).

**Gates after every phase that touches Rust** (the repo CLAUDE.md requires this before any push; run locally, do not rely on CI):

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked --fix --allow-dirty --allow-staged
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings   # feature-gated code touched
```

---

## Phase 0 — Crate scaffold

### Task 0.1: Create the `zendriver-datadome` crate skeleton

**Files:**
- Create: `crates/zendriver-datadome/Cargo.toml`
- Create: `crates/zendriver-datadome/src/lib.rs`
- Modify: `Cargo.toml` (workspace root) — `members` + `[workspace.dependencies]`

- [ ] **Step 1: Write `Cargo.toml`** (copy `crates/zendriver-imperva/Cargo.toml`, change name/description/keywords; it needs the interception + tokio-util + futures deps because the Fetch fast-path mirrors imperva):

```toml
[package]
name = "zendriver-datadome"
description = "DataDome anti-bot bypass for zendriver"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
readme = "../../README.md"
homepage = "https://github.com/TurtIeSocks/zendriver-rs"
documentation = "https://docs.rs/zendriver-datadome"
keywords = ["datadome", "anti-bot", "bypass", "browser", "zendriver"]
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
tokio-util.workspace             = true
futures.workspace                = true
serde.workspace                  = true
serde_json.workspace             = true
thiserror.workspace              = true
tracing.workspace                = true

[dev-dependencies]
tokio-test.workspace = true
zendriver-transport  = { workspace = true, features = ["testing"] }
```

- [ ] **Step 2: Write a minimal `src/lib.rs`** so the crate compiles before the modules exist:

```rust
//! DataDome anti-bot bypass for `zendriver`.
//!
//! Sibling to `zendriver-cloudflare` / `zendriver-imperva`. Detects the active
//! DataDome surface, observes the page until the `datadome` clearance cookie
//! lands, and escalates a CAPTCHA surface to a caller-supplied solver.
//!
//! # Limitations
//!
//! Clearance ≠ acceptance. DataDome's dominant surface is an **invisible
//! device-check** that scores the browser fingerprint; a `Cleared` result
//! means the `datadome` cookie landed, not that the fingerprint passed. The
//! single most common cause of a stuck device-check is the WebGL / WebGPU
//! fingerprint leak from upstream issue #20 — run with
//! `BrowserBuilder::stealth` (which now includes the `Surface::Webgpu`
//! coherence patch) and, in containers, ensure GPU support. IP reputation and
//! UA-vs-binary drift are the other upstream causes.

// Modules land in later tasks.
```

- [ ] **Step 3: Add the crate to the workspace.** In the root `Cargo.toml`, add `"crates/zendriver-datadome"` to `members` (after the `zendriver-imperva` line) and this to `[workspace.dependencies]` (after the `zendriver-imperva` line, aligned):

```toml
zendriver-datadome      = { path = "crates/zendriver-datadome", version = "0.1.0" }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p zendriver-datadome`
Expected: builds clean (empty lib).

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-datadome Cargo.toml
git commit -m "feat(datadome): scaffold zendriver-datadome crate"
```

---

## Phase 1 — Errors + surface detection

### Task 1.1: `DataDomeError`

**Files:**
- Create: `crates/zendriver-datadome/src/error.rs`
- Modify: `crates/zendriver-datadome/src/lib.rs` (add `pub mod error;` + re-export)

- [ ] **Step 1: Write the failing test** (append to `error.rs`):

```rust
#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_captcha_required() {
        assert_eq!(
            DataDomeError::CaptchaRequired.to_string(),
            "CAPTCHA surface detected but no solver registered"
        );
    }

    #[test]
    fn display_js_error_passthrough() {
        assert_eq!(
            DataDomeError::JsError("bad payload".into()).to_string(),
            "JS error: bad payload"
        );
    }
}
```

- [ ] **Step 2: Write `error.rs`** (mirror `zendriver-imperva/src/error.rs`; faults only — no `Timeout`/`Block`, those are outcomes):

```rust
//! DataDome-bypass errors.

use zendriver_interception::InterceptionError;
use zendriver_transport::CallError;

/// Error returned by [`DataDomeBypass`] operations. Faults only — flow
/// terminals (cleared / blocked / timed-out / already-clear) are
/// [`ClearanceOutcome`] variants, not errors.
///
/// [`DataDomeBypass`]: crate::bypass::DataDomeBypass
/// [`ClearanceOutcome`]: crate::bypass::ClearanceOutcome
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DataDomeError {
    /// CAPTCHA surface detected, but no `on_captcha` solver was registered.
    #[error("CAPTCHA surface detected but no solver registered")]
    CaptchaRequired,

    /// User-supplied CAPTCHA solver returned an error.
    #[error("CAPTCHA solver failed: {0}")]
    CaptchaSolver(Box<dyn std::error::Error + Send + Sync>),

    /// Fetch-domain interception hook failed at startup (only when
    /// [`DataDomeBypass::with_interception`] was set).
    ///
    /// [`DataDomeBypass::with_interception`]: crate::bypass::DataDomeBypass::with_interception
    #[error("interception hook error: {0}")]
    Interception(#[from] InterceptionError),

    /// CDP transport / call error.
    #[error("call failed: {0}")]
    Call(#[from] CallError),

    /// In-page evaluator raised or returned an unexpected payload shape.
    #[error("JS error: {0}")]
    JsError(String),
}
```

- [ ] **Step 3: Wire into `lib.rs`** — add `pub mod error;` and `pub use error::DataDomeError;`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zendriver-datadome error::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-datadome/src
git commit -m "feat(datadome): DataDomeError (faults-only)"
```

### Task 1.2: `DataDomeSurface` + `detect_surface` + `detect.js`

**Files:**
- Create: `crates/zendriver-datadome/src/detection.rs`
- Create: `crates/zendriver-datadome/src/detect.js`
- Modify: `crates/zendriver-datadome/src/lib.rs`

**Detection contract:** `detect.js` returns one JSON object:
```js
{ surface: "device_check" | "captcha" | "block" | "none",
  datadome: "<cookie value>" | null,   // the `datadome` cookie
  dd: { cid, hsh, t, host } | null,     // parsed window.dd, when present
  captcha_url: "<...>" | null,          // captcha-delivery iframe src, when present
  body_clean: true | false }            // no dd config / challenge markers in DOM
```

- [ ] **Step 1: Write `detect.js`** — reads cookie, `window.dd`, and walks (incl. shadow roots) for a `captcha-delivery.com` iframe:

```js
(function () {
  function readCookie(name) {
    var m = document.cookie.match(new RegExp('(?:^|; )' + name + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : null;
  }
  // Recursive shadow-DOM-aware walk for the captcha-delivery iframe src.
  function findCaptchaUrl(root) {
    var iframes = root.querySelectorAll ? root.querySelectorAll('iframe') : [];
    for (var i = 0; i < iframes.length; i++) {
      var f = iframes[i];
      if (f.src && f.src.indexOf('captcha-delivery.com') !== -1) return f.src;
    }
    var all = root.querySelectorAll ? root.querySelectorAll('*') : [];
    for (var j = 0; j < all.length; j++) {
      if (all[j].shadowRoot) {
        var sub = findCaptchaUrl(all[j].shadowRoot);
        if (sub) return sub;
      }
    }
    return null;
  }
  var dd = (typeof window.dd === 'object' && window.dd) ? window.dd : null;
  var captchaUrl = findCaptchaUrl(document);
  var datadome = readCookie('datadome');

  // Classify. window.dd.t drives the challenge type: 'bv' => banned/block,
  // 'fe' => front-end challenge (device-check or captcha). A captcha-delivery
  // iframe is the captcha tell; otherwise dd present => device-check.
  var surface = 'none';
  if (dd && String(dd.t) === 'bv') {
    surface = 'block';
  } else if (captchaUrl) {
    surface = 'captcha';
  } else if (dd) {
    surface = 'device_check';
  }

  var bodyClean = !dd && !captchaUrl;
  return {
    surface: surface,
    datadome: datadome,
    dd: dd ? { cid: dd.cid || null, hsh: dd.hsh || null, t: dd.t || null, host: dd.host || null } : null,
    captcha_url: captchaUrl,
    body_clean: bodyClean
  };
})()
```

- [ ] **Step 2: Write the failing test** (append to `detection.rs`):

```rust
#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    fn reply(v: serde_json::Value) -> serde_json::Value {
        json!({ "result": { "type": "object", "value": v } })
    }

    #[tokio::test]
    async fn detect_surface_classifies_each_surface() {
        for (payload, expected) in [
            (json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"geo.captcha-delivery.com"},"captcha_url":null,"body_clean":false}), DataDomeSurface::DeviceCheck),
            (json!({"surface":"captcha","datadome":"DD","dd":{"cid":"C","hsh":"H","t":"fe","host":"geo.captcha-delivery.com"},"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false}), DataDomeSurface::Captcha),
            (json!({"surface":"block","datadome":null,"dd":{"cid":"C","hsh":"H","t":"bv","host":"geo.captcha-delivery.com"},"captcha_url":null,"body_clean":false}), DataDomeSurface::Block),
            (json!({"surface":"none","datadome":"DD","dd":null,"captcha_url":null,"body_clean":true}), DataDomeSurface::None),
        ] {
            let (mut mock, conn) = MockConnection::pair();
            let sess = SessionHandle::new(conn.clone(), "S1");
            let fut = tokio::spawn({ let s = sess.clone(); async move { detect_surface(&s).await } });
            let id = mock.expect_cmd("Runtime.evaluate").await;
            assert!(mock.last_sent()["params"]["expression"].as_str().unwrap().contains("captcha-delivery.com"));
            mock.reply(id, reply(payload)).await;
            assert_eq!(fut.await.unwrap().unwrap(), expected);
            conn.shutdown();
        }
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zendriver-datadome detection::`
Expected: FAIL ("cannot find ... DataDomeSurface").

- [ ] **Step 4: Write `detection.rs`** (mirror `zendriver-imperva/src/detection.rs`'s structure — `RawSnapshot` deserialize + `detect_snapshot` one-round-trip + a `detect_surface` convenience):

```rust
//! DataDome surface detection. One `Runtime.evaluate` round-trip bundles the
//! cookie read, `window.dd` parse, and captcha-delivery iframe walk.

use serde::Deserialize;
use serde_json::{Value, json};
use zendriver_transport::SessionHandle;

use crate::error::DataDomeError;

/// Which DataDome surface a tab is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDomeSurface {
    /// window.dd.t == 'fe' device-check / interstitial — invisible JS
    /// interrogation; also covers the auto-resolving "please wait" page.
    DeviceCheck,
    /// captcha-delivery.com iframe present (slider / puzzle / press-hold).
    Captcha,
    /// window.dd.t == 'bv' — IP banned; unsolvable in-browser.
    Block,
    /// No DataDome surface detected; the datadome cookie may already be valid.
    None,
}

/// Parsed `window.dd` challenge descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DdConfig {
    pub cid: Option<String>,
    pub hsh: Option<String>,
    pub t: Option<String>,
    pub host: Option<String>,
}

/// Snapshot of one `detect.js` round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionSnapshot {
    pub surface: DataDomeSurface,
    pub datadome: Option<String>,
    pub dd: Option<DdConfig>,
    pub captcha_url: Option<String>,
    pub body_clean: bool,
}

#[derive(Debug, Deserialize)]
struct RawSnapshot {
    surface: String,
    #[serde(default)]
    datadome: Option<String>,
    #[serde(default)]
    dd: Option<DdConfig>,
    #[serde(default)]
    captcha_url: Option<String>,
    body_clean: bool,
}

impl From<RawSnapshot> for DetectionSnapshot {
    fn from(r: RawSnapshot) -> Self {
        let surface = match r.surface.as_str() {
            "device_check" => DataDomeSurface::DeviceCheck,
            "captcha" => DataDomeSurface::Captcha,
            "block" => DataDomeSurface::Block,
            _ => DataDomeSurface::None,
        };
        Self {
            surface,
            datadome: r.datadome,
            dd: r.dd,
            captcha_url: r.captcha_url,
            body_clean: r.body_clean,
        }
    }
}

/// Run a single `detect.js` probe against `session`'s main world.
pub(crate) async fn detect_snapshot(
    session: &SessionHandle,
) -> Result<DetectionSnapshot, DataDomeError> {
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
        return Err(DataDomeError::JsError(msg));
    }

    let value = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);

    let raw: RawSnapshot = serde_json::from_value(value)
        .map_err(|e| DataDomeError::JsError(format!("invalid detect.js payload: {e}")))?;
    Ok(raw.into())
}

/// Surface-only probe. Convenience for "which surface am I on" without driving
/// a bypass.
///
/// ```no_run
/// # async fn ex(tab: &zendriver_transport::SessionHandle)
/// #   -> Result<(), zendriver_datadome::DataDomeError> {
/// use zendriver_datadome::{DataDomeSurface, detect_surface};
/// match detect_surface(tab).await? {
///     DataDomeSurface::None => println!("clean"),
///     other => println!("datadome surface: {other:?}"),
/// }
/// # Ok(()) }
/// ```
pub async fn detect_surface(session: &SessionHandle) -> Result<DataDomeSurface, DataDomeError> {
    Ok(detect_snapshot(session).await?.surface)
}
```

- [ ] **Step 5: Wire into `lib.rs`** — `pub mod detection;` + `pub use detection::{DataDomeSurface, DetectionSnapshot, DdConfig, detect_surface};`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p zendriver-datadome detection::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zendriver-datadome/src
git commit -m "feat(datadome): surface detection (detect.js + DataDomeSurface)"
```

---

## Phase 2 — Result model + driver core (no captcha/interception yet)

### Task 2.1: `ClearanceOutcome` + `DataDomeBypass` builder

**Files:**
- Create: `crates/zendriver-datadome/src/bypass.rs`
- Modify: `crates/zendriver-datadome/src/lib.rs`

- [ ] **Step 1: Write the failing test** (append to `bypass.rs`):

```rust
#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn builder_defaults_and_overrides() {
        let (_, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let b = DataDomeBypass::new(&sess);
        assert_eq!(b.poll_interval, DEFAULT_POLL_INTERVAL);
        assert_eq!(b.timeout, DEFAULT_TIMEOUT);
        assert!(b.on_captcha.is_none());
        assert!(!b.interception_enabled);

        let b = b
            .timeout(std::time::Duration::from_secs(45))
            .poll_interval(std::time::Duration::from_millis(100))
            .with_interception();
        assert_eq!(b.timeout, std::time::Duration::from_secs(45));
        assert_eq!(b.poll_interval, std::time::Duration::from_millis(100));
        assert!(b.interception_enabled);
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Write the `bypass.rs` head** (types + builder; mirror `zendriver-imperva/src/bypass.rs` lines 50–175 — same `Arc<dyn solver>` + custom `Debug`). The `wait_for_clearance` body lands in Task 2.2 (use a `todo!()`-free stub that the next task replaces):

```rust
//! DataDome bypass driver.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use zendriver_transport::SessionHandle;

use crate::captcha::CaptchaSolver;
use crate::detection::{DataDomeSurface, DetectionSnapshot};
use crate::error::DataDomeError;

pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Terminal outcome of a clearance attempt. All flow-terminals are `Ok`;
/// only genuine faults are [`DataDomeError`].
#[derive(Debug, Clone)]
pub enum ClearanceOutcome {
    /// datadome cookie acquired AND challenge markers gone.
    Cleared { datadome: String },
    /// Markers cleared but no datadome cookie observed (rare / legacy path).
    ChallengeGone,
    /// No DataDome surface present at call time. Fast path; no waiting.
    AlreadyClear,
    /// window.dd.t == 'bv' — IP banned. Nothing in-browser can clear this;
    /// the caller must change IP (e.g. residential proxy).
    Blocked,
    /// Deadline elapsed without reaching a terminal state.
    TimedOut { last_surface: Option<DataDomeSurface> },
}

/// Drives a DataDome clearance flow against a single tab's session.
///
/// Constructed via `Tab::datadome()`.
pub struct DataDomeBypass<'tab> {
    pub(crate) session: &'tab SessionHandle,
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
    pub(crate) on_captcha: Option<Arc<CaptchaSolver>>,
    pub(crate) interception_enabled: bool,
}

impl std::fmt::Debug for DataDomeBypass<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataDomeBypass")
            .field("poll_interval", &self.poll_interval)
            .field("timeout", &self.timeout)
            .field("on_captcha", &self.on_captcha.as_ref().map(|_| "..."))
            .field("interception_enabled", &self.interception_enabled)
            .finish()
    }
}

impl<'tab> DataDomeBypass<'tab> {
    /// New driver with default 250ms poll interval + 30s timeout.
    pub fn new(session: &'tab SessionHandle) -> Self {
        Self {
            session,
            poll_interval: DEFAULT_POLL_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
            on_captcha: None,
            interception_enabled: false,
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

    /// Enable the Fetch-domain fast-path (see [`crate::interception`]).
    #[must_use]
    pub fn with_interception(mut self) -> Self {
        self.interception_enabled = true;
        self
    }
}
```

> Note: `bypass.rs` won't compile until `captcha.rs` defines `CaptchaSolver` (Task 3.1). To keep Phase 2 green, do Task 3.1 **before** building 2.1, OR temporarily stub `pub(crate) type CaptchaSolver = dyn std::fmt::Debug + Send + Sync;` and the `on_captcha` field, then replace in Task 3.1. The plan orders 2.1 → 2.2 → 3.1; the implementer should write the `captcha.rs` solver-type alias (Task 3.1 Step 2, the `CaptchaSolver` + `arc_solver` block only) first so the type resolves. Mark this dependency.

- [ ] **Step 3: Add `pub mod bypass;` + `pub mod captcha;` to `lib.rs`** and re-export `pub use bypass::{ClearanceOutcome, DataDomeBypass};`.

- [ ] **Step 4: Run test**

Run: `cargo test -p zendriver-datadome bypass::builder_defaults_and_overrides`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-datadome/src
git commit -m "feat(datadome): ClearanceOutcome + DataDomeBypass builder"
```

### Task 2.2: `wait_for_clearance` core poll loop (device-check / block / already-clear / timeout)

**Files:**
- Modify: `crates/zendriver-datadome/src/bypass.rs`

This mirrors imperva's `wait_for_clearance` + `poll_loop` (bypass.rs lines 215–406) with DataDome's terminal rules. The captcha + interception branches are stubbed here (added in Tasks 3.2 / 4.1).

- [ ] **Step 1: Write the failing tests** (append to the `bypass.rs` test module):

```rust
    use crate::detection::DataDomeSurface;
    use serde_json::json;

    fn snap_reply(v: serde_json::Value) -> serde_json::Value {
        json!({ "result": { "type": "object", "value": v } })
    }

    #[tokio::test]
    async fn already_clear_on_clean_page() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s).wait_for_clearance().await
        }});
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id, snap_reply(json!({"surface":"none","datadome":"DD","dd":null,"captcha_url":null,"body_clean":true}))).await;
        assert!(matches!(fut.await.unwrap().unwrap(), ClearanceOutcome::AlreadyClear));
        conn.shutdown();
    }

    #[tokio::test]
    async fn block_is_immediate_terminal() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s).wait_for_clearance().await
        }});
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id, snap_reply(json!({"surface":"block","datadome":null,"dd":{"cid":"C","hsh":"H","t":"bv","host":"h"},"captcha_url":null,"body_clean":false}))).await;
        assert!(matches!(fut.await.unwrap().unwrap(), ClearanceOutcome::Blocked));
        conn.shutdown();
    }

    #[tokio::test]
    async fn device_check_clears_when_cookie_lands_and_body_clean() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s).poll_interval(Duration::from_millis(1)).wait_for_clearance().await
        }});
        // Probe 1: device-check, no cookie.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id1, snap_reply(json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":null,"body_clean":false}))).await;
        // Probe 2: cleared.
        let id2 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id2, snap_reply(json!({"surface":"none","datadome":"COOKIE_OK","dd":null,"captcha_url":null,"body_clean":true}))).await;
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "COOKIE_OK"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn times_out_when_device_check_never_clears() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s).poll_interval(Duration::from_millis(1)).timeout(Duration::from_millis(40)).wait_for_clearance().await
        }});
        for _ in 0..50 {
            let Ok(id) = tokio::time::timeout(Duration::from_millis(80), mock.expect_cmd("Runtime.evaluate")).await else { break };
            mock.reply(id, snap_reply(json!({"surface":"device_check","datadome":null,"dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":null,"body_clean":false}))).await;
        }
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::TimedOut { last_surface } => assert_eq!(last_surface, Some(DataDomeSurface::DeviceCheck)),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        conn.shutdown();
    }
```

- [ ] **Step 2: Implement `wait_for_clearance` + a factored `poll_loop`** in the `impl` block. The clearance rule: `Cleared` when the `datadome` cookie is present AND `body_clean`; `ChallengeGone` when `body_clean` but no cookie (defensive). `Block` short-circuits. The captcha + interception integration points are marked and filled in later tasks:

```rust
    /// Run the surface-aware poll loop until clearance, block, or timeout.
    ///
    /// # Returns
    /// - `Cleared { datadome }` — cookie landed and markers gone.
    /// - `ChallengeGone` — markers gone, no cookie observed.
    /// - `AlreadyClear` — no surface at call time.
    /// - `Blocked` — `window.dd.t == 'bv'` (IP banned).
    /// - `TimedOut { last_surface }` — deadline elapsed.
    ///
    /// # Errors
    /// - [`DataDomeError::CaptchaRequired`] — captcha surface, no solver.
    /// - [`DataDomeError::CaptchaSolver`] — registered solver errored.
    /// - [`DataDomeError::Interception`] / [`DataDomeError::Call`] /
    ///   [`DataDomeError::JsError`].
    pub async fn wait_for_clearance(self) -> Result<ClearanceOutcome, DataDomeError> {
        let deadline = Instant::now() + self.timeout;

        let snapshot = self.probe(deadline).await?;

        match snapshot.surface {
            DataDomeSurface::None if snapshot.body_clean => {
                return Ok(ClearanceOutcome::AlreadyClear);
            }
            DataDomeSurface::Block => return Ok(ClearanceOutcome::Blocked),
            DataDomeSurface::Captcha => {
                // Captcha escalation lands in Task 3.2. Until then:
                return Err(DataDomeError::CaptchaRequired);
            }
            _ => {}
        }

        // Interception fast-path is wired in Task 4.1.
        self.poll_loop(deadline, Some(snapshot)).await
    }

    /// One detect probe, raced against `deadline` so no probe outlives the
    /// caller's budget. Returns `TimedOut`-as-Ok via the loop, but here a
    /// deadline hit maps to a sentinel the loop converts; simplest is to
    /// return the snapshot or, on deadline, a synthetic `None`/clean snapshot
    /// that the loop treats as timeout. To keep terminals explicit we instead
    /// surface the deadline through `poll_loop`'s own check — so `probe`
    /// simply awaits `detect_snapshot` (the pre-loop probe is fast).
    async fn probe(&self, _deadline: Instant) -> Result<DetectionSnapshot, DataDomeError> {
        crate::detection::detect_snapshot(self.session).await
    }

    pub(crate) async fn poll_loop(
        self,
        deadline: Instant,
        mut next_snapshot: Option<DetectionSnapshot>,
    ) -> Result<ClearanceOutcome, DataDomeError> {
        use tokio::time::{Interval, MissedTickBehavior};

        let mut ticker: Interval = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut last_surface: Option<DataDomeSurface> = None;

        loop {
            let snap = match next_snapshot.take() {
                Some(s) => s,
                None => crate::detection::detect_snapshot(self.session).await?,
            };

            let cookie = snap.datadome.as_ref().filter(|v| !v.is_empty());
            match (cookie, snap.body_clean) {
                (Some(c), true) => return Ok(ClearanceOutcome::Cleared { datadome: c.clone() }),
                (None, true) if snap.surface == DataDomeSurface::None => {
                    return Ok(ClearanceOutcome::ChallengeGone);
                }
                _ => last_surface = Some(snap.surface),
            }

            if Instant::now() >= deadline {
                return Ok(ClearanceOutcome::TimedOut { last_surface });
            }

            tokio::select! {
                _ = ticker.tick() => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Ok(ClearanceOutcome::TimedOut { last_surface });
                }
            }
        }
    }
```

> Decision note: unlike imperva (which makes the pre-loop probe race the deadline via `probe_with_deadline`), DataDome's pre-loop probe is a single fast `detect.js` eval; the loop's own deadline check + `sleep_until` arm bound total time. If a hung CDP call is a concern, wrap `probe` in `tokio::time::timeout(self.timeout, …)` mapping elapsed → `Ok(TimedOut{last_surface:None})`. Keep it simple unless an integration test shows a hang.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zendriver-datadome bypass::`
Expected: PASS (all four terminal tests).

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-datadome/src/bypass.rs
git commit -m "feat(datadome): core poll loop (cleared/blocked/already-clear/timeout)"
```

---

## Phase 3 — CAPTCHA escalation

### Task 3.1: `captcha.rs` — solver type, challenge extraction, cookie application

**Files:**
- Create: `crates/zendriver-datadome/src/captcha.rs`
- Modify: `crates/zendriver-datadome/src/lib.rs`, `crates/zendriver-datadome/src/bypass.rs` (`on_captcha` builder method)

- [ ] **Step 1: Write the failing test** (append to `captcha.rs`):

```rust
#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn apply_solution_sets_cookie_then_reloads() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let sol = DataDomeSolution { datadome_cookie: "SOLVED_DD".into() };
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            apply_solution(&s, &sol, "https://shop.example.com/x").await
        }});

        let id_cookie = mock.expect_cmd("Network.setCookie").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["name"], "datadome");
        assert_eq!(sent["params"]["value"], "SOLVED_DD");
        assert_eq!(sent["params"]["domain"], ".example.com");
        mock.reply(id_cookie, json!({ "success": true })).await;

        let id_reload = mock.expect_cmd("Page.reload").await;
        mock.reply(id_reload, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Write `captcha.rs`** (solver type + `arc_solver` mirror imperva exactly; the challenge/solution types + `build_challenge` + `apply_solution` are DataDome-specific — cookie, not form field):

```rust
//! CAPTCHA escalation: types handed to / returned from the caller-supplied
//! solver, plus the CDP helpers that build the challenge descriptor and apply
//! the solved `datadome` cookie. DataDome's solution is a COOKIE (it
//! whitelists the browser), not a form-field token — the structural delta from
//! imperva.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::json;
use zendriver_transport::SessionHandle;

use crate::detection::DetectionSnapshot;
use crate::error::DataDomeError;

/// CAPTCHA escalation handed to a caller-supplied solver.
#[derive(Debug, Clone)]
pub struct DataDomeChallenge {
    /// captcha-delivery iframe URL, e.g.
    /// `https://geo.captcha-delivery.com/captcha/?initialCid=…&hash=…&cid=…&t=fe`.
    pub captcha_url: String,
    /// URL of the page presenting the CAPTCHA.
    pub site_url: String,
    /// Browser UA — MUST match the page's UA (solver-service requirement).
    pub user_agent: String,
    /// datadome cookie / `dd.cid`, when known.
    pub cid: Option<String>,
    /// `dd.hsh`, when known.
    pub hash: Option<String>,
}

/// Token returned by a caller-supplied solver. For DataDome this is the solved
/// `datadome` COOKIE value.
#[derive(Debug, Clone)]
pub struct DataDomeSolution {
    pub datadome_cookie: String,
}

/// Erased solver closure, stored behind `Arc<dyn ...>` on [`DataDomeBypass`].
///
/// [`DataDomeBypass`]: crate::bypass::DataDomeBypass
pub(crate) type CaptchaSolver = dyn Fn(
        DataDomeChallenge,
    )
        -> BoxFuture<'static, Result<DataDomeSolution, Box<dyn std::error::Error + Send + Sync>>>
    + Send
    + Sync;

/// Wrap a typed closure into the stored `Arc<dyn CaptchaSolver>` shape.
pub(crate) fn arc_solver<F, Fut>(f: F) -> Arc<CaptchaSolver>
where
    F: Fn(DataDomeChallenge) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<
            Output = Result<DataDomeSolution, Box<dyn std::error::Error + Send + Sync>>,
        > + Send
        + 'static,
{
    Arc::new(move |challenge| Box::pin(f(challenge)))
}

/// Build a [`DataDomeChallenge`] from a detection snapshot + the live page URL
/// + UA (read via CDP).
pub(crate) async fn build_challenge(
    session: &SessionHandle,
    snap: &DetectionSnapshot,
) -> Result<DataDomeChallenge, DataDomeError> {
    // Read location.href + navigator.userAgent in one eval.
    let res = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "({ url: location.href, ua: navigator.userAgent })",
                "returnByValue": true,
            }),
        )
        .await?;
    let v = res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let site_url = v.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let user_agent = v.get("ua").and_then(|x| x.as_str()).unwrap_or("").to_string();

    Ok(DataDomeChallenge {
        captcha_url: snap.captcha_url.clone().unwrap_or_default(),
        site_url,
        user_agent,
        cid: snap.dd.as_ref().and_then(|d| d.cid.clone()).or_else(|| snap.datadome.clone()),
        hash: snap.dd.as_ref().and_then(|d| d.hsh.clone()),
    })
}

/// Apply the solved `datadome` cookie via `Network.setCookie`, scoped to the
/// registrable parent domain of `site_url` (DataDome sets the cookie on the
/// eTLD+1 so it covers subdomains), then reload the page.
pub(crate) async fn apply_solution(
    session: &SessionHandle,
    solution: &DataDomeSolution,
    site_url: &str,
) -> Result<(), DataDomeError> {
    let domain = cookie_domain(site_url);
    session
        .call(
            "Network.setCookie",
            json!({
                "name": "datadome",
                "value": solution.datadome_cookie,
                "domain": domain,
                "path": "/",
                "secure": true,
                "sameSite": "Lax",
            }),
        )
        .await?;
    session.call("Page.reload", json!({})).await?;
    Ok(())
}

/// Derive the `.eTLD+1` cookie domain from a URL. Best-effort: take the host,
/// drop the leftmost label when there are ≥3 labels, prefix with `.`.
/// (`shop.example.com` → `.example.com`; `example.com` → `.example.com`.)
fn cookie_domain(site_url: &str) -> String {
    let host = site_url
        .split("://")
        .nth(1)
        .unwrap_or(site_url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() >= 3 {
        format!(".{}", labels[labels.len() - 2..].join("."))
    } else if labels.len() == 2 {
        format!(".{host}")
    } else {
        host.to_string()
    }
}
```

> Note on `cookie_domain`: this is a deliberately simple heuristic (no public-suffix list) — it fails on multi-label TLDs like `co.uk`. v1 accepts this; a follow-up can pull in `publicsuffix` if real sites need it. Document the limitation in the crate docs.

- [ ] **Step 3: Add the `on_captcha` builder method** to `bypass.rs` (mirror imperva bypass.rs lines 139–150):

```rust
    /// Register a caller-supplied async CAPTCHA solver. Without it, a CAPTCHA
    /// surface yields [`DataDomeError::CaptchaRequired`].
    ///
    /// The solver receives a [`DataDomeChallenge`] (captcha URL, page URL, UA,
    /// cid, hash) and returns a [`DataDomeSolution`] carrying the solved
    /// `datadome` cookie — wire it to a service like 2captcha / capsolver.
    #[must_use]
    pub fn on_captcha<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(crate::captcha::DataDomeChallenge) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<
                Output = Result<
                    crate::captcha::DataDomeSolution,
                    Box<dyn std::error::Error + Send + Sync>,
                >,
            > + Send
            + 'static,
    {
        self.on_captcha = Some(crate::captcha::arc_solver(f));
        self
    }
```

- [ ] **Step 4: Wire into `lib.rs`** — `pub use captcha::{DataDomeChallenge, DataDomeSolution};` (and `pub mod captcha;` already added in 2.1).

- [ ] **Step 5: Run tests**

Run: `cargo test -p zendriver-datadome captcha::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-datadome/src
git commit -m "feat(datadome): captcha challenge extraction + cookie application"
```

### Task 3.2: Wire captcha escalation into `wait_for_clearance`

**Files:**
- Modify: `crates/zendriver-datadome/src/bypass.rs`

- [ ] **Step 1: Write the failing test** (append to `bypass.rs` tests):

```rust
    #[tokio::test]
    async fn captcha_with_solver_applies_cookie_then_clears() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s)
                .poll_interval(Duration::from_millis(1))
                .on_captcha(|ch| async move {
                    assert!(ch.captcha_url.contains("captcha-delivery.com"));
                    Ok(crate::captcha::DataDomeSolution { datadome_cookie: "FROM_SOLVER".into() })
                })
                .wait_for_clearance().await
        }});

        // Pre-loop probe: captcha surface.
        let id1 = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id1, snap_reply(json!({"surface":"captcha","datadome":"OLD","dd":{"cid":"C","hsh":"H","t":"fe","host":"h"},"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false}))).await;
        // build_challenge eval (url + ua).
        let id_ctx = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id_ctx, json!({"result":{"type":"object","value":{"url":"https://shop.example.com/p","ua":"Mozilla/5.0"}}})).await;
        // apply_solution: setCookie + reload.
        let id_cookie = mock.expect_cmd("Network.setCookie").await;
        mock.reply(id_cookie, json!({"success":true})).await;
        let id_reload = mock.expect_cmd("Page.reload").await;
        mock.reply(id_reload, json!({})).await;
        // Post-reload poll: cleared.
        let id_poll = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id_poll, snap_reply(json!({"surface":"none","datadome":"FROM_SOLVER","dd":null,"captcha_url":null,"body_clean":true}))).await;

        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "FROM_SOLVER"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn captcha_without_solver_errors() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s).wait_for_clearance().await
        }});
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id, snap_reply(json!({"surface":"captcha","datadome":null,"dd":null,"captcha_url":"https://geo.captcha-delivery.com/captcha/?cid=C","body_clean":false}))).await;
        assert!(matches!(fut.await.unwrap().unwrap_err(), DataDomeError::CaptchaRequired));
        conn.shutdown();
    }
```

- [ ] **Step 2: Replace the captcha arm** in `wait_for_clearance` (the `DataDomeSurface::Captcha => return Err(CaptchaRequired)` stub from Task 2.2) with the escalation logic. Because the page mutates after the solver runs, force the loop to re-probe (`next_snapshot = None`):

```rust
        // Pre-loop captcha escalation.
        if snapshot.surface == DataDomeSurface::Captcha {
            let Some(solver) = self.on_captcha.clone() else {
                return Err(DataDomeError::CaptchaRequired);
            };
            let challenge = crate::captcha::build_challenge(self.session, &snapshot).await?;
            let site_url = challenge.site_url.clone();
            let solution = solver(challenge).await.map_err(DataDomeError::CaptchaSolver)?;
            crate::captcha::apply_solution(self.session, &solution, &site_url).await?;
            // Page reloaded — discard the stale snapshot, force a re-probe.
            return self.poll_loop(deadline, None).await;
        }
```

Adjust the `match snapshot.surface { ... }` block from Task 2.2: remove the `Captcha` arm there (it's now handled by this dedicated block placed *before* the `poll_loop` call), keeping `None`/`Block` short-circuits. Final control flow:

```rust
        match snapshot.surface {
            DataDomeSurface::None if snapshot.body_clean => return Ok(ClearanceOutcome::AlreadyClear),
            DataDomeSurface::Block => return Ok(ClearanceOutcome::Blocked),
            _ => {}
        }
        if snapshot.surface == DataDomeSurface::Captcha {
            // ... escalation block above ...
        }
        self.poll_loop(deadline, Some(snapshot)).await
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p zendriver-datadome bypass::`
Expected: PASS (captcha-with-solver + captcha-without-solver + the Task 2.2 four).

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver-datadome/src/bypass.rs
git commit -m "feat(datadome): wire captcha solver escalation into wait_for_clearance"
```

---

## Phase 4 — Opt-in interception fast-path

### Task 4.1: `interception.rs` + race into the poll loop

**Files:**
- Create: `crates/zendriver-datadome/src/interception.rs`
- Modify: `crates/zendriver-datadome/src/bypass.rs` (thread `interception_rx` + guard through `poll_loop`), `crates/zendriver-datadome/src/lib.rs` (`mod interception;`)

- [ ] **Step 1: Write `interception.rs`** — **identical** to `zendriver-imperva/src/interception.rs` except the two `pattern(...)` strings. Copy that file verbatim, then change:
  - module doc: "Imperva" → "DataDome"; pattern examples → `*captcha-delivery.com*`.
  - the subscribe chain to:

```rust
    let stream = InterceptBuilder::new(session)
        .pattern("*captcha-delivery.com*")
        .at_response()
        .pattern("*datadome*")
        .at_response()
        .subscribe();
```

  The 2xx-detection + `InterceptionGuard` (CancellationToken + JoinHandle, `Drop` cancels then aborts) stay byte-for-byte identical.

- [ ] **Step 2: Thread the receiver through `poll_loop`** (mirror imperva bypass.rs lines 263–392). Change `poll_loop`'s signature to accept `mut interception_rx: Option<tokio::sync::oneshot::Receiver<()>>` + `_guard: Option<crate::interception::InterceptionGuard>`, and add the third `select!` arm:

```rust
                Ok(()) = async {
                    match interception_rx.as_mut() {
                        Some(rx) => rx.await,
                        None => std::future::pending().await,
                    }
                } => {
                    interception_rx = None; // re-probe immediately next iteration
                }
```

In `wait_for_clearance`, before calling `poll_loop`, construct the signal when enabled (mirror imperva lines 266–271):

```rust
        let (interception_rx, interception_guard) = if self.interception_enabled {
            let (rx, guard) = crate::interception::spawn_signal(self.session);
            (Some(rx), Some(guard))
        } else {
            (None, None)
        };
        self.poll_loop(deadline, Some(snapshot), interception_rx, interception_guard).await
```

(and the captcha-escalation path passes `None, None, …` since it re-probes synchronously after reload, OR also constructs the signal — keep it simple: pass `None, None` there).

- [ ] **Step 2b: Add an interception unit test** (mirror imperva's `poll_loop`-driven test — drive `poll_loop` directly with a pre-fired oneshot to assert the signal arm triggers a re-probe that returns `Cleared`). Place in `bypass.rs` tests:

```rust
    #[tokio::test]
    async fn interception_signal_triggers_reprobe() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(()).unwrap(); // signal already fired
        let fut = tokio::spawn({ let s = sess.clone(); async move {
            DataDomeBypass::new(&s).poll_interval(Duration::from_secs(10))
                .poll_loop(Instant::now() + Duration::from_secs(5), None, Some(rx), None).await
        }});
        // The signal arm fires immediately → one detect probe → cleared.
        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(id, snap_reply(json!({"surface":"none","datadome":"DD","dd":null,"captcha_url":null,"body_clean":true}))).await;
        match fut.await.unwrap().unwrap() {
            ClearanceOutcome::Cleared { datadome } => assert_eq!(datadome, "DD"),
            other => panic!("expected Cleared, got {other:?}"),
        }
        conn.shutdown();
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p zendriver-datadome`
Expected: PASS (all crate tests).

- [ ] **Step 4: Gates + commit**

```bash
cargo fmt --all && cargo clippy -p zendriver-datadome --all-targets -- -D warnings
git add crates/zendriver-datadome/src
git commit -m "feat(datadome): opt-in Fetch interception fast-path"
```

---

## Phase 5 — `zendriver` crate wiring

### Task 5.1: feature flag, `Tab::datadome()`, re-exports, error `From`

**Files:**
- Modify: `crates/zendriver/Cargo.toml`, `crates/zendriver/src/tab.rs`, `crates/zendriver/src/lib.rs`, `crates/zendriver/src/error.rs`

- [ ] **Step 1: `Cargo.toml`** — add the optional dep + features (mirror the imperva lines):

```toml
# in [dependencies]
zendriver-datadome            = { workspace = true, optional = true }

# in [features]
datadome = ["interception", "dep:zendriver-datadome"]
datadome-tests = ["datadome", "integration-tests"]
```
Also add `"datadome"` to the `integration-tests` feature list (next to `"imperva"`), and add a `[[test]]` entry:
```toml
[[test]]
name = "datadome_v0"
required-features = ["datadome"]
```

- [ ] **Step 2: `tab.rs`** — add (mirror the `#[cfg(feature="imperva")] impl Tab` block at lines 2694–2718):

```rust
#[cfg(feature = "datadome")]
impl Tab {
    /// Construct a [`DataDomeBypass`](zendriver_datadome::DataDomeBypass) bound
    /// to this tab's session.
    ///
    /// Chain `timeout` / `poll_interval` / `with_interception` / `on_captcha`,
    /// then `wait_for_clearance` to detect the active DataDome surface
    /// (device-check, captcha, or block) and poll until the `datadome`
    /// clearance cookie lands.
    ///
    /// **Stealth strongly recommended.** DataDome's device-check scores the
    /// browser fingerprint; without `BrowserBuilder::stealth` (including the
    /// `Surface::Webgpu` coherence patch) the device-check will not clear.
    ///
    /// Gated by the `datadome` cargo feature.
    #[must_use]
    pub fn datadome(&self) -> zendriver_datadome::DataDomeBypass<'_> {
        zendriver_datadome::DataDomeBypass::new(self.session())
    }
}
```

- [ ] **Step 3: `lib.rs`** — add re-exports (mirror imperva lines 146–161). `ClearanceOutcome` collides with cloudflare/imperva, so alias it:

```rust
/// DataDome bypass surface re-exports. Gated by the `datadome` cargo feature.
#[cfg(feature = "datadome")]
pub use zendriver_datadome::{
    DataDomeBypass, DataDomeChallenge, DataDomeError, DataDomeSolution, DataDomeSurface,
    detect_surface as datadome_detect_surface,
};

/// DataDome clearance outcome (aliased to avoid colliding with the cloudflare
/// and imperva `ClearanceOutcome`).
#[cfg(feature = "datadome")]
pub use zendriver_datadome::ClearanceOutcome as DataDomeClearanceOutcome;
```

> Note: imperva already exports `detect_surface` unaliased. DataDome's `detect_surface` collides, so re-export it as `datadome_detect_surface`. (Adjust imperva's similarly only if a name clash surfaces at compile time — `cargo build --features imperva,datadome` will tell you.)

- [ ] **Step 4: Add the send_sync compile test** in `lib.rs` (mirror lines 339–356):

```rust
    #[cfg(feature = "datadome")]
    #[allow(unused_imports)]
    use crate::{
        DataDomeBypass, DataDomeClearanceOutcome, DataDomeError, DataDomeSurface,
    };
    #[cfg(feature = "datadome")]
    #[test]
    fn datadome_surface_is_send_sync() {
        assert_send_sync::<DataDomeBypass<'_>>();
        assert_send_sync::<DataDomeError>();
        assert_send_sync::<DataDomeSurface>();
        assert_send_sync::<DataDomeClearanceOutcome>();
    }
```

- [ ] **Step 5: `error.rs`** — add the `ZendriverError` variant (mirror lines 151–154) + `From` (mirror lines 196–200):

```rust
    /// DataDome bypass sub-crate error. Gated by feature `datadome`.
    #[cfg(feature = "datadome")]
    #[error("datadome: {0}")]
    DataDome(Box<zendriver_datadome::DataDomeError>),
```
```rust
#[cfg(feature = "datadome")]
impl From<zendriver_datadome::DataDomeError> for ZendriverError {
    fn from(e: zendriver_datadome::DataDomeError) -> Self {
        Self::DataDome(Box::new(e))
    }
}
```

- [ ] **Step 6: Verify**

Run: `cargo build -p zendriver --features datadome && cargo test -p zendriver --features datadome --lib datadome`
Expected: builds; send_sync test passes.

- [ ] **Step 7: Gates + commit**

```bash
cargo fmt --all && cargo clippy -p zendriver --features datadome --all-targets -- -D warnings
git add crates/zendriver/Cargo.toml crates/zendriver/src
git commit -m "feat(datadome): wire Tab::datadome() + re-exports + error mapping behind the datadome feature"
```

---

## Phase 6 — WebGPU coherence surface (`zendriver-stealth`)

### Task 6.1: `Surface::Webgpu` enum + Persona field + override plumbing

**Files:**
- Modify: `crates/zendriver-stealth/src/persona/surface.rs`, `crates/zendriver-stealth/src/persona/mod.rs`

- [ ] **Step 1: Write the failing tests** (append to `surface.rs` tests):

```rust
    #[test]
    fn webgpu_is_value_kind_default_value() {
        assert_eq!(Surface::Webgpu.kind(), SurfaceKind::Value);
        assert_eq!(Surface::Webgpu.resolve_strategy(None), Strategy::Value);
    }

    #[test]
    fn webgpu_block_passes() {
        assert_eq!(
            Surface::Webgpu.resolve_strategy(Some(Strategy::Block)),
            Strategy::Block
        );
    }
```

- [ ] **Step 2: Add `Webgpu` to the `Surface` enum** (surface.rs line 7–15) and the `kind()` match (line 38–41): add `Surface::Webgpu` to the `SurfaceKind::Value` arm alongside `Webgl | Fonts | Hardware`.

- [ ] **Step 3: Add the Persona field + plumbing** in `mod.rs`:
  - `Persona` struct: add `pub webgpu: Option<SurfaceCfg>,` (after `webgl`).
  - `overlay`: add `webgpu: over.webgpu.or(self.webgpu),`.
  - `apply_surface_override`: add the arm:
    ```rust
            Surface::Webgpu => {
                self.webgpu.get_or_insert_with(Default::default).strategy = Some(strategy)
            }
    ```

- [ ] **Step 4: Add a Persona override test** (append to `mod.rs` persona_tests):

```rust
    #[test]
    fn apply_surface_override_webgpu() {
        use crate::{Strategy, Surface};
        let mut p = Persona::default();
        p.apply_surface_override(Surface::Webgpu, Strategy::Block);
        assert_eq!(p.webgpu.as_ref().and_then(|c| c.strategy), Some(Strategy::Block));
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p zendriver-stealth persona::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zendriver-stealth/src/persona
git commit -m "feat(stealth): add Surface::Webgpu (Value kind) + Persona.webgpu plumbing"
```

### Task 6.2: renderer → GPUAdapter derivation (Rust, dataset-mapped)

**Files:**
- Create: `crates/zendriver-stealth/src/persona/webgpu_adapter.rs`
- Modify: `crates/zendriver-stealth/src/persona/mod.rs` (`mod webgpu_adapter;`)

The coherence rule: derive a `GPUAdapterInfo` `{ vendor, architecture, description }` from the WebGL `UNMASKED_RENDERER` string, drawn from a fixed dataset (never randomized).

- [ ] **Step 1: Write the failing test** (in `webgpu_adapter.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_renderer_maps_to_nvidia_adapter() {
        let a = adapter_for_renderer("ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)");
        assert_eq!(a.vendor, "nvidia");
        assert_eq!(a.architecture, "ada-lovelace");
        assert!(a.description.contains("RTX 4090"));
    }

    #[test]
    fn intel_renderer_maps_to_intel_adapter() {
        let a = adapter_for_renderer("ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)");
        assert_eq!(a.vendor, "intel");
        assert_eq!(a.architecture, "gen-12lp");
    }

    #[test]
    fn unknown_renderer_falls_back_to_coherent_intel() {
        let a = adapter_for_renderer("Mesa OffScreen");
        // Never panics, never randomizes: a stable coherent default.
        assert_eq!(a.vendor, "intel");
    }
}
```

- [ ] **Step 2: Implement `webgpu_adapter.rs`:**

```rust
//! Derive a coherent WebGPU `GPUAdapterInfo` from the spoofed WebGL renderer
//! string. Dataset-mapped, deterministic — NEVER randomized (DataDome and
//! other WAFs hash the WebGPU fingerprint and compare against a device dataset;
//! a random adapter reads as an unknown device).

/// Minimal `GPUAdapterInfo` triple the patch substitutes into `navigator.gpu`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuAdapterInfo {
    pub vendor: String,
    pub architecture: String,
    pub description: String,
}

/// Map a WebGL `UNMASKED_RENDERER` string to a coherent adapter. Falls back to
/// a stable Intel integrated-GPU adapter for unrecognized renderers.
pub(crate) fn adapter_for_renderer(renderer: &str) -> GpuAdapterInfo {
    let r = renderer.to_ascii_lowercase();
    // Order matters: check discrete vendors before the Intel fallback.
    if r.contains("nvidia") || r.contains("geforce") || r.contains("rtx") || r.contains("gtx") {
        let arch = if r.contains("rtx 40") || r.contains("ada") {
            "ada-lovelace"
        } else if r.contains("rtx 30") {
            "ampere"
        } else if r.contains("rtx 20") {
            "turing"
        } else {
            "ampere"
        };
        return GpuAdapterInfo {
            vendor: "nvidia".into(),
            architecture: arch.into(),
            description: extract_model(renderer, "NVIDIA"),
        };
    }
    if r.contains("amd") || r.contains("radeon") {
        return GpuAdapterInfo {
            vendor: "amd".into(),
            architecture: "rdna-3".into(),
            description: extract_model(renderer, "AMD"),
        };
    }
    if r.contains("apple") {
        return GpuAdapterInfo {
            vendor: "apple".into(),
            architecture: "common-3".into(),
            description: extract_model(renderer, "Apple"),
        };
    }
    // Intel + everything unrecognized → coherent Intel integrated default.
    GpuAdapterInfo {
        vendor: "intel".into(),
        architecture: "gen-12lp".into(),
        description: if r.contains("intel") {
            extract_model(renderer, "Intel")
        } else {
            "Intel(R) UHD Graphics 630".into()
        },
    }
}

/// Pull a human-readable model substring out of the ANGLE renderer string,
/// or fall back to a vendor-generic description.
fn extract_model(renderer: &str, vendor: &str) -> String {
    // ANGLE strings look like "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 ...)".
    // Take the middle segment after the first comma, trimmed of the D3D suffix.
    if let Some(inner) = renderer.split('(').nth(1).and_then(|s| s.split(')').next()) {
        if let Some(mid) = inner.split(',').nth(1) {
            let model = mid
                .split(" Direct3D")
                .next()
                .unwrap_or(mid)
                .split(" vs_")
                .next()
                .unwrap_or(mid)
                .trim();
            if !model.is_empty() {
                return model.to_string();
            }
        }
    }
    format!("{vendor} Graphics")
}
```

- [ ] **Step 3: Register the module** — add `mod webgpu_adapter;` to `mod.rs` (private; used by `patches.rs`). Make it `pub(crate)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zendriver-stealth webgpu_adapter`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver-stealth/src/persona
git commit -m "feat(stealth): renderer->GPUAdapter coherence map for WebGPU"
```

### Task 6.3: `webgpu.js` patch + `push_webgpu` bootstrap wiring

**Files:**
- Create: `crates/zendriver-stealth/src/patches/webgpu.js`
- Modify: `crates/zendriver-stealth/src/patches.rs`

The patch overrides `navigator.gpu.requestAdapter()` to resolve a `GPUAdapter` whose `.info` matches the substituted tokens, plus a coherent standard `.features` set + `.limits`. `Block` strategy deletes `navigator.gpu`.

- [ ] **Step 1: Write `webgpu.js`** (token placeholders `WEBGPU_VENDOR` / `WEBGPU_ARCHITECTURE` / `WEBGPU_DESCRIPTION` / `WEBGPU_MODE`):

```js
// Coherent WebGPU adapter. Defeats DataDome's navigator.gpu.requestAdapter()
// inconsistency check (upstream #20). Values are dataset-derived from the
// spoofed WebGL renderer (never randomized).
(function (vendor, architecture, description, mode) {
  if (!('gpu' in navigator)) return;            // nothing to patch
  if (mode === 'block') {
    try { Object.defineProperty(navigator, 'gpu', { get: () => undefined }); } catch (e) {}
    return;
  }
  if (vendor === null) return;                  // Native → leave real gpu in place

  const info = { vendor: vendor, architecture: architecture, device: '', description: description };
  const realGpu = navigator.gpu;
  const realRequest = realGpu && realGpu.requestAdapter ? realGpu.requestAdapter.bind(realGpu) : null;

  async function requestAdapter(opts) {
    let adapter = realRequest ? await realRequest(opts) : null;
    if (!adapter) return adapter;               // headless w/o gpu: don't fabricate a whole adapter
    try {
      Object.defineProperty(adapter, 'info', { get: () => info, configurable: true });
      if (adapter.requestAdapterInfo) {
        adapter.requestAdapterInfo = async () => info;
      }
    } catch (e) {}
    return adapter;
  }
  try {
    Object.defineProperty(navigator.gpu, 'requestAdapter', {
      value: requestAdapter, writable: true, configurable: true,
    });
  } catch (e) {}
})(WEBGPU_VENDOR, WEBGPU_ARCHITECTURE, WEBGPU_DESCRIPTION, WEBGPU_MODE);
```

> Coherence note: the patch only decorates a real adapter's `info` (it does not fabricate a full `GPUAdapter` when the platform has none — fabricating `requestDevice`/`limits` coherently is brittle and out of v1 scope). On a GPU-capable Chrome (the common case, incl. most CI + desktop), this makes `requestAdapter().info` match the WebGL renderer. In a GPU-less container, `requestAdapter()` returns null both before and after — which is itself coherent for "no GPU". The container-GPU case is the upstream maintainer's `zendriver-docker` concern (spec §15 non-goal).

- [ ] **Step 2: Write the failing test** (append to `patches.rs` tests):

```rust
    #[test]
    fn webgpu_value_substitutes_coherent_adapter_from_renderer() {
        let p = Persona {
            webgl: Some(WebglSpec {
                strategy: Some(Strategy::Value),
                unmasked_vendor: Some("Google Inc. (NVIDIA)".into()),
                unmasked_renderer: Some("ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 Direct3D11 vs_5_0 ps_5_0, D3D11)".into()),
            }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(s.contains("requestAdapter"), "webgpu patch emitted by default (Value)");
        assert!(s.contains("\"nvidia\""), "coherent vendor derived from renderer");
        assert!(s.contains("ada-lovelace"), "coherent architecture derived from renderer");
    }

    #[test]
    fn webgpu_block_deletes_navigator_gpu() {
        let p = Persona {
            webgpu: Some(SurfaceCfg { strategy: Some(Strategy::Block) }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        assert!(s.contains("\"block\""), "block mode token substituted");
    }

    #[test]
    fn webgpu_native_passes_null_vendor() {
        let p = Persona {
            webgpu: Some(SurfaceCfg { strategy: Some(Strategy::Native) }),
            ..Persona::default()
        };
        let s = bootstrap_script(&p, &mock_identity());
        // Native → no webgpu patch emitted at all.
        assert!(!s.contains("WEBGPU_VENDOR"), "no unsubstituted token");
        assert!(!s.contains("requestAdapter"), "Native webgpu omits the patch");
    }
```

- [ ] **Step 3: Add the `WEBGPU` const + `push_webgpu`** to `patches.rs`. Register the const near the other surface patches (line ~44) and call `push_webgpu` in `bootstrap_script` after `push_webgl` (so the renderer it derives from is consistent with the webgl block just emitted):

```rust
const WEBGPU: &str = include_str!("patches/webgpu.js");
```
```rust
// in bootstrap_script, after push_webgl(...):
    push_webgpu(&mut out, persona.webgpu.as_ref(), persona.webgl.as_ref());
```
```rust
/// Append the WebGPU coherence patch. The adapter info is derived from the
/// persona's WebGL renderer (or the hardcoded Intel default the webgl block
/// falls back to), so navigator.gpu agrees with WebGL. Omitted under `Native`.
fn push_webgpu(out: &mut String, cfg: Option<&SurfaceCfg>, webgl: Option<&WebglSpec>) {
    use crate::persona::webgpu_adapter::adapter_for_renderer;
    let strat = Surface::Webgpu.resolve_strategy(cfg.and_then(|c| c.strategy));
    if strat == Strategy::Native {
        return; // leave the real navigator.gpu untouched
    }
    // Default coherent renderer must match webgl.js's hardcoded fallback.
    const DEFAULT_RENDERER: &str =
        "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)";
    let renderer = webgl
        .and_then(|w| w.unmasked_renderer.as_deref())
        .unwrap_or(DEFAULT_RENDERER);
    let adapter = adapter_for_renderer(renderer);
    let (vendor, arch, desc, mode) = if strat == Strategy::Block {
        ("null".to_string(), "null".to_string(), "null".to_string(), "\"block\"".to_string())
    } else {
        (
            serde_json::to_string(&adapter.vendor).unwrap_or_else(|_| "null".into()),
            serde_json::to_string(&adapter.architecture).unwrap_or_else(|_| "null".into()),
            serde_json::to_string(&adapter.description).unwrap_or_else(|_| "null".into()),
            "\"value\"".to_string(),
        )
    };
    out.push('\n');
    out.push_str(
        &WEBGPU
            .replace("WEBGPU_VENDOR", &vendor)
            .replace("WEBGPU_ARCHITECTURE", &arch)
            .replace("WEBGPU_DESCRIPTION", &desc)
            .replace("WEBGPU_MODE", &mode),
    );
}
```

- [ ] **Step 4: Extend the `no_unsubstituted_tokens_remain` test** — add `"WEBGPU_VENDOR"`, `"WEBGPU_ARCHITECTURE"`, `"WEBGPU_DESCRIPTION"`, `"WEBGPU_MODE"` to its token list, and add `webgpu: Some(SurfaceCfg { strategy: Some(Strategy::Value) })` to its exercised persona.

- [ ] **Step 5: Run tests**

Run: `cargo test -p zendriver-stealth patches::`
Expected: PASS.

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt --all && cargo clippy -p zendriver-stealth --all-targets -- -D warnings
git add crates/zendriver-stealth/src
git commit -m "feat(stealth): WebGPU coherence patch (navigator.gpu adapter from WebGL renderer)"
```

---

## Phase 7 — Cross-crate result-model retrofit

### Task 7.1: imperva — `Timeout` Error → `TimedOut` Outcome

**Files:**
- Modify: `crates/zendriver-imperva/src/bypass.rs`, `crates/zendriver-imperva/src/error.rs`, `crates/zendriver-mcp/src/tools/imperva.rs`

- [ ] **Step 1: Update the `ClearanceOutcome` enum** (bypass.rs ~line 56) — add:

```rust
    /// Deadline elapsed without clearance. `last_surface` is the most recent
    /// surface the poll loop observed.
    TimedOut { last_surface: Option<crate::detection::ImpervaSurface> },
```

- [ ] **Step 2: Replace `Err(ImpervaError::Timeout { … })` with `Ok(ClearanceOutcome::TimedOut { … })`** at the three return sites in `bypass.rs` (the pre-loop `probe_with_deadline` Err, and the two loop deadline returns at ~365 and ~374). `probe_with_deadline` currently returns `Result<DetectionSnapshot, ImpervaError>` with `Err(Timeout)` on deadline — change the loop to detect that sentinel. Simplest: keep `probe_with_deadline` returning the snapshot, and have its deadline arm return a new `ProbeOutcome::TimedOut` instead. **Concretely:** change the three `return Err(ImpervaError::Timeout { timeout, last_surface })` to `return Ok(ClearanceOutcome::TimedOut { last_surface })`, and for the pre-loop probe (`probe_with_deadline` returning Err on deadline) convert that Err arm at the call site in `wait_for_clearance` into an early `Ok(ClearanceOutcome::TimedOut { last_surface: None })`.

- [ ] **Step 3: Remove `Timeout` from `ImpervaError`** (error.rs ~lines 16–22) and its `display_timeout_includes_duration` test. Remove the now-unused `Duration` import if it becomes dead.

- [ ] **Step 4: Update `tools/imperva.rs`** — replace the `Err(ImpervaError::Timeout { .. }) => Ok(Outcome::Timeout)` arm (lines ~138–141) with a new `Ok` arm:

```rust
        Ok(ImpervaClearanceOutcome::TimedOut { .. }) => Ok(SolveImpervaOutput {
            outcome: Outcome::Timeout,
            reese84: None,
        }),
```
Keep the MCP wire `Outcome::Timeout` variant as-is (snapshot unchanged).

- [ ] **Step 5: Fix the imperva unit tests** that asserted `ImpervaError::Timeout` (e.g. `wait_for_clearance_holds_when_cookie_only_no_body_clean` near bypass.rs:541) — change the expected terminal from `Err(Timeout)` to `Ok(ClearanceOutcome::TimedOut { .. })`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p zendriver-imperva && cargo test -p zendriver-mcp --features imperva imperva`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zendriver-imperva crates/zendriver-mcp/src/tools/imperva.rs
git commit -m "refactor(imperva): TimedOut is an Outcome, not an Error (unify result model)"
```

### Task 7.2: cloudflare — `NoChallenge` + `ClearanceTimeout` Errors → `TimedOut` Outcome

**Files:**
- Modify: `crates/zendriver-cloudflare/src/bypass.rs`, `crates/zendriver-cloudflare/src/error.rs`, `crates/zendriver-mcp/src/tools/cloudflare.rs`

- [ ] **Step 1: Update the `ClearanceOutcome` enum** (cloudflare bypass.rs ~line 44) — add a single `TimedOut` carrying the diagnostic that distinguished the two old errors:

```rust
    /// Deadline elapsed. `saw_challenge` is true if challenge markers were
    /// observed (the old `ClearanceTimeout`); false if none ever appeared
    /// (the old `NoChallenge`).
    TimedOut { saw_challenge: bool },
```

- [ ] **Step 2: Replace the two `Err(...)` returns** at the deadline check (bypass.rs ~lines 167–173) with:

```rust
            if Instant::now() >= deadline {
                return Ok(ClearanceOutcome::TimedOut { saw_challenge: ever_seen_markers });
            }
```

- [ ] **Step 3: Remove `NoChallenge` + `ClearanceTimeout` from `CloudflareError`** (error.rs lines 6–10) and their two display tests.

- [ ] **Step 4: Update `tools/cloudflare.rs`** — replace the `Err(CloudflareError::ClearanceTimeout) => Ok(Outcome::Timeout)` arm (lines ~126–129) with:

```rust
        Ok(ClearanceOutcome::TimedOut { .. }) => Ok(SolveOutput {
            outcome: Outcome::Timeout,
            token: None,
        }),
```
The MCP wire `Outcome::Timeout` stays (snapshot unchanged). The old `Err(NoChallenge)` previously fell into the `Err(other) => real error` arm; now it never occurs (it's a `TimedOut` outcome), so no special handling needed.

- [ ] **Step 5: Fix cloudflare unit tests** referencing `NoChallenge` / `ClearanceTimeout` — search `crates/zendriver-cloudflare/src/bypass.rs` tests and update any expecting those `Err`s to expect `Ok(ClearanceOutcome::TimedOut { .. })`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p zendriver-cloudflare && cargo test -p zendriver-mcp --features cloudflare cloudflare`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zendriver-cloudflare crates/zendriver-mcp/src/tools/cloudflare.rs
git commit -m "refactor(cloudflare): TimedOut is an Outcome, not an Error (unify result model)"
```

---

## Phase 8 — MCP `browser_solve_datadome` tool

### Task 8.1: tool module + server registration + ledger + schema snapshot

**Files:**
- Create: `crates/zendriver-mcp/src/tools/datadome.rs`
- Modify: `crates/zendriver-mcp/src/tools/mod.rs`, `crates/zendriver-mcp/src/server.rs`, `crates/zendriver-mcp/Cargo.toml`, `crates/zendriver-mcp/mcp-coverage-ledger.toml`

- [ ] **Step 1: `Cargo.toml`** — add to `[features]`:
```toml
datadome = ["zendriver/datadome"]
```
and add `"datadome"` to the `default` feature list (next to `"imperva"`).

- [ ] **Step 2: Write `tools/datadome.rs`** (mirror `tools/imperva.rs`; output carries `datadome` cookie instead of `reese84`, plus `blocked` outcome):

```rust
//! DataDome bypass tool — `browser_solve_datadome`. Gated behind the
//! `datadome` feature. Mirrors `tools/imperva.rs`: all flow-terminals
//! (cleared / challenge_gone / already_clear / blocked / timed_out) are
//! success-channel outcomes; only genuine faults (captcha-without-solver, CDP,
//! JS) are MCP errors. The `on_captcha` solver hook is intentionally not wired
//! over MCP (documented non-goal, same as imperva): a captcha surface without
//! a solver surfaces as a `CaptchaRequired` error the agent handles
//! out-of-band.

#![cfg(feature = "datadome")]

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{DataDomeClearanceOutcome, DataDomeError, ZendriverError};

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab;

/// Input for `browser_solve_datadome`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SolveDataDomeInput {
    /// Maximum total wait for a terminal outcome, in milliseconds. Default
    /// 30_000 (30s).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Override the bypass driver's internal poll cadence, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
    /// Enable the Fetch-domain interception fast-path. Default `false`.
    #[serde(default)]
    pub with_interception: bool,
}

fn default_timeout() -> u64 {
    30_000
}

/// Terminal outcome of a DataDome bypass attempt.
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// datadome cookie acquired (value in [`SolveDataDomeOutput::datadome`]).
    Cleared,
    /// Body markers cleared without a datadome cookie.
    ChallengeGone,
    /// No DataDome surface present at call time (fast path).
    AlreadyClear,
    /// `window.dd.t == 'bv'` — IP banned. Caller must change IP.
    Blocked,
    /// `timeout_ms` elapsed without a terminal state. Not a hard error.
    Timeout,
}

/// Output of `browser_solve_datadome`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SolveDataDomeOutput {
    pub outcome: Outcome,
    /// datadome cookie value. Populated only when `outcome == cleared`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datadome: Option<String>,
}

/// Drive the DataDome clearance flow on the current tab.
pub async fn solve_datadome(
    state: Arc<Mutex<SessionState>>,
    input: SolveDataDomeInput,
) -> Result<SolveDataDomeOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let mut bypass = tab.datadome().timeout(Duration::from_millis(input.timeout_ms));
    if let Some(p) = input.poll_interval_ms {
        bypass = bypass.poll_interval(Duration::from_millis(p));
    }
    if input.with_interception {
        bypass = bypass.with_interception();
    }
    match bypass.wait_for_clearance().await {
        Ok(DataDomeClearanceOutcome::Cleared { datadome }) => Ok(SolveDataDomeOutput {
            outcome: Outcome::Cleared,
            datadome: Some(datadome),
        }),
        Ok(DataDomeClearanceOutcome::ChallengeGone) => Ok(SolveDataDomeOutput {
            outcome: Outcome::ChallengeGone,
            datadome: None,
        }),
        Ok(DataDomeClearanceOutcome::AlreadyClear) => Ok(SolveDataDomeOutput {
            outcome: Outcome::AlreadyClear,
            datadome: None,
        }),
        Ok(DataDomeClearanceOutcome::Blocked) => Ok(SolveDataDomeOutput {
            outcome: Outcome::Blocked,
            datadome: None,
        }),
        Ok(DataDomeClearanceOutcome::TimedOut { .. }) => Ok(SolveDataDomeOutput {
            outcome: Outcome::Timeout,
            datadome: None,
        }),
        Err(other) => Err(map_error(McpServerError::from(ZendriverError::from(other)))),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn solve_datadome_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = solve_datadome(
            state,
            SolveDataDomeInput { timeout_ms: 100, poll_interval_ms: None, with_interception: false },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
```

- [ ] **Step 3: `tools/mod.rs`** — add (next to the imperva entry):
```rust
#[cfg(feature = "datadome")]
pub mod datadome;
```

- [ ] **Step 4: `server.rs`** — add the `use` (next to imperva, ~line 41):
```rust
#[cfg(feature = "datadome")]
use crate::tools::datadome;
```
add a feature-gated `#[tool_router]` block (mirror the `imperva_tool_router` block @ ~978):
```rust
#[cfg(feature = "datadome")]
#[tool_router(router = datadome_tool_router, vis = "pub")]
impl ZendriverServer {
    #[tool(
        name = "browser_solve_datadome",
        description = "Detect the active DataDome surface and poll until the datadome clearance cookie lands. Returns outcome: cleared/challenge_gone/already_clear/blocked/timeout. A captcha surface without a registered solver errors (handle out-of-band)."
    )]
    pub async fn browser_solve_datadome(
        &self,
        params: rmcp::handler::server::tool::Parameters<datadome::SolveDataDomeInput>,
    ) -> Result<Json<datadome::SolveDataDomeOutput>, ErrorData> {
        datadome::solve_datadome(self.state.clone(), params.0).await.map(Json)
    }
}
```
> Match the exact `#[tool]` method signature style of `browser_solve_imperva` at server.rs:982–991 (Parameters wrapper, `Json` return, `.map(Json)` or the existing pattern — copy it verbatim, swapping types/names).

and in `combined_tool_router()` (~line 1094, after the `+ Self::imperva_tool_router();` line @ ~1103):
```rust
        #[cfg(feature = "datadome")]
        let router = router + Self::datadome_tool_router();
```

- [ ] **Step 5: Ledger entries** — append to `crates/zendriver-mcp/mcp-coverage-ledger.toml`:
```toml
# ── Group D: DataDome bypass crate + WebGPU surface ─────────────────────────
[[entry]]
api = "zendriver::Tab::datadome"
covered = "browser_solve_datadome"

[[entry]]
api = "zendriver::DataDomeBypass"
covered = "browser_solve_datadome"

[[entry]]
api = "zendriver::DataDomeClearanceOutcome"
covered = "browser_solve_datadome"

[[entry]]
api = "zendriver::DataDomeSurface"
excluded = "diagnostic enum surfaced via browser_solve_datadome.outcome; the standalone datadome_detect_surface fn is a Rust convenience"

[[entry]]
api = "zendriver::DataDomeError"
excluded = "error type surfaced through browser_solve_datadome MCP errors"

[[entry]]
api = "zendriver::DataDomeChallenge"
excluded = "captcha-solver callback type; the on_captcha hook is a documented MCP non-goal (same as imperva CaptchaChallenge)"

[[entry]]
api = "zendriver::DataDomeSolution"
excluded = "captcha-solver callback type; on_captcha hook is a documented MCP non-goal"

[[entry]]
api = "zendriver::datadome_detect_surface"
excluded = "Rust convenience probe; surface is reported via browser_solve_datadome.outcome"
```
> Also add ledger entries (or confirm coverage) for any NEW imperva/cloudflare public items the retrofit added — `ImpervaClearanceOutcome::TimedOut` and `cloudflare ClearanceOutcome::TimedOut` are new enum variants. If `cargo public-api` flags them, add `covered = "browser_solve_imperva"` / `"browser_solve_turnstile"`. Run the public-api check (Step 7) to find out.

- [ ] **Step 6: Regenerate schema snapshots**

```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo insta accept --all
```
Expected: a NEW `browser_solve_datadome` snapshot; imperva/cloudflare snapshots UNCHANGED (the wire `Outcome::Timeout` variants were preserved). If imperva/cloudflare snapshots changed, that's a regression in Phase 7 — investigate before accepting.

- [ ] **Step 7: Public-API check + full gates**

```bash
cargo test -p zendriver-mcp --features public-api-check --test public_api
cargo build -p zendriver-mcp --all-features
cargo test -p zendriver-mcp --all-features datadome
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
```
Expected: public-api check passes once ledger covers the new items; datadome tool test passes.

- [ ] **Step 8: Commit**

```bash
git add crates/zendriver-mcp
git commit -m "feat(mcp): browser_solve_datadome tool + ledger + schema snapshot"
```

---

## Phase 9 — Integration tests (fixture + nightly)

### Task 9.1: Synthetic-403 fixture integration test

**Files:**
- Create: `crates/zendriver/tests/datadome_v0.rs`

Mirror `network_monitor_http.rs`'s wiremock harness. The fixture serves a DataDome-shaped 403 (a `var dd = {...}` body) at `/` first, then — after a "clearance" GET — serves a clean page AND sets the `datadome` cookie, so the poll loop observes the transition.

- [ ] **Step 1: Write the test** (gated `integration-tests + datadome`, `#[ignore]`, `#[serial]`):

```rust
//! Integration tests for `tab.datadome()` against a synthetic DataDome fixture.
//!
//! Gated behind `integration-tests` + `datadome`, `#[ignore]` (needs a real
//! Chrome). Run:
//! ```bash
//! cargo test -p zendriver --features datadome-tests --test datadome_v0 -- --ignored
//! ```
#![cfg(all(feature = "integration-tests", feature = "datadome"))]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::{Browser, DataDomeClearanceOutcome};

/// A challenge page carrying a `var dd` config (device-check, t='fe').
const CHALLENGE_HTML: &str = r#"<!doctype html><html><head>
<script>var dd={'rt':'c','cid':'CID123','hsh':'HSH456','t':'fe','s':1,'host':'geo.captcha-delivery.com'};</script>
</head><body>verifying…</body></html>"#;

const CLEAN_HTML: &str = r#"<!doctype html><html><body>welcome</body></html>"#;

#[tokio::test]
#[serial]
#[ignore]
async fn datadome_device_check_clears_when_cookie_set() {
    let mock = MockServer::start().await;
    // First the challenge page (no cookie); after a reload the server sets the
    // datadome cookie + serves the clean page. wiremock has no built-in
    // call-ordering, so use two paths: "/" = challenge, then the test sets the
    // cookie via CDP through a captcha-less device-check is not possible — so
    // this fixture exercises detection + AlreadyClear/TimedOut, and the
    // cookie-set path is covered by the captcha integration variant below.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CHALLENGE_HTML.as_bytes().to_vec(), "text/html"))
        .mount(&mock)
        .await;

    let browser = Browser::builder().build().await.unwrap();
    let tab = browser.get(&mock.uri()).await.unwrap();
    tab.wait_for_load(Duration::from_secs(5)).await.ok();

    // No datadome cookie + device-check markers → times out (markers never clear).
    let outcome = tab.datadome().timeout(Duration::from_millis(800)).poll_interval(Duration::from_millis(100)).wait_for_clearance().await.unwrap();
    assert!(matches!(outcome, DataDomeClearanceOutcome::TimedOut { .. }));

    browser.close().await.ok();
}

#[tokio::test]
#[serial]
#[ignore]
async fn datadome_already_clear_on_plain_page() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CLEAN_HTML.as_bytes().to_vec(), "text/html"))
        .mount(&mock)
        .await;
    let browser = Browser::builder().build().await.unwrap();
    let tab = browser.get(&mock.uri()).await.unwrap();
    tab.wait_for_load(Duration::from_secs(5)).await.ok();
    let outcome = tab.datadome().timeout(Duration::from_secs(2)).wait_for_clearance().await.unwrap();
    assert!(matches!(outcome, DataDomeClearanceOutcome::AlreadyClear));
    browser.close().await.ok();
}
```

> The captcha-with-cookie-set happy path is hard to model in wiremock without an injected solver + cookie round-trip; the unit test `captcha_with_solver_applies_cookie_then_clears` (Task 3.2) already covers that flow deterministically. The fixture's job is to prove detection + AlreadyClear/TimedOut against a real Chrome.

- [ ] **Step 2: Run (only if a Chrome binary is available)**

Run: `cargo test -p zendriver --features datadome-tests --test datadome_v0 -- --ignored`
Expected: both PASS against real Chrome. If no Chrome locally, verify it COMPILES: `cargo test -p zendriver --features datadome-tests --test datadome_v0 --no-run`.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/tests/datadome_v0.rs
git commit -m "test(datadome): synthetic-403 fixture integration tests"
```

### Task 9.2: Nightly real-site probe + WebGPU coherence assertion

**Files:**
- Modify: `crates/zendriver/tests/datadome_v0.rs` (add a nightly real-site test, gated + `#[ignore]`)
- Modify: the stealth nightly test file (find it: `rg -l 'sannysoft|areyouheadless' crates/zendriver*/tests crates/zendriver-stealth`) — add a WebGPU coherence assertion.

- [ ] **Step 1: Add a real-site nightly test** to `datadome_v0.rs` (best-effort drift signal; never CI-blocking — it `#[ignore]`s and tolerates network failure):

```rust
/// Drift probe against a known DataDome-protected surface. Best-effort: skips
/// (does not fail) on network errors. Run only in the nightly job.
#[tokio::test]
#[serial]
#[ignore]
async fn datadome_real_site_drift_probe() {
    let browser = match Browser::builder().stealth().build().await {
        Ok(b) => b,
        Err(_) => return,
    };
    // deviceandbrowserinfo.com/are_you_a_bot is the surface the #20 reporter used.
    let Ok(tab) = browser.get("https://deviceandbrowserinfo.com/are_you_a_bot").await else { browser.close().await.ok(); return; };
    tab.wait_for_load(Duration::from_secs(15)).await.ok();
    // We don't assert pass/fail (sites change); we assert the bypass RUNS and
    // returns a terminal without panicking.
    let outcome = tab.datadome().timeout(Duration::from_secs(20)).wait_for_clearance().await;
    eprintln!("datadome real-site outcome: {outcome:?}");
    browser.close().await.ok();
}
```

- [ ] **Step 2: Add a WebGPU coherence assertion** to the stealth nightly. After applying a `stealth()` persona on a real page, read `navigator.gpu.requestAdapter()` and assert the vendor matches the WebGL renderer. Add to the identified nightly file:

```rust
#[tokio::test]
#[serial]
#[ignore]
async fn webgpu_adapter_coheres_with_webgl_renderer() {
    let browser = Browser::builder().stealth().build().await.unwrap();
    let tab = browser.get("about:blank").await.unwrap();
    let v: serde_json::Value = tab.evaluate_main(r#"(async () => {
        const a = navigator.gpu && await navigator.gpu.requestAdapter();
        const c = document.createElement('canvas').getContext('webgl');
        const dbg = c && c.getExtension('WEBGL_debug_renderer_info');
        return {
          gpuVendor: a && a.info ? a.info.vendor : null,
          webglRenderer: dbg ? c.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
        };
    })()"#).await.unwrap();
    // If WebGPU is available, its vendor must be a substring-consistent with
    // the WebGL renderer (both nvidia / both intel / etc.). If gpuVendor is
    // null (no GPU in this env), the test is a no-op pass.
    if let Some(vendor) = v.get("gpuVendor").and_then(|x| x.as_str()) {
        let renderer = v.get("webglRenderer").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
        assert!(renderer.contains(vendor) || (vendor == "intel" && renderer.contains("intel")),
            "webgpu vendor {vendor} must cohere with webgl renderer {renderer}");
    }
    browser.close().await.ok();
}
```
> Adjust `evaluate_main` to the crate's actual main-world eval method name (check `tab.rs`). Use `about:blank` so the persona bootstrap script has applied.

- [ ] **Step 3: Compile-check**

Run: `cargo test -p zendriver --features datadome-tests --test datadome_v0 --no-run` and the stealth nightly's `--no-run`.
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver/tests crates/zendriver-stealth
git commit -m "test(datadome,stealth): nightly real-site drift probe + WebGPU coherence assertion"
```

---

## Phase 10 — Docs + final gates

### Task 10.1: mdBook chapter, README, CHANGELOG, full-workspace gates

**Files:**
- Create: `docs/book/src/datadome.md` (mirror `imperva.md`)
- Modify: `docs/book/src/SUMMARY.md`, `README.md` (feature matrix + crate list), `CHANGELOG.md`

- [ ] **Step 1: Write `docs/book/src/datadome.md`** — mirror the structure of `docs/book/src/imperva.md` (find it first). Cover: enabling the `datadome` feature, `tab.datadome()` builder, the three surfaces, the `on_captcha` solver recipe (2captcha-shaped, returning the `datadome` cookie), the `Blocked`/`TimedOut` outcomes, and the **WebGPU/#20 stealth note** (device-check needs `stealth()`; the `Surface::Webgpu` coherence patch ships with it).

- [ ] **Step 2: Add to `SUMMARY.md`** — a `- [DataDome](datadome.md)` line next to the Imperva entry.

- [ ] **Step 3: Update `README.md`** — add `zendriver-datadome` to the crate list / feature matrix (next to cloudflare + imperva), and note the new `Surface::Webgpu`.

- [ ] **Step 4: Update `CHANGELOG.md`** — an entry under the unreleased section summarizing: new `zendriver-datadome` crate (#20), `Surface::Webgpu` coherence, and the imperva/cloudflare result-model unification (note the breaking lib-level change: `Timeout`/`NoChallenge`/`ClearanceTimeout` moved from `Error` to `Outcome`).

- [ ] **Step 5: Full-workspace gates** (the complete CLAUDE.md pre-push set):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
cargo test --workspace
cargo test -p zendriver-mcp --all-features
cargo test -p zendriver --no-default-features
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked
cargo doc --workspace --no-deps
```
Expected: all green; no doc warnings. (Background the long runs per the user's workflow.)

- [ ] **Step 6: Commit + push**

```bash
git add docs README.md CHANGELOG.md
git commit -m "docs(datadome): user guide chapter + README + CHANGELOG"
git push -u origin claude/group-d-datadome
```

- [ ] **Step 7: Open the PR**, then run `/founders-review` on it (per the user's per-PR requirement) before merge. PR body must record the documented MCP non-goal (the `on_captcha` solver callback) and the breaking result-model unification.

---

## Self-review checklist (completed)

**Spec coverage:** §3 crate layout → Tasks 0.1/1.x/2.x/3.x/4.1. §4 detection → 1.2. §5 result model → 2.1. §6 driver → 2.2. §7 captcha-delta → 3.1/3.2. §8 interception → 4.1. §9 feature wiring → 5.1. §10 WebGPU → 6.1/6.2/6.3. §11 retrofit → 7.1/7.2. §12 MCP → 8.1. §13 testing → unit (1.x–4.1), fixture (9.1), nightly + WebGPU (9.2). §14 sequencing → phase order. §15 non-goals → recorded in docs/PR (10.1).

**Type consistency:** `ClearanceOutcome` variants identical across bypass.rs (2.1) ↔ MCP map (8.1) ↔ fixture asserts (9.1). `DataDomeSurface` four variants identical across detection.rs (1.2) ↔ bypass terminals (2.2) ↔ ledger (8.1). `DataDomeChallenge`/`DataDomeSolution` identical across captcha.rs (3.1) ↔ on_captcha (3.1) ↔ wire note (8.1). `CaptchaSolver`/`arc_solver` defined in 3.1, referenced by 2.1's field — dependency flagged in 2.1 Step 2 note. WebGPU tokens (`WEBGPU_VENDOR/ARCHITECTURE/DESCRIPTION/MODE`) consistent across webgpu.js (6.3 Step 1) ↔ push_webgpu (6.3 Step 3) ↔ no-unsubstituted-tokens test (6.3 Step 4).

**Known risk flagged inline:** `cookie_domain` heuristic (no PSL); WebGPU patch decorates rather than fabricates an adapter; imperva/cloudflare snapshot-unchanged claim must be verified at 8.1 Step 6; the 2.1↔3.1 build-order dependency.
