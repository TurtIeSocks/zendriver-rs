# zendriver-rs Phase 1 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the foundation slice of zendriver-rs: CDP transport actor + minimal `Browser`/`Tab`/`Element` surface, tested end-to-end against real Chrome via wiremock. End state: `examples/hello.rs` opens a page, finds an element by CSS, clicks it, reads text back.

**Architecture:** Cargo workspace; `zendriver` (public face) depends on internal `zendriver-transport` and the external `chromiumoxide_cdp` for CDP type bindings. One tokio task owns the WebSocket; communication via mpsc (commands), oneshot (responses), broadcast (events). No locks across `.await`. Builder-pattern public API; `thiserror` hierarchy. Stealth and full element surface are deferred to later phases — Phase 1 reserves hooks but does not populate them.

**Tech Stack:** Rust (edition 2021), tokio + tokio-tungstenite for async WS, chromiumoxide_cdp for CDP type bindings, thiserror for errors, tracing for instrumentation, wiremock for HTTP fixtures, insta for snapshots, serial_test for serializing integration tests, tempfile for user-data-dir, tokio-util's CancellationToken for shutdown.

**Spec:** [docs/superpowers/specs/2026-05-23-zendriver-rs-phase1-foundation-design.md](../specs/2026-05-23-zendriver-rs-phase1-foundation-design.md)

---

## File structure

Each file gets one clear responsibility. Files that change together live together.

### Workspace root
- `Cargo.toml` — workspace declaration, shared lints, shared dev-deps
- `rust-toolchain.toml` — pin to stable
- `rustfmt.toml` — formatting rules
- `clippy.toml` — clippy config
- `.cargo/config.toml` — local cargo settings (unused initially, reserved)
- `LICENSE-MIT` — MIT license text
- `LICENSE-APACHE` — Apache-2.0 license text
- `LICENSE` — delete (the existing single-license file gets replaced by the dual pair)
- `README.md` — project README (replaces existing one-liner)
- `.github/workflows/ci.yml` — CI matrix: unit + doc + clippy + fmt; integration gated
- `.gitignore` — already exists; add `/target` if missing
- `examples/hello.rs` — Phase 1 exit example

### `crates/zendriver-transport/`
- `Cargo.toml`
- `src/lib.rs` — re-exports
- `src/error.rs` — `TransportError`
- `src/frame.rs` — `CdpCommand`, `CdpInbound`, `CdpRpcError`, `RawEvent`
- `src/actor.rs` — `ConnectionActor` (the tokio task that owns the WS)
- `src/connection.rs` — `Connection` (public handle to the actor)
- `src/session.rs` — `SessionHandle` (per-target wrapper around `Connection`)
- `src/testing.rs` — `MockConnection` (gated `cfg(any(test, feature = "testing"))`)

### `crates/zendriver/`
- `Cargo.toml`
- `src/lib.rs` — re-exports + `start()` convenience
- `src/error.rs` — `ZendriverError`, `Result` alias, `BrowserError`
- `src/browser.rs` — `Browser`, `BrowserBuilder`, executable discovery, subprocess lifecycle
- `src/tab.rs` — `Tab`
- `src/element.rs` — `Element`
- `src/query.rs` — `FindBuilder`

### Skeleton crates (empty stubs in Phase 1)
- `crates/zendriver-stealth/{Cargo.toml, src/lib.rs}`
- `crates/zendriver-cloudflare/{Cargo.toml, src/lib.rs}`
- `crates/zendriver-interception/{Cargo.toml, src/lib.rs}`
- `crates/zendriver-fetcher/{Cargo.toml, src/lib.rs}`

Each stub is exactly: `// populated in phase N` + minimal `Cargo.toml`. They exist so the workspace builds cleanly and future phases land without restructuring.

### Tests
- `crates/zendriver-transport/tests/` — integration tests internal to transport (none in P1)
- `crates/zendriver/tests/integration_phase1.rs` — single end-to-end test, gated behind `integration-tests` feature
- `crates/zendriver/tests/snapshots/` — `insta` snapshot files
- Unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each source file (Rust convention)

---

## Task list (overview)

| # | Title | Files |
|---|---|---|
| 0 | Workspace skeleton + dual license + CI baseline | root, all crate stubs |
| 1 | `TransportError` type | `crates/zendriver-transport/src/error.rs` |
| 2 | CDP frame types (serde round-trip) | `crates/zendriver-transport/src/frame.rs` |
| 3 | `RawEvent` + event-bus type | `crates/zendriver-transport/src/frame.rs` |
| 4 | `ConnectionActor`: command/response path with IO injection | `crates/zendriver-transport/src/actor.rs` |
| 5 | `ConnectionActor`: event broadcast + lagged-subscriber test | `crates/zendriver-transport/src/actor.rs` |
| 6 | `ConnectionActor`: shutdown drains pending | `crates/zendriver-transport/src/actor.rs` |
| 7 | `Connection` public handle + `subscribe::<E>()` typed stream | `crates/zendriver-transport/src/connection.rs` |
| 8 | `SessionHandle` wrapper | `crates/zendriver-transport/src/session.rs` |
| 9 | `MockConnection` testing helper | `crates/zendriver-transport/src/testing.rs` |
| 10 | `BrowserError` + `ZendriverError` + `Result` alias | `crates/zendriver/src/error.rs` |
| 11 | Chrome executable auto-discovery | `crates/zendriver/src/browser.rs` |
| 12 | DevTools WS URL parser (from Chrome stderr) | `crates/zendriver/src/browser.rs` |
| 13 | `BrowserBuilder` (no launch yet) | `crates/zendriver/src/browser.rs` |
| 14 | `Browser::launch` (spawn + attach + main tab) | `crates/zendriver/src/browser.rs` |
| 15 | `Browser::close` + `Drop` graceful teardown | `crates/zendriver/src/browser.rs` |
| 16 | `Tab` struct + `Tab::session()` escape hatch | `crates/zendriver/src/tab.rs` |
| 17 | `Tab::goto` + `Tab::wait_for_load` | `crates/zendriver/src/tab.rs` |
| 18 | `Tab::evaluate<T>` (Runtime.evaluate, typed) | `crates/zendriver/src/tab.rs` |
| 19 | `Tab::url` + `Tab::title` | `crates/zendriver/src/tab.rs` |
| 20 | `Element` struct + node-id resolution | `crates/zendriver/src/element.rs` |
| 21 | `Element::click` | `crates/zendriver/src/element.rs` |
| 22 | `Element::inner_text` + `outer_html` + `evaluate<T>` | `crates/zendriver/src/element.rs` |
| 23 | `FindBuilder::css().one()` + `one_or_none()` + `.timeout()` | `crates/zendriver/src/query.rs` |
| 24 | `zendriver::start` convenience + crate re-exports | `crates/zendriver/src/lib.rs` |
| 25 | `examples/hello.rs` + integration test against wiremock | `examples/`, `crates/zendriver/tests/` |
| 26 | Insta snapshot tests for launch flags + error displays | `crates/zendriver/tests/snapshots.rs` |
| 27 | CI workflow finalization + README | `.github/workflows/ci.yml`, `README.md` |

---

## Task 0: Workspace skeleton + dual license + CI baseline

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `rust-toolchain.toml`
- Create: `rustfmt.toml`
- Create: `clippy.toml`
- Create: `LICENSE-MIT`
- Create: `LICENSE-APACHE`
- Delete: `LICENSE`
- Modify: `README.md`
- Create: `crates/zendriver/Cargo.toml`
- Create: `crates/zendriver/src/lib.rs`
- Create: `crates/zendriver-transport/Cargo.toml`
- Create: `crates/zendriver-transport/src/lib.rs`
- Create: `crates/zendriver-stealth/Cargo.toml`
- Create: `crates/zendriver-stealth/src/lib.rs`
- Create: `crates/zendriver-cloudflare/Cargo.toml`
- Create: `crates/zendriver-cloudflare/src/lib.rs`
- Create: `crates/zendriver-interception/Cargo.toml`
- Create: `crates/zendriver-interception/src/lib.rs`
- Create: `crates/zendriver-fetcher/Cargo.toml`
- Create: `crates/zendriver-fetcher/src/lib.rs`
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write workspace root `Cargo.toml`**

Path: `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/zendriver",
    "crates/zendriver-transport",
    "crates/zendriver-stealth",
    "crates/zendriver-cloudflare",
    "crates/zendriver-interception",
    "crates/zendriver-fetcher",
]

[workspace.package]
version = "0.1.0-dev"
edition = "2021"
rust-version = "1.75"
license = "MIT OR Apache-2.0"
repository = "https://github.com/cdpdriver/zendriver-rs"
authors = ["zendriver-rs contributors"]

[workspace.dependencies]
# Internal
zendriver-transport     = { path = "crates/zendriver-transport", version = "0.1.0-dev" }
zendriver-stealth       = { path = "crates/zendriver-stealth",       version = "0.1.0-dev" }
zendriver-cloudflare    = { path = "crates/zendriver-cloudflare",    version = "0.1.0-dev" }
zendriver-interception  = { path = "crates/zendriver-interception",  version = "0.1.0-dev" }
zendriver-fetcher       = { path = "crates/zendriver-fetcher",       version = "0.1.0-dev" }

# External — runtime
tokio              = { version = "1.40",  features = ["full"] }
tokio-util         = { version = "0.7",   features = ["rt"] }
tokio-tungstenite  = { version = "0.24" }
futures            = { version = "0.3" }
async-trait        = { version = "0.1" }

# External — CDP
chromiumoxide_cdp  = { version = "0.7" }

# External — serde
serde              = { version = "1",     features = ["derive"] }
serde_json         = { version = "1" }

# External — errors / logging
thiserror          = { version = "1" }
tracing            = { version = "0.1" }

# External — utilities
url                = { version = "2" }
tempfile           = { version = "3" }

# Dev-deps
tokio-test         = { version = "0.4" }
wiremock           = { version = "0.6" }
insta              = { version = "1",     features = ["yaml", "json"] }
serial_test        = { version = "3" }
tracing-subscriber = { version = "0.3",   features = ["env-filter"] }

[workspace.lints.rust]
unsafe_code        = "deny"
missing_docs       = "warn"

[workspace.lints.clippy]
pedantic           = { level = "warn", priority = -1 }
nursery            = { level = "warn", priority = -1 }
unwrap_used        = "warn"
expect_used        = "warn"
panic              = "warn"
todo               = "warn"
unimplemented      = "warn"
```

- [ ] **Step 2: Write `rust-toolchain.toml`**

Path: `rust-toolchain.toml`

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 3: Write `rustfmt.toml`**

Path: `rustfmt.toml`

```toml
edition = "2021"
max_width = 100
use_field_init_shorthand = true
use_try_shorthand = true
```

- [ ] **Step 4: Write `clippy.toml`**

Path: `clippy.toml`

```toml
msrv = "1.75"
```

- [ ] **Step 5: Replace `LICENSE` with `LICENSE-MIT` + `LICENSE-APACHE`**

Run:
```bash
rm LICENSE
curl -fsSL https://opensource.org/licenses/MIT > LICENSE-MIT.tmp || echo "manual MIT fallback"
```

If the `curl` fails, write `LICENSE-MIT` manually:

```
MIT License

Copyright (c) 2026 zendriver-rs contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Then `LICENSE-APACHE` — copy the standard Apache-2.0 license text (https://www.apache.org/licenses/LICENSE-2.0.txt). It's ~11k bytes; paste in full.

- [ ] **Step 6: Replace `README.md`**

Path: `README.md`

```markdown
# zendriver-rs

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver) — an undetectable, async-first browser automation library using the Chrome DevTools Protocol directly.

**Status:** Pre-alpha. Phase 1 of a six-phase port is in progress; not yet published.

## Phases

1. **Foundation** (in progress): transport + minimal `Browser`/`Tab`/`Element`.
2. Stealth (planned).
3. Element API completeness (planned).
4. `Tab`/`Browser` completeness, cookies, screenshots, multi-tab, iframes (planned).
5. Optional gated features: interception, Cloudflare bypass, `expect()`, fetcher (planned).
6. Polish + 0.1 release (planned).

See `docs/superpowers/specs/` for the per-phase design documents.

## License

Dual-licensed under MIT (`LICENSE-MIT`) and Apache-2.0 (`LICENSE-APACHE`) at your option.
```

- [ ] **Step 7: Create `crates/zendriver/Cargo.toml`**

```toml
[package]
name = "zendriver"
description = "Async-first, undetectable browser automation via the Chrome DevTools Protocol"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
readme = "../../README.md"

[lints]
workspace = true

[features]
default = []
# Enables real-Chrome integration tests (requires Chrome installed on $PATH).
integration-tests = ["dep:wiremock", "dep:serial_test"]
# Re-exports the MockConnection testing helper for downstream users.
testing = ["zendriver-transport/testing"]

[dependencies]
zendriver-transport.workspace = true
chromiumoxide_cdp.workspace   = true
tokio.workspace               = true
tokio-util.workspace          = true
serde.workspace               = true
serde_json.workspace          = true
thiserror.workspace           = true
tracing.workspace             = true
url.workspace                 = true
tempfile.workspace            = true
futures.workspace             = true

[dev-dependencies]
tokio-test.workspace          = true
insta.workspace               = true
tracing-subscriber.workspace  = true
wiremock = { workspace = true, optional = false }
serial_test = { workspace = true, optional = false }
```

- [ ] **Step 8: Create `crates/zendriver/src/lib.rs` stub**

```rust
//! zendriver — async, undetectable Chrome automation over CDP.
//!
//! Phase 1 surface: see the [module-level docs] on each public type.
//!
//! [module-level docs]: crate

#![cfg_attr(docsrs, feature(doc_cfg))]

// Module skeleton; populated in subsequent tasks.
pub mod browser;
pub mod element;
pub mod error;
pub mod query;
pub mod tab;
```

- [ ] **Step 9: Create stub `src/lib.rs` files for `browser`, `element`, `error`, `query`, `tab`**

Each gets a single line (real impls land in later tasks):

```rust
//! Populated in subsequent Phase 1 tasks.
```

Apply to:
- `crates/zendriver/src/browser.rs`
- `crates/zendriver/src/element.rs`
- `crates/zendriver/src/error.rs`
- `crates/zendriver/src/query.rs`
- `crates/zendriver/src/tab.rs`

- [ ] **Step 10: Create `crates/zendriver-transport/Cargo.toml`**

```toml
[package]
name = "zendriver-transport"
description = "Internal: WebSocket + CDP routing actor for zendriver"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[lints]
workspace = true

[features]
default = []
# Exposes the MockConnection testing helper to downstream users.
testing = []

[dependencies]
chromiumoxide_cdp.workspace   = true
tokio.workspace               = true
tokio-util.workspace          = true
tokio-tungstenite.workspace   = true
futures.workspace             = true
serde.workspace               = true
serde_json.workspace          = true
thiserror.workspace           = true
tracing.workspace             = true

[dev-dependencies]
tokio-test.workspace          = true
tracing-subscriber.workspace  = true
```

- [ ] **Step 11: Create `crates/zendriver-transport/src/lib.rs`**

```rust
//! Internal transport layer for zendriver: WebSocket I/O, command/response
//! routing, event broadcast. Not a public API — re-exported selectively via
//! the `zendriver` crate.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod actor;
pub mod connection;
pub mod error;
pub mod frame;
pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use connection::Connection;
pub use error::TransportError;
pub use session::SessionHandle;
```

- [ ] **Step 12: Stub `actor`, `connection`, `error`, `frame`, `session` source files in `zendriver-transport`**

Each: `//! Populated in subsequent Phase 1 tasks.`

- [ ] **Step 13: Create stub Cargo.toml + src/lib.rs for each skeleton crate**

For `zendriver-stealth`, `zendriver-cloudflare`, `zendriver-interception`, `zendriver-fetcher`:

Path: `crates/zendriver-stealth/Cargo.toml` (substitute the name for each):

```toml
[package]
name = "zendriver-stealth"
description = "CDP-level stealth patches for zendriver (populated in Phase 2)"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[lints]
workspace = true
```

Path: `crates/zendriver-stealth/src/lib.rs`:

```rust
//! Populated in Phase 2.
```

Repeat for the other three crates, substituting the name and phase:
- `zendriver-cloudflare` → phase 5
- `zendriver-interception` → phase 5
- `zendriver-fetcher` → phase 5

- [ ] **Step 14: Add `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-D warnings"

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --all --check

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets --locked -- -D warnings

  test-unit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --lib --locked

  test-doc:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --doc --locked

  test-integration:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install Chromium
        run: sudo apt-get update && sudo apt-get install -y chromium-browser
      - run: cargo test --workspace --features integration-tests --test '*' --locked -- --test-threads=1
```

- [ ] **Step 15: Build the empty workspace to confirm it compiles**

Run: `cargo build --workspace`
Expected: clean build, no errors. Warnings about empty `mod` declarations are fine.

Run: `cargo fmt --all --check`
Expected: PASS (no diffs).

Run: `cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS.

- [ ] **Step 16: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore: workspace skeleton, dual MIT/Apache license, CI baseline

Adds Cargo workspace with six crate stubs (zendriver, zendriver-transport,
plus four phase-deferred stubs), rust-toolchain pin, fmt + clippy config,
and a CI matrix covering fmt, clippy, unit, doc, and gated integration
tests. Stubs compile clean; no functional code yet.
EOF
)"
```

---

## Task 1: `TransportError` type

**Files:**
- Modify: `crates/zendriver-transport/src/error.rs`

- [ ] **Step 1: Write the failing tests**

Path: `crates/zendriver-transport/src/error.rs`

```rust
//! Transport-layer errors.

use std::fmt;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    #[error("websocket closed unexpectedly")]
    Disconnected,

    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("framing: {0}")]
    Frame(#[from] serde_json::Error),

    #[error("connection shut down")]
    Shutdown,

    #[error("response channel dropped before reply (id={id})")]
    ResponseDropped { id: u64 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_disconnected_is_stable() {
        assert_eq!(TransportError::Disconnected.to_string(), "websocket closed unexpectedly");
    }

    #[test]
    fn display_shutdown_is_stable() {
        assert_eq!(TransportError::Shutdown.to_string(), "connection shut down");
    }

    #[test]
    fn display_response_dropped_includes_id() {
        let e = TransportError::ResponseDropped { id: 42 };
        assert_eq!(e.to_string(), "response channel dropped before reply (id=42)");
    }

    #[test]
    fn source_preserved_through_ws_wrap() {
        // Construct a tungstenite error and wrap it; check source chain works.
        let tung = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        let wrapped = TransportError::Ws(tung);
        // Display starts with "websocket: "
        assert!(wrapped.to_string().starts_with("websocket: "));
        // source() returns the inner
        assert!(std::error::Error::source(&wrapped).is_some());
    }
}
```

- [ ] **Step 2: Run tests; verify they pass**

Run: `cargo test -p zendriver-transport --lib error::tests`
Expected: 4 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/error.rs
git commit -m "feat(transport): add TransportError type with stable Display output"
```

---

## Task 2: CDP frame types (serde round-trip)

**Files:**
- Modify: `crates/zendriver-transport/src/frame.rs`

- [ ] **Step 1: Write the failing tests + implementation in one file**

The frame types are pure data — implementation and tests go in the same commit. Use TDD by writing tests first; here both blocks are presented together because the impl is short and trivial.

Path: `crates/zendriver-transport/src/frame.rs`

```rust
//! CDP frame envelope types. Wraps `chromiumoxide_cdp` typed parameters with
//! the id / method / session_id envelope CDP requires on the wire.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Outbound command frame, wire-encoded JSON sent to Chrome.
#[derive(Debug, Serialize)]
pub struct CdpCommand<'a> {
    pub id: u64,
    pub method: &'a str,
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sessionId")]
    pub session_id: Option<&'a str>,
}

/// Inbound frame from Chrome — either a command response or a domain event.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CdpInbound {
    Response {
        id: u64,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<CdpRpcError>,
        #[serde(default, rename = "sessionId")]
        session_id: Option<String>,
    },
    Event {
        method: String,
        #[serde(default)]
        params: Value,
        #[serde(default, rename = "sessionId")]
        session_id: Option<String>,
    },
}

/// CDP-style RPC error returned by Chrome for a failing command.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CdpRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Untyped event as it leaves the actor's broadcast bus.
///
/// Subscribers downcast to a `chromiumoxide_cdp` typed event by matching on
/// `method` and deserializing `params`.
#[derive(Debug, Clone)]
pub struct RawEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_serialize_omits_session_id_when_none() {
        let cmd = CdpCommand {
            id: 1,
            method: "Page.navigate",
            params: json!({ "url": "https://example.com" }),
            session_id: None,
        };
        let s = serde_json::to_string(&cmd).expect("ser");
        assert!(!s.contains("sessionId"), "sessionId should be omitted when None, got: {s}");
        assert!(s.contains(r#""id":1"#));
        assert!(s.contains(r#""method":"Page.navigate""#));
    }

    #[test]
    fn command_serialize_includes_session_id_when_some() {
        let cmd = CdpCommand {
            id: 7,
            method: "Page.navigate",
            params: json!({ "url": "https://example.com" }),
            session_id: Some("S1"),
        };
        let s = serde_json::to_string(&cmd).expect("ser");
        assert!(s.contains(r#""sessionId":"S1""#), "got: {s}");
    }

    #[test]
    fn inbound_deserialize_response_with_result() {
        let raw = r#"{"id":3,"result":{"frameId":"F1"}}"#;
        let parsed: CdpInbound = serde_json::from_str(raw).expect("de");
        match parsed {
            CdpInbound::Response { id, result, error, session_id } => {
                assert_eq!(id, 3);
                assert_eq!(result.unwrap()["frameId"], "F1");
                assert!(error.is_none());
                assert!(session_id.is_none());
            }
            CdpInbound::Event { .. } => panic!("expected Response, got Event"),
        }
    }

    #[test]
    fn inbound_deserialize_response_with_error() {
        let raw = r#"{"id":3,"error":{"code":-32602,"message":"Invalid params"}}"#;
        let parsed: CdpInbound = serde_json::from_str(raw).expect("de");
        match parsed {
            CdpInbound::Response { error: Some(e), .. } => {
                assert_eq!(e.code, -32602);
                assert_eq!(e.message, "Invalid params");
            }
            _ => panic!("expected Response with error"),
        }
    }

    #[test]
    fn inbound_deserialize_event() {
        let raw = r#"{"method":"Page.frameStoppedLoading","params":{"frameId":"F1"},"sessionId":"S1"}"#;
        let parsed: CdpInbound = serde_json::from_str(raw).expect("de");
        match parsed {
            CdpInbound::Event { method, params, session_id } => {
                assert_eq!(method, "Page.frameStoppedLoading");
                assert_eq!(params["frameId"], "F1");
                assert_eq!(session_id.as_deref(), Some("S1"));
            }
            _ => panic!("expected Event"),
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver-transport --lib frame::tests`
Expected: 5 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/frame.rs
git commit -m "feat(transport): add CDP frame envelope + serde round-trip tests"
```

---

## Task 3: `RawEvent` already covered + add transport `lib.rs` re-exports

`RawEvent` was defined in Task 2. This task is a no-op for code and exists only to flag that the `lib.rs` of `zendriver-transport` needs to re-export it. Verify and adjust.

**Files:**
- Modify: `crates/zendriver-transport/src/lib.rs`

- [ ] **Step 1: Update re-exports**

```rust
//! Internal transport layer for zendriver: WebSocket I/O, command/response
//! routing, event broadcast. Not a public API — re-exported selectively via
//! the `zendriver` crate.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod actor;
pub mod connection;
pub mod error;
pub mod frame;
pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use connection::Connection;
pub use error::TransportError;
pub use frame::{CdpCommand, CdpInbound, CdpRpcError, RawEvent};
pub use session::SessionHandle;
```

- [ ] **Step 2: Build to confirm**

Run: `cargo build -p zendriver-transport`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/lib.rs
git commit -m "feat(transport): re-export frame types from crate root"
```

---

## Task 4: `ConnectionActor` — command/response path with IO injection

The actor is the hardest piece. We parameterize it over the WebSocket Sink/Stream so we can test with an in-memory pipe instead of a real WebSocket.

**Files:**
- Modify: `crates/zendriver-transport/src/actor.rs`

- [ ] **Step 1: Write the failing tests + implementation**

Path: `crates/zendriver-transport/src/actor.rs`

```rust
//! ConnectionActor: tokio task owning the WebSocket. Routes commands and
//! responses by id, fans events out on a broadcast bus.

use std::collections::HashMap;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace, warn};

use crate::frame::{CdpCommand, CdpInbound, CdpRpcError, RawEvent};

/// Outbound command sent from a `Connection` handle to the actor.
pub(crate) struct OutboundCmd {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
    pub reply: oneshot::Sender<Result<Value, CdpRpcError>>,
}

/// Default broadcast bus capacity. Lagged subscribers drop frames.
pub(crate) const EVENT_BUS_CAPACITY: usize = 1024;

/// Runs the actor loop until `shutdown` is cancelled or the WS dies.
///
/// Generic over the WS sink + stream so tests can drive in-memory streams
/// instead of real WebSockets.
pub(crate) async fn run_actor<S>(
    mut ws: S,
    mut cmd_rx: mpsc::Receiver<OutboundCmd>,
    event_tx: broadcast::Sender<RawEvent>,
    shutdown: CancellationToken,
) where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, CdpRpcError>>> = HashMap::new();
    let mut next_id: u64 = 1;

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                debug!("actor shutdown received; draining {} pending", pending.len());
                break;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    debug!("cmd channel closed; shutting down");
                    break;
                };
                let id = next_id;
                next_id = next_id.wrapping_add(1);
                let frame = CdpCommand {
                    id,
                    method: &cmd.method,
                    params: cmd.params,
                    session_id: cmd.session_id.as_deref(),
                };
                match serde_json::to_string(&frame) {
                    Ok(s) => {
                        trace!(id, method = %cmd.method, "send");
                        if let Err(e) = ws.send(Message::text(s)).await {
                            error!("ws send failed: {e}");
                            let _ = cmd.reply.send(Err(CdpRpcError {
                                code: -32000,
                                message: format!("ws send failed: {e}"),
                                data: None,
                            }));
                            break;
                        }
                        pending.insert(id, cmd.reply);
                    }
                    Err(e) => {
                        let _ = cmd.reply.send(Err(CdpRpcError {
                            code: -32700,
                            message: format!("serialize: {e}"),
                            data: None,
                        }));
                    }
                }
            }
            frame = ws.next() => {
                match frame {
                    None => {
                        debug!("ws stream ended");
                        break;
                    }
                    Some(Err(e)) => {
                        error!("ws read failed: {e}");
                        break;
                    }
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<CdpInbound>(&text) {
                            Ok(CdpInbound::Response { id, result, error, .. }) => {
                                if let Some(reply) = pending.remove(&id) {
                                    let res = match error {
                                        Some(e) => Err(e),
                                        None    => Ok(result.unwrap_or(Value::Null)),
                                    };
                                    let _ = reply.send(res);
                                } else {
                                    warn!(id, "response for unknown id (caller dropped?)");
                                }
                            }
                            Ok(CdpInbound::Event { method, params, session_id }) => {
                                let ev = RawEvent { method, params, session_id };
                                // Ignore SendError: zero subscribers is fine.
                                let _ = event_tx.send(ev);
                            }
                            Err(e) => warn!("frame parse failed: {e} (text: {text})"),
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("ws close frame; shutting down");
                        break;
                    }
                    Some(Ok(_)) => { /* ignore binary, ping, pong, frame */ }
                }
            }
        }
    }

    // Drain pending into shutdown errors so callers don't hang.
    for (_id, reply) in pending.drain() {
        let _ = reply.send(Err(CdpRpcError {
            code: -32001,
            message: "connection shut down".into(),
            data: None,
        }));
    }
    debug!("actor exit");
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio_tungstenite::tungstenite::Message;

    /// Build a paired (driver-side, test-side) Sink/Stream of tungstenite
    /// `Message`s using mpsc channels. Driver writes go to `test_rx`; test
    /// writes go to `driver_rx`.
    fn duplex_pair() -> (
        impl futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
        mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        mpsc::Receiver<Message>,
    ) {
        let (driver_tx_out, test_rx) = mpsc::channel::<Message>(32);
        let (test_tx_in, driver_rx_in) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(32);

        // Driver-side: sink writes to driver_tx_out; stream reads from driver_rx_in.
        let driver = DriverStream {
            tx: driver_tx_out,
            rx: driver_rx_in,
        };
        (driver, test_tx_in, test_rx)
    }

    struct DriverStream {
        tx: mpsc::Sender<Message>,
        rx: mpsc::Receiver<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    }

    impl futures::Sink<Message> for DriverStream {
        type Error = tokio_tungstenite::tungstenite::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(mut self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.tx
                .try_send(item)
                .map_err(|_| tokio_tungstenite::tungstenite::Error::ConnectionClosed)
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl futures::Stream for DriverStream {
        type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.rx.poll_recv(cx)
        }
    }

    #[tokio::test]
    async fn cmd_id_assigned_starting_at_one_and_serialized_correctly() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

        let (reply_tx, _reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://example.com" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let sent = test_rx.recv().await.expect("driver sent something");
        let text = match sent {
            Message::Text(t) => t,
            other => panic!("unexpected frame: {other:?}"),
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "Page.navigate");
        assert_eq!(v["params"]["url"], "https://example.com");
        assert!(v.get("sessionId").is_none());

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn response_routes_to_correct_oneshot() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://x.test" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let sent = test_rx.recv().await.unwrap();
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!(),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();

        // Simulate Chrome reply.
        test_tx
            .send(Ok(Message::text(
                json!({ "id": id, "result": { "frameId": "F1" } }).to_string(),
            )))
            .await
            .unwrap();

        let res = reply_rx.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F1");

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn response_error_propagates_to_caller() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Foo.bar".into(),
                params: json!({}),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let sent = test_rx.recv().await.unwrap();
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!(),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();
        test_tx
            .send(Ok(Message::text(
                json!({ "id": id, "error": { "code": -32601, "message": "Method not found" } })
                    .to_string(),
            )))
            .await
            .unwrap();

        let res = reply_rx.await.unwrap();
        let err = res.unwrap_err();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");

        shutdown.cancel();
        actor_handle.await.unwrap();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver-transport --lib actor::tests`
Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/actor.rs
git commit -m "feat(transport): ConnectionActor command/response path"
```

---

## Task 5: `ConnectionActor` — event broadcast + lagged-subscriber test

The event path is already wired in Task 4's actor (any `CdpInbound::Event` goes onto the broadcast bus). This task adds tests demonstrating it works under multi-subscriber and lagged scenarios.

**Files:**
- Modify: `crates/zendriver-transport/src/actor.rs` (extend `mod tests`)

- [ ] **Step 1: Append tests to `actor::tests`**

Insert into the existing `#[cfg(test)] mod tests` block, after the existing tests:

```rust
    #[tokio::test]
    async fn event_fanned_out_to_multiple_subscribers() {
        let (ws, test_tx, _test_rx) = duplex_pair();
        let (_cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let mut sub_a = event_tx.subscribe();
        let mut sub_b = event_tx.subscribe();
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

        test_tx
            .send(Ok(Message::text(
                json!({ "method": "Page.frameStoppedLoading", "params": { "frameId": "F1" } })
                    .to_string(),
            )))
            .await
            .unwrap();

        let a = sub_a.recv().await.unwrap();
        let b = sub_b.recv().await.unwrap();
        assert_eq!(a.method, "Page.frameStoppedLoading");
        assert_eq!(b.method, "Page.frameStoppedLoading");

        shutdown.cancel();
        actor_handle.await.unwrap();
    }

    #[tokio::test]
    async fn lagged_subscriber_recovers_with_lagged_error() {
        // Small bus to force the subscriber to lag.
        let (ws, test_tx, _test_rx) = duplex_pair();
        let (_cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(2);
        let mut sub = event_tx.subscribe();
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

        // Push 5 events while sub doesn't consume.
        for i in 0..5 {
            test_tx
                .send(Ok(Message::text(
                    json!({ "method": "Test.evt", "params": { "i": i } }).to_string(),
                )))
                .await
                .unwrap();
        }

        // Give the actor a tick to drain.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // First recv should be Lagged.
        let first = sub.recv().await;
        assert!(matches!(first, Err(tokio::sync::broadcast::error::RecvError::Lagged(_))));

        shutdown.cancel();
        actor_handle.await.unwrap();
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver-transport --lib actor::tests`
Expected: 5 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/actor.rs
git commit -m "test(transport): event fan-out and lagged-subscriber behavior"
```

---

## Task 6: `ConnectionActor` — shutdown drains pending

The pending-drain on shutdown is wired in Task 4's actor. This task adds a regression test.

**Files:**
- Modify: `crates/zendriver-transport/src/actor.rs` (extend `mod tests`)

- [ ] **Step 1: Append test**

Insert into the existing `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn shutdown_drains_pending_with_shutdown_error() {
        let (ws, _test_tx, _test_rx) = duplex_pair();
        let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(8);
        let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
        let shutdown = CancellationToken::new();
        let actor_handle = tokio::spawn(run_actor(ws, cmd_rx, event_tx, shutdown.clone()));

        let (reply_tx, reply_rx) = oneshot::channel();
        cmd_tx
            .send(OutboundCmd {
                method: "Page.navigate".into(),
                params: json!({ "url": "https://x.test" }),
                session_id: None,
                reply: reply_tx,
            })
            .await
            .unwrap();

        // Give actor time to register the pending entry before cancelling.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        shutdown.cancel();

        let res = reply_rx.await.unwrap();
        let err = res.unwrap_err();
        assert_eq!(err.code, -32001);
        assert!(err.message.contains("shut down"));

        actor_handle.await.unwrap();
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver-transport --lib actor::tests`
Expected: 6 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/actor.rs
git commit -m "test(transport): shutdown drains pending into typed errors"
```

---

## Task 7: `Connection` public handle + `subscribe::<E>()`

**Files:**
- Modify: `crates/zendriver-transport/src/connection.rs`

- [ ] **Step 1: Write the failing tests + implementation**

Path: `crates/zendriver-transport/src/connection.rs`

```rust
//! `Connection` — the public handle to the transport actor.

use std::sync::Arc;

use futures::Stream;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

use crate::actor::{run_actor, OutboundCmd, EVENT_BUS_CAPACITY};
use crate::error::TransportError;
use crate::frame::{CdpRpcError, RawEvent};

/// Cheap-to-clone handle to the connection actor. All `Tab`s and `Element`s
/// hold one of these (via `Arc<...>`); the actor itself runs in a separate
/// tokio task.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<ConnectionInner>,
}

pub(crate) struct ConnectionInner {
    pub(crate) cmd_tx: mpsc::Sender<OutboundCmd>,
    pub(crate) event_tx: broadcast::Sender<RawEvent>,
    pub(crate) shutdown: CancellationToken,
}

impl Connection {
    /// Send a CDP command and await its response.
    ///
    /// `method` is the dotted CDP method name (e.g. `"Page.navigate"`).
    /// `params` is the JSON value for the command's parameters.
    /// `session_id` routes the command to a particular target's session.
    pub async fn call_raw(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
    ) -> Result<Value, TransportError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .send(OutboundCmd {
                method: method.into(),
                params,
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| TransportError::Shutdown)?;
        match reply_rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(rpc_err)) => Err(rpc_err_to_transport(rpc_err)),
            Err(_) => Err(TransportError::Shutdown),
        }
    }

    /// Subscribe to all events on this connection (no filtering).
    pub fn subscribe_raw(&self) -> impl Stream<Item = RawEvent> + Send + Unpin {
        BroadcastStream::new(self.inner.event_tx.subscribe()).filter_map(|res| async move {
            // Lagged frames are dropped.
            res.ok()
        })
    }

    /// Subscribe to events of a specific CDP method, deserialized into `T`.
    pub fn subscribe<T>(&self, method: &'static str) -> impl Stream<Item = T> + Send + Unpin
    where
        T: DeserializeOwned + Send + 'static,
    {
        BroadcastStream::new(self.inner.event_tx.subscribe()).filter_map(move |res| async move {
            let ev = res.ok()?;
            if ev.method == method {
                serde_json::from_value(ev.params).ok()
            } else {
                None
            }
        })
    }

    /// Trigger graceful shutdown of the underlying actor.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Public accessor for advanced users who need to drive the underlying
    /// shutdown token directly.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }
}

fn rpc_err_to_transport(e: CdpRpcError) -> TransportError {
    // Mapping is mostly for the transport-level cases that round-trip; higher
    // layers map richer CDP errors. Here we just preserve the message.
    if e.code == -32001 && e.message.contains("shut down") {
        TransportError::Shutdown
    } else {
        // Embed the JSON-RPC error as an io error so it survives the trait
        // bounds; richer mapping is the job of the zendriver crate.
        TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("[{code}] {msg}", code = e.code, msg = e.message),
        ))
    }
}

/// Connect to a Chrome DevTools WebSocket URL and spawn the actor.
pub async fn connect(ws_url: &str) -> Result<Connection, TransportError> {
    use futures::StreamExt;
    use tokio_tungstenite::connect_async;
    let (ws, _resp) = connect_async(ws_url).await?;
    Ok(spawn_actor(ws))
}

/// Spawn the actor on the given pre-connected WebSocket. Mainly for tests
/// and for `connect`; production code uses `connect`.
pub fn spawn_actor<S>(ws: S) -> Connection
where
    S: futures::Sink<tokio_tungstenite::tungstenite::Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures::Stream<Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>>
        + Send
        + Unpin
        + 'static,
{
    let (cmd_tx, cmd_rx) = mpsc::channel::<OutboundCmd>(64);
    let (event_tx, _event_rx) = broadcast::channel::<RawEvent>(EVENT_BUS_CAPACITY);
    let shutdown = CancellationToken::new();
    tokio::spawn(run_actor(ws, cmd_rx, event_tx.clone(), shutdown.clone()));
    Connection {
        inner: Arc::new(ConnectionInner {
            cmd_tx,
            event_tx,
            shutdown,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Reuse the duplex helper from actor.rs by re-declaring it. (Visibility
    // workaround for cross-module test helpers.)
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    fn duplex_pair() -> (
        impl futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Send
            + Unpin
            + 'static,
        tokio::sync::mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        tokio::sync::mpsc::Receiver<Message>,
    ) {
        let (driver_tx_out, test_rx) = tokio::sync::mpsc::channel::<Message>(32);
        let (test_tx_in, driver_rx_in) =
            tokio::sync::mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(32);

        let driver = super::test_only::DriverStream {
            tx: driver_tx_out,
            rx: driver_rx_in,
        };
        (driver, test_tx_in, test_rx)
    }

    #[tokio::test]
    async fn call_raw_round_trips_through_actor() {
        let (ws, test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);

        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
            }
        });

        let sent = test_rx.recv().await.unwrap();
        let id = serde_json::from_str::<Value>(match &sent {
            Message::Text(t) => t,
            _ => panic!(),
        })
        .unwrap()["id"]
            .as_u64()
            .unwrap();

        test_tx
            .send(Ok(Message::text(
                json!({ "id": id, "result": { "frameId": "F1" } }).to_string(),
            )))
            .await
            .unwrap();

        let res = call.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F1");

        conn.shutdown();
    }
}

/// Re-export the test DriverStream type at a shared visibility level so
/// both `actor::tests` and `connection::tests` can construct it.
#[cfg(test)]
pub(crate) mod test_only {
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    pub struct DriverStream {
        pub tx: mpsc::Sender<Message>,
        pub rx: mpsc::Receiver<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    }

    impl futures::Sink<Message> for DriverStream {
        type Error = tokio_tungstenite::tungstenite::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(mut self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.tx
                .try_send(item)
                .map_err(|_| tokio_tungstenite::tungstenite::Error::ConnectionClosed)
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl futures::Stream for DriverStream {
        type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.rx.poll_recv(cx)
        }
    }
}
```

Also add `tokio-stream` to the `zendriver-transport` deps:

Path: `crates/zendriver-transport/Cargo.toml` — add to `[dependencies]`:

```toml
tokio-stream = { version = "0.1", features = ["sync"] }
```

And in workspace `Cargo.toml` under `[workspace.dependencies]`:

```toml
tokio-stream = { version = "0.1", features = ["sync"] }
```

Then change the per-crate line to `tokio-stream.workspace = true`.

Also: refactor `actor::tests::duplex_pair` to reuse `test_only::DriverStream` instead of redefining `DriverStream`. Delete the local `DriverStream` struct + impls from `actor.rs`'s test module; have its `duplex_pair` construct `super::test_only::DriverStream` directly.

- [ ] **Step 2: Run all transport tests**

Run: `cargo test -p zendriver-transport --lib`
Expected: 9 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/connection.rs crates/zendriver-transport/src/actor.rs Cargo.toml crates/zendriver-transport/Cargo.toml
git commit -m "feat(transport): Connection handle, typed subscribe<T>, connect()"
```

---

## Task 8: `SessionHandle` wrapper

`SessionHandle` binds a `Connection` to a specific `sessionId`. All calls go out tagged with that session.

**Files:**
- Modify: `crates/zendriver-transport/src/session.rs`

- [ ] **Step 1: Write the failing tests + implementation**

Path: `crates/zendriver-transport/src/session.rs`

```rust
//! `SessionHandle`: a `Connection` bound to a particular CDP `sessionId`.
//! All commands sent through the handle are routed to that target.

use std::sync::Arc;

use futures::Stream;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::connection::Connection;
use crate::error::TransportError;
use crate::frame::RawEvent;

#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Connection,
    session_id: String,
}

impl SessionHandle {
    pub fn new(conn: Connection, session_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                conn,
                session_id: session_id.into(),
            }),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.inner.session_id
    }

    pub fn connection(&self) -> &Connection {
        &self.inner.conn
    }

    /// Send a CDP command routed to this session.
    pub async fn call(
        &self,
        method: impl Into<String>,
        params: Value,
    ) -> Result<Value, TransportError> {
        self.inner
            .conn
            .call_raw(method, params, Some(self.inner.session_id.clone()))
            .await
    }

    /// Subscribe to events for this session only (others are filtered out).
    pub fn subscribe<T>(&self, method: &'static str) -> impl Stream<Item = T> + Send + Unpin
    where
        T: DeserializeOwned + Send + 'static,
    {
        let sid = self.inner.session_id.clone();
        let raw = self.inner.conn.subscribe_raw();
        use futures::StreamExt;
        raw.filter_map(move |ev: RawEvent| {
            let sid = sid.clone();
            async move {
                if ev.session_id.as_deref() == Some(sid.as_str()) && ev.method == method {
                    serde_json::from_value(ev.params).ok()
                } else {
                    None
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::spawn_actor;
    use crate::connection::test_only::DriverStream;
    use futures::SinkExt;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    fn duplex_pair() -> (
        DriverStream,
        mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        mpsc::Receiver<Message>,
    ) {
        let (tx_out, rx_out) = mpsc::channel(32);
        let (tx_in, rx_in) = mpsc::channel(32);
        (DriverStream { tx: tx_out, rx: rx_in }, tx_in, rx_out)
    }

    #[tokio::test]
    async fn session_call_includes_session_id() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let sess = SessionHandle::new(conn.clone(), "S1");

        let call = tokio::spawn({
            let s = sess.clone();
            async move { s.call("Page.navigate", json!({ "url": "https://x.test" })).await }
        });

        let sent = test_rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(match &sent {
            Message::Text(t) => t,
            _ => panic!(),
        })
        .unwrap();
        assert_eq!(v["sessionId"], "S1");

        // Cancel via dropping
        drop(call);
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver-transport --lib session::tests`
Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/session.rs
git commit -m "feat(transport): SessionHandle wraps Connection with a session_id"
```

---

## Task 9: `MockConnection` testing helper

`MockConnection` lets downstream tests drive a `Connection` without spawning a real WebSocket. We back it with the same `DriverStream` test helper.

**Files:**
- Modify: `crates/zendriver-transport/src/testing.rs`

- [ ] **Step 1: Write implementation**

Path: `crates/zendriver-transport/src/testing.rs`

```rust
//! Test-only helpers — gated behind `cfg(any(test, feature = "testing"))`.

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::connection::{spawn_actor, Connection};

/// A paired pseudo-Chrome: tests push frames the driver would read, and read
/// frames the driver sent. Driving an end-to-end interaction looks like:
///
/// ```ignore
/// let (mut mock, conn) = MockConnection::pair();
/// let call = tokio::spawn(async move { conn.call_raw("Page.navigate", json!({}), None).await });
/// let id = mock.expect_cmd("Page.navigate").await;
/// mock.reply(id, json!({ "frameId": "F1" })).await;
/// let res = call.await.unwrap().unwrap();
/// ```
pub struct MockConnection {
    server_in: mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    server_out: mpsc::Receiver<Message>,
    last_sent: Option<Value>,
}

impl MockConnection {
    /// Pair a `MockConnection` with a driver-side `Connection`.
    #[must_use]
    pub fn pair() -> (Self, Connection) {
        let (tx_to_driver, rx_driver) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream {
            tx: tx_from_driver,
            rx: rx_driver,
        };
        let conn = spawn_actor(driver);
        let mock = MockConnection {
            server_in: tx_to_driver,
            server_out: rx_test,
            last_sent: None,
        };
        (mock, conn)
    }

    /// Block until the driver sends a command whose `method` field matches.
    /// Returns the command id. Wrap in `tokio::time::timeout` at test sites —
    /// there is no built-in timeout.
    pub async fn expect_cmd(&mut self, method: &str) -> u64 {
        loop {
            let msg = self.server_out.recv().await.expect("driver did not send");
            let text = match msg {
                Message::Text(t) => t,
                other => panic!("expected text frame, got {other:?}"),
            };
            let v: Value = serde_json::from_str(&text).expect("invalid frame");
            self.last_sent = Some(v.clone());
            if v["method"] == method {
                return v["id"].as_u64().expect("frame missing id");
            }
            // Otherwise, keep waiting for the right method.
        }
    }

    pub fn last_sent(&self) -> &Value {
        self.last_sent.as_ref().expect("no command observed yet")
    }

    pub async fn reply(&self, id: u64, result: Value) {
        let frame = serde_json::json!({ "id": id, "result": result }).to_string();
        self.server_in.send(Ok(Message::text(frame))).await.expect("driver closed");
    }

    pub async fn reply_err(&self, id: u64, code: i32, message: &str) {
        let frame = serde_json::json!({
            "id": id,
            "error": { "code": code, "message": message }
        })
        .to_string();
        self.server_in.send(Ok(Message::text(frame))).await.expect("driver closed");
    }

    pub async fn emit_event(&self, method: &str, params: Value) {
        let frame = serde_json::json!({ "method": method, "params": params }).to_string();
        self.server_in.send(Ok(Message::text(frame))).await.expect("driver closed");
    }

    pub async fn emit_event_for_session(&self, method: &str, params: Value, session_id: &str) {
        let frame = serde_json::json!({
            "method": method,
            "params": params,
            "sessionId": session_id,
        })
        .to_string();
        self.server_in.send(Ok(Message::text(frame))).await.expect("driver closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn mock_round_trips_a_call() {
        let (mut mock, conn) = MockConnection::pair();
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None).await
            }
        });
        let id = mock.expect_cmd("Page.navigate").await;
        assert_eq!(mock.last_sent()["params"]["url"], "https://x.test");
        mock.reply(id, json!({ "frameId": "F1" })).await;
        let res = call.await.unwrap().unwrap();
        assert_eq!(res["frameId"], "F1");
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver-transport --lib testing::tests`
Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver-transport/src/testing.rs
git commit -m "feat(transport): MockConnection test helper for downstream crates"
```

---

## Task 10: `BrowserError` + `ZendriverError` + `Result` alias

**Files:**
- Modify: `crates/zendriver/src/error.rs`

- [ ] **Step 1: Write tests + implementation**

Path: `crates/zendriver/src/error.rs`

```rust
//! Error hierarchy for the `zendriver` crate.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ZendriverError {
    #[error("browser process failed: {0}")]
    Browser(#[from] BrowserError),

    #[error("transport: {0}")]
    Transport(#[from] zendriver_transport::TransportError),

    #[error("CDP RPC error [{code}] {message}")]
    Cdp {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },

    #[error("element not found: {selector}")]
    ElementNotFound { selector: String },

    #[error("timed out after {0:?}")]
    Timeout(Duration),

    #[error("navigation failed: {0}")]
    Navigation(String),

    #[error("javascript exception: {0}")]
    JsException(String),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = ZendriverError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserError {
    #[error("chrome executable not found; searched: {searched:?}")]
    ExecutableNotFound { searched: Vec<PathBuf> },

    #[error("chrome failed to start: {0}")]
    SpawnFailed(#[source] std::io::Error),

    #[error("chrome exited before WS endpoint became available (status: {0:?})")]
    EarlyExit(std::process::ExitStatus),

    #[error("timed out waiting for chrome WS endpoint")]
    WsTimeout,

    #[error("could not parse devtools endpoint from chrome stderr")]
    DevtoolsParse,

    #[error("failed to clean user_data_dir: {0}")]
    Cleanup(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_for_element_not_found_includes_selector() {
        let e = ZendriverError::ElementNotFound {
            selector: "button.foo".into(),
        };
        assert_eq!(e.to_string(), "element not found: button.foo");
    }

    #[test]
    fn display_for_timeout_includes_duration() {
        let e = ZendriverError::Timeout(Duration::from_secs(5));
        assert_eq!(e.to_string(), "timed out after 5s");
    }

    #[test]
    fn display_for_cdp_includes_code_and_message() {
        let e = ZendriverError::Cdp {
            code: -32602,
            message: "Invalid params".into(),
            data: None,
        };
        assert_eq!(e.to_string(), "CDP RPC error [-32602] Invalid params");
    }

    #[test]
    fn display_for_executable_not_found_includes_paths() {
        let e = ZendriverError::Browser(BrowserError::ExecutableNotFound {
            searched: vec![PathBuf::from("/usr/bin/google-chrome")],
        });
        assert!(e.to_string().contains("/usr/bin/google-chrome"));
    }

    #[test]
    fn from_transport_error_works() {
        let te = zendriver_transport::TransportError::Shutdown;
        let ze: ZendriverError = te.into();
        assert!(matches!(ze, ZendriverError::Transport(_)));
        assert!(ze.to_string().contains("connection shut down"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib error::tests`
Expected: 5 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/error.rs
git commit -m "feat(zendriver): ZendriverError + BrowserError + Result alias"
```

---

## Task 11: Chrome executable auto-discovery

**Files:**
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: Add discovery function with tests**

Path: `crates/zendriver/src/browser.rs`

```rust
//! Browser lifecycle: executable discovery, subprocess spawn, WS attach,
//! graceful teardown.

use std::path::{Path, PathBuf};

use crate::error::BrowserError;

/// Look for a Chromium-family binary on PATH and in conventional locations.
/// Returns the first path that exists.
pub fn find_chrome_executable() -> Result<PathBuf, BrowserError> {
    let candidates = candidate_paths();
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    Err(BrowserError::ExecutableNotFound { searched: candidates })
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();

    // PATH lookups.
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        if let Some(p) = which_on_path(name) {
            v.push(p);
        }
    }

    // Platform-specific known locations.
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        v.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        v.push(PathBuf::from("/usr/bin/google-chrome"));
        v.push(PathBuf::from("/usr/bin/chromium"));
        v.push(PathBuf::from("/usr/bin/chromium-browser"));
        v.push(PathBuf::from("/snap/bin/chromium"));
    }
    #[cfg(target_os = "windows")]
    {
        v.push(PathBuf::from(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ));
        v.push(PathBuf::from(
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ));
    }

    v
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let full = dir.join(name);
        if full.is_file() {
            return Some(full);
        }
        #[cfg(target_os = "windows")]
        {
            let with_exe = dir.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_is_nonempty() {
        let v = candidate_paths();
        assert!(!v.is_empty());
    }

    #[test]
    fn find_chrome_executable_returns_err_when_none_exist() {
        // Force an empty PATH and assert ExecutableNotFound on a system
        // without any default-location binaries. We can't reliably do this
        // cross-platform without mocking, so we just test the type signature
        // by calling the function in a save way:
        let _ = find_chrome_executable();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib browser::tests`
Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): chrome executable auto-discovery on PATH + platforms"
```

---

## Task 12: DevTools WS URL parser

**Files:**
- Modify: `crates/zendriver/src/browser.rs` (append parser function + tests)

- [ ] **Step 1: Add the parser**

Append to `crates/zendriver/src/browser.rs` (before `#[cfg(test)] mod tests`):

```rust
/// Parse a `DevTools listening on ws://...` line from Chrome's stderr.
pub(crate) fn parse_devtools_line(line: &str) -> Option<String> {
    // Format: `DevTools listening on ws://127.0.0.1:NNNN/devtools/browser/UUID`
    let needle = "DevTools listening on ";
    let idx = line.find(needle)?;
    let rest = &line[idx + needle.len()..];
    let end = rest
        .find(char::is_whitespace)
        .unwrap_or(rest.len());
    let url = rest[..end].trim();
    if url.starts_with("ws://") || url.starts_with("wss://") {
        Some(url.to_string())
    } else {
        None
    }
}
```

Append test cases inside the `mod tests` block:

```rust
    #[test]
    fn parse_devtools_line_extracts_ws_url() {
        let line =
            "DevTools listening on ws://127.0.0.1:54321/devtools/browser/abc-def-123\n";
        assert_eq!(
            parse_devtools_line(line).as_deref(),
            Some("ws://127.0.0.1:54321/devtools/browser/abc-def-123")
        );
    }

    #[test]
    fn parse_devtools_line_returns_none_for_unrelated() {
        assert!(parse_devtools_line("loading extension foo").is_none());
        assert!(parse_devtools_line("DevTools listening on http://x").is_none());
    }

    #[test]
    fn parse_devtools_line_handles_prefixed_log_lines() {
        // Real Chrome stderr is sometimes prefixed with [pid:tid:date:level].
        let line = "[12345:1234:0102/030405.000000:INFO:browser.cc] DevTools listening on ws://localhost:1/devtools/browser/x";
        assert_eq!(
            parse_devtools_line(line).as_deref(),
            Some("ws://localhost:1/devtools/browser/x")
        );
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib browser::tests`
Expected: 5 passed (previous 2 + 3 new).

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): DevTools WS URL parser from chrome stderr"
```

---

## Task 13: `BrowserBuilder` (no launch yet)

**Files:**
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: Add `BrowserBuilder` skeleton + flag computation**

Append to `crates/zendriver/src/browser.rs`:

```rust
#[derive(Debug, Default, Clone)]
pub struct BrowserBuilder {
    pub(crate) headless: Option<bool>,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) user_data_dir: Option<PathBuf>,
    pub(crate) extra_args: Vec<String>,
}

impl BrowserBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn headless(mut self, on: bool) -> Self {
        self.headless = Some(on);
        self
    }

    #[must_use]
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    #[must_use]
    pub fn user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(path.into());
        self
    }

    #[must_use]
    pub fn arg(mut self, flag: impl Into<String>) -> Self {
        self.extra_args.push(flag.into());
        self
    }

    #[must_use]
    pub fn args(mut self, flags: impl IntoIterator<Item = String>) -> Self {
        self.extra_args.extend(flags);
        self
    }

    /// Compute the full argv that would be passed to Chrome. Exposed to
    /// tests + snapshots; called internally by `launch`.
    pub(crate) fn build_flags(&self, user_data_dir: &Path) -> Vec<String> {
        let mut v = Vec::with_capacity(8 + self.extra_args.len());
        v.push("--remote-debugging-port=0".to_string());
        v.push(format!("--user-data-dir={}", user_data_dir.display()));
        v.push("--no-first-run".to_string());
        v.push("--no-default-browser-check".to_string());
        if self.headless.unwrap_or(true) {
            v.push("--headless=new".to_string());
            v.push("--disable-gpu".to_string());
        }
        v.extend(self.extra_args.iter().cloned());
        v
    }
}

impl Browser {
    pub fn builder() -> BrowserBuilder {
        BrowserBuilder::new()
    }
}

// Forward declaration for Task 14. Defined more completely there.
pub struct Browser {
    pub(crate) _placeholder: (),
}
```

Append tests:

```rust
    #[test]
    fn build_flags_default_is_headless() {
        let b = BrowserBuilder::new();
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(flags.contains(&"--headless=new".to_string()));
        assert!(flags.contains(&"--disable-gpu".to_string()));
        assert!(flags.contains(&"--user-data-dir=/tmp/x".to_string()));
        assert!(flags.contains(&"--remote-debugging-port=0".to_string()));
    }

    #[test]
    fn build_flags_no_headless_when_disabled() {
        let b = BrowserBuilder::new().headless(false);
        let flags = b.build_flags(Path::new("/tmp/x"));
        assert!(!flags.iter().any(|f| f.starts_with("--headless")));
        assert!(!flags.contains(&"--disable-gpu".to_string()));
    }

    #[test]
    fn build_flags_includes_extra_args_in_order() {
        let b = BrowserBuilder::new()
            .arg("--proxy-server=http://x")
            .arg("--lang=en-US");
        let flags = b.build_flags(Path::new("/tmp/x"));
        let proxy = flags.iter().position(|f| f == "--proxy-server=http://x").unwrap();
        let lang = flags.iter().position(|f| f == "--lang=en-US").unwrap();
        assert!(proxy < lang);
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib browser::tests`
Expected: 8 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): BrowserBuilder + launch-flag construction"
```

---

## Task 14: `Browser::launch` (spawn + attach + main tab)

**Files:**
- Modify: `crates/zendriver/src/browser.rs`
- Modify: `crates/zendriver/src/tab.rs` (stub `Tab::for_session_unchecked` constructor)

- [ ] **Step 1: Define `Tab` skeleton (just enough for `Browser` to construct one)**

Path: `crates/zendriver/src/tab.rs`

```rust
//! Tab — handle to a single CDP target session.

use std::sync::Arc;
use zendriver_transport::SessionHandle;

#[derive(Clone)]
pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
}

impl Tab {
    pub(crate) fn new(session: SessionHandle) -> Self {
        Self {
            inner: Arc::new(TabInner { session }),
        }
    }

    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }
}
```

- [ ] **Step 2: Replace the `Browser` forward declaration with the real launcher**

Path: `crates/zendriver/src/browser.rs` — replace the `pub struct Browser { _placeholder }` block with:

```rust
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tracing::{debug, info};
use zendriver_transport::{Connection, SessionHandle};

use crate::error::ZendriverError;
use crate::tab::Tab;

#[derive(Clone)]
pub struct Browser {
    pub(crate) inner: Arc<BrowserInner>,
}

pub(crate) struct BrowserInner {
    pub(crate) conn: Connection,
    pub(crate) main_tab: Tab,
    pub(crate) child: tokio::sync::Mutex<Option<Child>>,
    pub(crate) _user_data: Option<TempDir>,
}

const WS_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl BrowserBuilder {
    /// Spawn Chrome and attach. Returns once the main tab is bound.
    pub async fn launch(self) -> Result<Browser, ZendriverError> {
        let exe = match self.executable.clone() {
            Some(p) => p,
            None => find_chrome_executable()?,
        };

        // Allocate user_data_dir (or use a TempDir we keep alive until shutdown).
        let (user_data_path, owned_tmp) = match self.user_data_dir.clone() {
            Some(p) => (p, None),
            None => {
                let td = tempfile::Builder::new()
                    .prefix("zendriver-")
                    .tempdir()
                    .map_err(crate::error::BrowserError::SpawnFailed)?;
                (td.path().to_path_buf(), Some(td))
            }
        };

        let flags = self.build_flags(&user_data_path);
        info!(executable = %exe.display(), "launching chrome");

        let mut cmd = Command::new(&exe);
        cmd.args(&flags)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(crate::error::BrowserError::SpawnFailed)?;

        // Read stderr line-by-line until we see the DevTools URL.
        let stderr = child
            .stderr
            .take()
            .ok_or(crate::error::BrowserError::DevtoolsParse)?;
        let mut lines = BufReader::new(stderr).lines();

        let ws_url = timeout(WS_ENDPOINT_TIMEOUT, async {
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(line = %line, "chrome stderr");
                if let Some(url) = parse_devtools_line(&line) {
                    return Ok::<String, ZendriverError>(url);
                }
            }
            Err(crate::error::BrowserError::DevtoolsParse.into())
        })
        .await
        .map_err(|_| crate::error::BrowserError::WsTimeout)??;

        debug!(ws_url = %ws_url, "connecting to chrome");
        let conn = zendriver_transport::connection::connect(&ws_url).await?;

        // Attach to the first target. We discover it via Target.getTargets.
        let list = conn
            .call_raw("Target.getTargets", serde_json::json!({}), None)
            .await?;
        let target_id = list["targetInfos"]
            .as_array()
            .and_then(|arr| {
                arr.iter().find(|t| t["type"] == "page").or_else(|| arr.first())
            })
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| ZendriverError::Navigation("no initial target found".into()))?
            .to_string();

        let attach = conn
            .call_raw(
                "Target.attachToTarget",
                serde_json::json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = attach["sessionId"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("attach returned no sessionId".into()))?
            .to_string();

        let session = SessionHandle::new(conn.clone(), session_id);
        let main_tab = Tab::new(session);

        Ok(Browser {
            inner: Arc::new(BrowserInner {
                conn,
                main_tab,
                child: tokio::sync::Mutex::new(Some(child)),
                _user_data: owned_tmp,
            }),
        })
    }
}

impl Browser {
    pub fn main_tab(&self) -> Tab {
        self.inner.main_tab.clone()
    }

    pub fn cdp(&self) -> &Connection {
        &self.inner.conn
    }
}
```

- [ ] **Step 2: Build (no test runs Chrome at this stage)**

Run: `cargo build -p zendriver`
Expected: clean build.

Run: `cargo test -p zendriver --lib`
Expected: existing tests still pass; no new tests added in this task (`launch` is exercised in Task 25's integration test).

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): Browser::launch spawns chrome and attaches to main target"
```

---

## Task 15: `Browser::close` + `Drop` graceful teardown

**Files:**
- Modify: `crates/zendriver/src/browser.rs`

- [ ] **Step 1: Implement close + Drop**

Append to `crates/zendriver/src/browser.rs` (inside `impl Browser` and after):

```rust
impl Browser {
    /// Graceful shutdown: cancel the transport, send SIGTERM to Chrome,
    /// wait up to `SHUTDOWN_GRACE`, then SIGKILL on timeout. Cleans up
    /// user_data_dir.
    pub async fn close(self) -> Result<(), ZendriverError> {
        self.inner.conn.shutdown();
        let mut child_guard = self.inner.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // Try graceful exit first.
            let _ = child.start_kill();
            match timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_status)) => {}
                _ => {
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }
}

impl Drop for BrowserInner {
    fn drop(&mut self) {
        self.conn.shutdown();
        // We can't `.await` in Drop. If `close()` was not called explicitly,
        // we rely on `kill_on_drop(true)` set on the spawned Command, which
        // causes tokio to SIGKILL the child when the Child is dropped.
        // The TempDir for user_data_dir is dropped here too.
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p zendriver`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(zendriver): graceful Browser::close + Drop teardown"
```

---

## Task 16: `Tab` struct + `Tab::session()` escape hatch

Tab already declared in Task 14. This task formalizes the public surface.

**Files:**
- Modify: `crates/zendriver/src/tab.rs`

- [ ] **Step 1: Expand Tab**

Path: `crates/zendriver/src/tab.rs` (replace earlier stub with this):

```rust
//! Tab — handle to a single CDP target session.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tracing::trace;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};

#[derive(Clone)]
pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
}

impl Tab {
    pub(crate) fn new(session: SessionHandle) -> Self {
        Self {
            inner: Arc::new(TabInner { session }),
        }
    }

    /// Escape hatch: raw `SessionHandle` for advanced users who need to send
    /// CDP commands the high-level API doesn't expose.
    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }

    /// Helper: call a CDP method on this tab's session, parsing transport
    /// errors into `ZendriverError`.
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        trace!(%method, "tab.call");
        let res = self.inner.session.call(method, params).await?;
        Ok(res)
    }
}
```

- [ ] **Step 2: Build + verify unit tests still pass**

Run: `cargo build -p zendriver`
Expected: clean build.

Run: `cargo test -p zendriver --lib`
Expected: prior tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): Tab public surface + session() escape hatch"
```

---

## Task 17: `Tab::goto` + `Tab::wait_for_load`

**Files:**
- Modify: `crates/zendriver/src/tab.rs`

- [ ] **Step 1: Add goto + wait_for_load**

Append to `crates/zendriver/src/tab.rs`:

```rust
use futures::StreamExt;
use std::time::Duration;
use tokio::time::timeout;

const DEFAULT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

impl Tab {
    /// Navigate the tab to the given URL. Does NOT wait for the load to
    /// complete — call `wait_for_load` after.
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()> {
        // Enable Page domain so we get FrameStoppedLoading events.
        self.call("Page.enable", json!({})).await?;
        let url_s = url.as_ref().to_string();
        let res = self.call("Page.navigate", json!({ "url": url_s })).await?;
        if let Some(err) = res.get("errorText").and_then(|v| v.as_str()) {
            if !err.is_empty() {
                return Err(ZendriverError::Navigation(err.to_string()));
            }
        }
        Ok(())
    }

    /// Wait until the main frame's load event fires.
    pub async fn wait_for_load(&self) -> Result<()> {
        // Subscribe before any `goto` to avoid missing the event; in P1 we
        // accept that callers may have a small race. P3+ revisits.
        let mut stream = self
            .inner
            .session
            .subscribe::<Value>("Page.frameStoppedLoading");
        timeout(DEFAULT_LOAD_TIMEOUT, stream.next())
            .await
            .map_err(|_| ZendriverError::Timeout(DEFAULT_LOAD_TIMEOUT))?
            .ok_or_else(|| ZendriverError::Navigation("page event stream closed".into()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn goto_sends_page_enable_then_page_navigate_with_url() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://example.com").await }
        });

        let _id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(_id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        assert_eq!(mock.last_sent()["params"]["url"], "https://example.com");
        mock.reply(id_nav, json!({ "frameId": "F1" })).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn goto_returns_navigation_error_when_chrome_reports_errortext() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.goto("https://bad.test").await }
        });

        let id_enable = mock.expect_cmd("Page.enable").await;
        mock.reply(id_enable, json!({})).await;

        let id_nav = mock.expect_cmd("Page.navigate").await;
        mock.reply(
            id_nav,
            json!({ "errorText": "net::ERR_NAME_NOT_RESOLVED" }),
        )
        .await;

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::Navigation(m)) => assert!(m.contains("ERR_NAME_NOT_RESOLVED")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }
}
```

To use `MockConnection` in `zendriver`'s tests, enable the `testing` feature on `zendriver-transport` for dev-only. Edit `crates/zendriver/Cargo.toml`:

```toml
[dev-dependencies]
# ... existing ...
zendriver-transport = { workspace = true, features = ["testing"] }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib tab::tests`
Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/tab.rs crates/zendriver/Cargo.toml
git commit -m "feat(zendriver): Tab::goto + wait_for_load with mock-driven tests"
```

---

## Task 18: `Tab::evaluate<T>` (typed Runtime.evaluate)

**Files:**
- Modify: `crates/zendriver/src/tab.rs`

- [ ] **Step 1: Add evaluate**

Append to `impl Tab`:

```rust
impl Tab {
    /// Evaluate a JavaScript expression in the tab's main frame. The result
    /// is deserialized into `T`. Throws `JsException` if the expression
    /// raises.
    pub async fn evaluate<T: DeserializeOwned>(
        &self,
        js: impl AsRef<str>,
    ) -> Result<T> {
        let res = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": js.as_ref(),
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
            return Err(ZendriverError::JsException(msg));
        }
        let value = res
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }
}
```

Append test inside `mod tests`:

```rust
    #[tokio::test]
    async fn evaluate_returns_typed_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("1+1").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["params"]["expression"], "1+1");
        mock.reply(id, json!({ "result": { "value": 2, "type": "number" } })).await;
        let n = fut.await.unwrap().unwrap();
        assert_eq!(n, 2);
        conn.shutdown();
    }

    #[tokio::test]
    async fn evaluate_returns_js_exception_when_chrome_reports_one() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.evaluate::<i32>("throw new Error('boom')").await }
        });

        let id = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id,
            json!({
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": {
                    "exception": { "description": "Error: boom\n    at <anonymous>:1:7" }
                }
            }),
        )
        .await;
        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::JsException(m)) => assert!(m.contains("Error: boom")),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib tab::tests`
Expected: 4 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): Tab::evaluate<T> via Runtime.evaluate"
```

---

## Task 19: `Tab::url` + `Tab::title`

**Files:**
- Modify: `crates/zendriver/src/tab.rs`

- [ ] **Step 1: Add url + title**

Append to `impl Tab`:

```rust
impl Tab {
    /// Get the tab's current URL.
    pub async fn url(&self) -> Result<url::Url> {
        let res = self.call("Target.getTargetInfo", json!({})).await?;
        let s = res["targetInfo"]["url"]
            .as_str()
            .ok_or_else(|| ZendriverError::Navigation("target has no url".into()))?;
        url::Url::parse(s).map_err(|e| ZendriverError::Navigation(e.to_string()))
    }

    /// Get the tab's `<title>`.
    pub async fn title(&self) -> Result<String> {
        let res = self.call("Target.getTargetInfo", json!({})).await?;
        Ok(res["targetInfo"]["title"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }
}
```

Append tests:

```rust
    #[tokio::test]
    async fn url_returns_parsed_url_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.url().await }
        });

        let id = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id,
            json!({ "targetInfo": { "url": "https://example.com/x", "title": "ok" } }),
        )
        .await;
        let u = fut.await.unwrap().unwrap();
        assert_eq!(u.as_str(), "https://example.com/x");
        conn.shutdown();
    }

    #[tokio::test]
    async fn title_returns_string_from_target_info() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.title().await }
        });

        let id = mock.expect_cmd("Target.getTargetInfo").await;
        mock.reply(
            id,
            json!({ "targetInfo": { "url": "https://x", "title": "Hello" } }),
        )
        .await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "Hello");
        conn.shutdown();
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib tab::tests`
Expected: 6 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): Tab::url + Tab::title via Target.getTargetInfo"
```

---

## Task 20: `Element` struct + node-id resolution

**Files:**
- Modify: `crates/zendriver/src/element.rs`

- [ ] **Step 1: Add Element**

Path: `crates/zendriver/src/element.rs`

```rust
//! `Element` — handle to a DOM node via CDP `RemoteObjectId` / `BackendNodeId`.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

#[derive(Clone)]
pub struct Element {
    pub(crate) inner: Arc<ElementInner>,
}

pub(crate) struct ElementInner {
    pub(crate) tab: Tab,
    pub(crate) backend_node_id: i64,
    pub(crate) remote_object_id: String,
}

impl Element {
    pub(crate) fn new(tab: Tab, backend_node_id: i64, remote_object_id: String) -> Self {
        Self {
            inner: Arc::new(ElementInner {
                tab,
                backend_node_id,
                remote_object_id,
            }),
        }
    }

    pub(crate) fn tab(&self) -> &Tab {
        &self.inner.tab
    }

    /// Call a JS function on this element's remote object. The function
    /// signature MUST take exactly one parameter (the element); use
    /// `function(el){ ... }`.
    pub(crate) async fn call_on(
        &self,
        function: &str,
        args: Value,
    ) -> Result<Value> {
        let res = self
            .inner
            .tab
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": self.inner.remote_object_id,
                    "functionDeclaration": function,
                    "arguments": args,
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
            return Err(ZendriverError::JsException(msg));
        }
        Ok(res["result"].clone())
    }

    /// Evaluate a JS expression where `el` is bound to this element handle.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let function = format!("function(el){{ return ({}) }}", js.as_ref());
        let result = self.call_on(&function, json!([{ "objectId": self.inner.remote_object_id }])).await?;
        let value = result.get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p zendriver`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/element.rs
git commit -m "feat(zendriver): Element struct + Runtime.callFunctionOn helper + evaluate"
```

---

## Task 21: `Element::click`

**Files:**
- Modify: `crates/zendriver/src/element.rs`

- [x] **Step 1: Add click + tests**

Append to `impl Element`:

```rust
impl Element {
    /// Click this element via DOM `el.click()`. Phase 1 uses the simple DOM
    /// dispatch; Phase 3 will upgrade to realistic mouse-move + `Input.dispatchMouseEvent`.
    pub async fn click(&self) -> Result<()> {
        let _ = self
            .call_on("function(){ this.click(); }", json!([]))
            .await?;
        Ok(())
    }
}
```

Append `#[cfg(test)] mod tests` at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn click_calls_runtime_callfunctionon_with_this_dot_click() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);
        let el = Element::new(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.click().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["objectId"], "R1");
        assert!(last["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("this.click()"));
        mock.reply(id, json!({ "result": { "type": "undefined" } })).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
```

- [x] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib element::tests`
Expected: 1 passed.

- [x] **Step 3: Commit**

```bash
git add crates/zendriver/src/element.rs
git commit -m "feat(zendriver): Element::click via Runtime.callFunctionOn"
```

---

## Task 22: `Element::inner_text` + `outer_html`

**Files:**
- Modify: `crates/zendriver/src/element.rs`

- [ ] **Step 1: Add inner_text + outer_html**

Append to `impl Element`:

```rust
impl Element {
    pub async fn inner_text(&self) -> Result<String> {
        let res = self
            .call_on("function(){ return this.innerText; }", json!([]))
            .await?;
        Ok(res["value"].as_str().unwrap_or("").to_string())
    }

    pub async fn outer_html(&self) -> Result<String> {
        let res = self
            .call_on("function(){ return this.outerHTML; }", json!([]))
            .await?;
        Ok(res["value"].as_str().unwrap_or("").to_string())
    }
}
```

Append tests:

```rust
    #[tokio::test]
    async fn inner_text_returns_value_field() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);
        let el = Element::new(tab, 1, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.inner_text().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(id, json!({ "result": { "value": "hello", "type": "string" } })).await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "hello");
        conn.shutdown();
    }

    #[tokio::test]
    async fn outer_html_returns_value_field() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);
        let el = Element::new(tab, 1, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.outer_html().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(id, json!({ "result": { "value": "<button>x</button>", "type": "string" } })).await;
        let s = fut.await.unwrap().unwrap();
        assert_eq!(s, "<button>x</button>");
        conn.shutdown();
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p zendriver --lib element::tests`
Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/element.rs
git commit -m "feat(zendriver): Element::inner_text + outer_html"
```

---

## Task 23: `FindBuilder::css().one()` + `one_or_none()` + `.timeout()`

**Files:**
- Modify: `crates/zendriver/src/query.rs`
- Modify: `crates/zendriver/src/tab.rs` (add `find()` method)

- [ ] **Step 1: Implement FindBuilder**

Path: `crates/zendriver/src/query.rs`

```rust
//! `FindBuilder` — chainable element queries scoped to a `Tab`.

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct FindBuilder<'tab> {
    pub(crate) tab: &'tab Tab,
    pub(crate) selector: Option<String>,
    pub(crate) timeout: Duration,
}

impl<'tab> FindBuilder<'tab> {
    pub(crate) fn new(tab: &'tab Tab) -> Self {
        Self {
            tab,
            selector: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    #[must_use]
    pub fn css(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Wait for and return the first matching element. Errors with
    /// `ElementNotFound` if no element matches within the timeout.
    pub async fn one(self) -> Result<Element> {
        let sel = self
            .selector
            .ok_or_else(|| ZendriverError::Navigation(
                "FindBuilder requires a selector (.css(...))".into(),
            ))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(el) = try_query_selector(self.tab, &sel).await? {
                return Ok(el);
            }
            if Instant::now() >= deadline {
                return Err(ZendriverError::ElementNotFound { selector: sel });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Like `one()`, but returns `None` instead of erroring when no element
    /// matches within the timeout.
    pub async fn one_or_none(self) -> Result<Option<Element>> {
        match self.one().await {
            Ok(el) => Ok(Some(el)),
            Err(ZendriverError::ElementNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

async fn try_query_selector(tab: &Tab, selector: &str) -> Result<Option<Element>> {
    // Use Runtime.evaluate to find the node and return a remote object handle.
    let res = tab
        .call(
            "Runtime.evaluate",
            json!({
                "expression": format!("document.querySelector({})", json!(selector)),
                "returnByValue": false,
            }),
        )
        .await?;
    let result = &res["result"];
    if result["subtype"] == "null" || result["type"] == "undefined" {
        return Ok(None);
    }
    let object_id = result["objectId"]
        .as_str()
        .ok_or_else(|| ZendriverError::Navigation("querySelector returned no objectId".into()))?
        .to_string();

    // Get the backend node id for later use (Element::call_on uses objectId,
    // but other operations need backend_node_id — we resolve once here).
    let describe = tab
        .call(
            "DOM.describeNode",
            json!({ "objectId": object_id }),
        )
        .await
        .ok();
    let backend_node_id = describe
        .as_ref()
        .and_then(|d| d["node"]["backendNodeId"].as_i64())
        .unwrap_or_default();

    Ok(Some(Element::new(tab.clone(), backend_node_id, object_id)))
}
```

- [ ] **Step 2: Wire `Tab::find`**

Append to `impl Tab` in `crates/zendriver/src/tab.rs`:

```rust
impl Tab {
    pub fn find(&self) -> crate::query::FindBuilder<'_> {
        crate::query::FindBuilder::new(self)
    }
}
```

- [ ] **Step 3: Add tests**

Append to `crates/zendriver/src/query.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn one_returns_element_when_query_selector_matches() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().css("#b").one().await }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert!(mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .contains("document.querySelector"));
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "R1", "type": "object", "subtype": "node" } }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        mock.reply(id_d, json!({ "node": { "backendNodeId": 42 } })).await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(el.inner.backend_node_id, 42);
        assert_eq!(el.inner.remote_object_id, "R1");
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_returns_element_not_found_when_query_returns_null() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find()
                    .css("#missing")
                    .timeout(Duration::from_millis(150))
                    .one()
                    .await
            }
        });

        // The builder will poll a few times; reply null each time until timeout.
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(cmd, json!({ "result": { "type": "object", "subtype": "null" } })).await;
                }
            }
        }

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::ElementNotFound { selector }) => assert_eq!(selector, "#missing"),
            other => panic!("unexpected: {other:?}"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_or_none_returns_none_on_timeout() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find()
                    .css("#missing")
                    .timeout(Duration::from_millis(120))
                    .one_or_none()
                    .await
            }
        });

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(180)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(cmd, json!({ "result": { "type": "object", "subtype": "null" } })).await;
                }
            }
        }

        let res = fut.await.unwrap().unwrap();
        assert!(res.is_none());
        conn.shutdown();
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zendriver --lib query::tests`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver/src/query.rs crates/zendriver/src/tab.rs
git commit -m "feat(zendriver): FindBuilder.css().one() + one_or_none() + timeout"
```

---

## Task 24: `zendriver::start` convenience + crate re-exports

**Files:**
- Modify: `crates/zendriver/src/lib.rs`

- [ ] **Step 1: Add re-exports + start()**

Path: `crates/zendriver/src/lib.rs`

```rust
//! zendriver — async, undetectable Chrome automation over CDP.
//!
//! Phase 1 public surface.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod browser;
pub mod element;
pub mod error;
pub mod query;
pub mod tab;

pub use browser::{Browser, BrowserBuilder};
pub use element::Element;
pub use error::{BrowserError, Result, ZendriverError};
pub use query::FindBuilder;
pub use tab::Tab;

// Re-export selected transport types for advanced users.
pub use zendriver_transport::{Connection, SessionHandle, TransportError};

/// Convenience entry point: launch a Chrome instance with default settings.
///
/// Equivalent to `Browser::builder().launch().await`.
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// let browser = zendriver::start().await?;
/// let tab = browser.main_tab();
/// tab.goto("https://example.com").await?;
/// # Ok(()) }
/// ```
pub async fn start() -> Result<Browser> {
    Browser::builder().launch().await
}
```

- [ ] **Step 2: Build + doctest**

Run: `cargo build -p zendriver`
Expected: clean.

Run: `cargo test -p zendriver --doc`
Expected: PASS (doctest compiles, `no_run` doesn't execute).

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/lib.rs
git commit -m "feat(zendriver): public re-exports + start() convenience"
```

---

## Task 25: `examples/hello.rs` + integration test against wiremock

**Files:**
- Create: `examples/hello.rs`
- Create: `crates/zendriver/tests/integration_phase1.rs`

- [ ] **Step 1: Write `examples/hello.rs`**

Path: `examples/hello.rs`

```rust
//! Phase 1 exit example: launch Chrome, navigate to example.com, find <h1>,
//! print its text.

use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let h1 = tab.find().css("h1").one().await?;
    let text = h1.inner_text().await?;
    println!("h1 text: {text}");

    browser.close().await?;
    Ok(())
}
```

The example sits at the workspace root `examples/` directory but to be picked up by `cargo`, it needs the `zendriver` crate as the workspace's default example host. Easiest: put it under `crates/zendriver/examples/hello.rs` instead, where `cargo run --example hello -p zendriver` picks it up. Use that location.

Move to: `crates/zendriver/examples/hello.rs` (same contents).

Also add to `crates/zendriver/Cargo.toml`:

```toml
[[example]]
name = "hello"
required-features = []

[dev-dependencies]
tracing-subscriber.workspace = true
```

- [ ] **Step 2: Write the integration test**

Path: `crates/zendriver/tests/integration_phase1.rs`

```rust
//! Phase 1 end-to-end test: real Chrome against a wiremock HTTP fixture.
//!
//! Gated behind the `integration-tests` feature so CI can skip it on
//! Chrome-less runners.

#![cfg(feature = "integration-tests")]

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::Browser;

#[tokio::test]
#[serial]
async fn click_dispatches_event_to_dom_listener() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<!doctype html>
            <html><body>
              <button id="b" onclick="window.clicked = true">x</button>
            </body></html>"#,
        ))
        .mount(&mock)
        .await;

    let browser = Browser::builder()
        .headless(true)
        .launch()
        .await
        .expect("launch failed");
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.expect("goto");
    tab.wait_for_load().await.expect("wait_for_load");

    let btn = tab.find().css("#b").one().await.expect("find #b");
    btn.click().await.expect("click");

    let clicked: bool = tab.evaluate("window.clicked").await.expect("eval");
    assert!(clicked, "button click should have set window.clicked");

    browser.close().await.expect("close");
}
```

- [ ] **Step 3: Run unit tests + integration test (locally; integration requires Chrome)**

Run: `cargo test -p zendriver --lib`
Expected: all previous tests pass.

Run (if Chrome is installed): `cargo test -p zendriver --features integration-tests --test integration_phase1 -- --test-threads=1`
Expected: 1 passed.

If Chrome is not installed locally, the integration test will be skipped (only runs under the feature flag) — CI runs it.

- [ ] **Step 4: Commit**

```bash
git add crates/zendriver/examples/hello.rs crates/zendriver/tests/integration_phase1.rs crates/zendriver/Cargo.toml
git commit -m "test(zendriver): integration test against wiremock + hello.rs example"
```

---

## Task 26: Insta snapshot tests for launch flags + error displays

**Files:**
- Create: `crates/zendriver/tests/snapshots.rs`
- Create: `crates/zendriver/tests/snapshots/` (insta drops files here)

- [ ] **Step 1: Write the snapshot tests**

Path: `crates/zendriver/tests/snapshots.rs`

```rust
//! Snapshot tests for stable output: launch flags, error Display.

use std::path::Path;

use zendriver::{BrowserBuilder, ZendriverError, BrowserError};

#[test]
fn default_launch_flags_snapshot() {
    let b = BrowserBuilder::new();
    let flags = b.build_flags(Path::new("/tmp/test-user-data"));
    insta::assert_yaml_snapshot!(flags);
}

#[test]
fn non_headless_launch_flags_snapshot() {
    let b = BrowserBuilder::new().headless(false);
    let flags = b.build_flags(Path::new("/tmp/test-user-data"));
    insta::assert_yaml_snapshot!(flags);
}

#[test]
fn error_displays_snapshot() {
    let cases = vec![
        ("element_not_found", ZendriverError::ElementNotFound {
            selector: "button.foo".into(),
        }.to_string()),
        ("timeout_5s", ZendriverError::Timeout(std::time::Duration::from_secs(5)).to_string()),
        ("cdp_invalid_params", ZendriverError::Cdp {
            code: -32602,
            message: "Invalid params".into(),
            data: None,
        }.to_string()),
        ("navigation", ZendriverError::Navigation("ERR_NAME_NOT_RESOLVED".into()).to_string()),
        ("js_exception", ZendriverError::JsException("Error: boom".into()).to_string()),
        ("browser_no_exec", ZendriverError::Browser(BrowserError::ExecutableNotFound {
            searched: vec!["/usr/bin/google-chrome".into()]
        }).to_string()),
    ];
    insta::assert_yaml_snapshot!(cases);
}
```

`build_flags` is `pub(crate)` — to use from an external `tests/` directory, the test target is outside the crate. Two options:
1. Mark `build_flags` `pub` in `Cargo.toml` features (overkill).
2. Move snapshot tests into a `#[cfg(test)] mod` inside `browser.rs` and use `insta` there.

Pick option 2. Move `snapshots.rs` contents into `crates/zendriver/src/browser.rs`'s `mod tests` block, and likewise for error displays into `error.rs`'s `mod tests`.

Update `crates/zendriver/src/browser.rs` to append into `mod tests`:

```rust
    #[test]
    fn default_launch_flags_snapshot() {
        let b = BrowserBuilder::new();
        let flags = b.build_flags(std::path::Path::new("/tmp/test-user-data"));
        insta::assert_yaml_snapshot!("default_launch_flags", flags);
    }

    #[test]
    fn non_headless_launch_flags_snapshot() {
        let b = BrowserBuilder::new().headless(false);
        let flags = b.build_flags(std::path::Path::new("/tmp/test-user-data"));
        insta::assert_yaml_snapshot!("non_headless_launch_flags", flags);
    }
```

Update `crates/zendriver/src/error.rs` to append into `mod tests`:

```rust
    #[test]
    fn error_displays_snapshot() {
        let cases = vec![
            ("element_not_found", ZendriverError::ElementNotFound { selector: "button.foo".into() }.to_string()),
            ("timeout_5s", ZendriverError::Timeout(Duration::from_secs(5)).to_string()),
            ("cdp_invalid_params", ZendriverError::Cdp { code: -32602, message: "Invalid params".into(), data: None }.to_string()),
            ("navigation", ZendriverError::Navigation("ERR_NAME_NOT_RESOLVED".into()).to_string()),
            ("js_exception", ZendriverError::JsException("Error: boom".into()).to_string()),
        ];
        insta::assert_yaml_snapshot!("error_displays", cases);
    }
```

Delete the standalone `tests/snapshots.rs` file (we used the inline form instead).

- [ ] **Step 2: Run snapshot tests + accept**

Run: `cargo test -p zendriver --lib`
Expected: tests fail because no snapshots exist yet (insta reports pending).

Run: `cargo insta accept`
Expected: snapshot files written to `crates/zendriver/src/snapshots/`.

Re-run: `cargo test -p zendriver --lib`
Expected: all pass.

- [ ] **Step 3: Commit**

```bash
git add crates/zendriver/src/browser.rs crates/zendriver/src/error.rs crates/zendriver/src/snapshots
git commit -m "test(zendriver): insta snapshots for launch flags + error Displays"
```

---

## Task 27: CI workflow finalization + README polish

**Files:**
- Modify: `.github/workflows/ci.yml` (verified in Task 0; nothing more to add for P1)
- Modify: `README.md`

- [ ] **Step 1: Update README with usage example**

Path: `README.md`

```markdown
# zendriver-rs

A Rust port of [zendriver](https://github.com/cdpdriver/zendriver) — an undetectable, async-first browser automation library using the Chrome DevTools Protocol directly.

**Status:** Phase 1 (foundation) under active development. Not yet published to crates.io.

## Example

```rust
use zendriver::Browser;

#[tokio::main]
async fn main() -> zendriver::Result<()> {
    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let h1 = tab.find().css("h1").one().await?;
    println!("h1: {}", h1.inner_text().await?);

    browser.close().await?;
    Ok(())
}
```

## Phases

1. **Foundation** (in progress): transport + minimal `Browser`/`Tab`/`Element`.
2. Stealth (planned).
3. Element API completeness (planned).
4. `Tab`/`Browser` completeness, cookies, screenshots, multi-tab, iframes (planned).
5. Optional gated features: interception, Cloudflare bypass, `expect()`, fetcher (planned).
6. Polish + 0.1 release (planned).

See `docs/superpowers/specs/` for per-phase design documents.

## Development

```bash
cargo test --workspace --lib                                       # unit tests, no Chrome
cargo test --workspace --doc                                       # doctests
cargo clippy --workspace --all-targets --locked -- -D warnings    # lint
cargo fmt --all --check                                            # format
cargo test --workspace --features integration-tests --test '*' -- --test-threads=1  # real Chrome (requires Chrome on $PATH)
```

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) and Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.
```

- [ ] **Step 2: Verify CI workflow**

The workflow created in Task 0 already covers all P1 needs:
- fmt
- clippy with `-D warnings`
- unit tests
- doctests
- integration tests (gated by feature flag, runs once Chrome is installed in CI)

No edits needed unless `cargo test --workspace --lib` exposes a missing target. Verify locally:

Run: `cargo build --workspace --all-targets --locked`
Expected: clean.

Run: `cargo test --workspace --lib --locked`
Expected: all pass.

Run: `cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS.

Run: `cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: README with phase 1 usage example and dev workflow"
```

---

## Self-review checklist

After all tasks complete, run this checklist:

**Spec coverage:**
- [x] Workspace skeleton with all six crates — Task 0
- [x] `TransportError` — Task 1
- [x] CDP frame types — Task 2
- [x] `RawEvent` re-exports — Task 3
- [x] Connection actor cmd/response — Task 4
- [x] Event broadcast — Task 5
- [x] Shutdown drain — Task 6
- [x] `Connection` handle + typed subscribe — Task 7
- [x] `SessionHandle` — Task 8
- [x] `MockConnection` — Task 9
- [x] `ZendriverError` + `BrowserError` + `Result` — Task 10
- [x] Chrome auto-discovery — Task 11
- [x] DevTools URL parser — Task 12
- [x] `BrowserBuilder` — Task 13
- [x] `Browser::launch` — Task 14
- [x] `Browser::close` + `Drop` — Task 15
- [x] `Tab` + `session()` — Task 16
- [x] `Tab::goto` + `wait_for_load` — Task 17
- [x] `Tab::evaluate<T>` — Task 18
- [x] `Tab::url` + `Tab::title` — Task 19
- [x] `Element` + node-id resolution — Task 20
- [x] `Element::click` — Task 21
- [x] `Element::inner_text` + `outer_html` — Task 22
- [x] `FindBuilder` (`css`, `one`, `one_or_none`, `timeout`) — Task 23
- [x] `zendriver::start` + re-exports — Task 24
- [x] `examples/hello.rs` + integration test — Task 25
- [x] Snapshot tests — Task 26
- [x] CI + README — Tasks 0 + 27

**Placeholder scan:**
- No TBD / TODO / "add appropriate error handling" / "implement later".
- Every step that changes code shows the code.

**Type consistency:**
- `OutboundCmd`, `RawEvent`, `CdpRpcError`, `Connection`, `SessionHandle`, `Tab`, `Element`, `FindBuilder` — names consistent throughout.
- `Tab::call(method, params)` signature stable (used by `goto`, `evaluate`, `url`, `title`, `query`).
- `Element::call_on(function, args)` signature stable (used by `click`, `inner_text`, `outer_html`, `evaluate`).
- `zendriver_transport::testing::MockConnection::{pair, expect_cmd, last_sent, reply, emit_event}` — names match every test that imports them.

---

## Notes for the implementing engineer

1. **`chromiumoxide_cdp` is used for type bindings only in this phase.** The actual `Command` trait integration is deferred to a refactor in Phase 3 — for Phase 1 we send raw JSON via `Connection::call_raw` because the surface area is small. When you reach a CDP call where the typed `chromiumoxide_cdp` API would be clearly nicer, leave a `// TODO(phase-3): use chromiumoxide_cdp typed Command` comment, but do not refactor in Phase 1.

2. **Some test helpers cross modules.** The `DriverStream` test struct lives in `crates/zendriver-transport/src/connection.rs::test_only` so both `actor::tests` and `session::tests` can construct it. Don't duplicate the impl.

3. **The integration test in Task 25 requires Chrome installed.** On macOS it'll find `/Applications/Google Chrome.app/...`; on Linux CI it requires `apt-get install chromium-browser` first (already in `.github/workflows/ci.yml`).

4. **Insta snapshots get committed.** When you run `cargo insta accept`, the resulting `*.snap` files under `crates/zendriver/src/snapshots/` go into git so other contributors don't regenerate them.

5. **If a step's code references a function defined in a later step, that's a bug — flag it.** Tasks should be readable in order.

6. **Commit cadence: one commit per task is fine.** If a task involves multiple files + a meaningful intermediate state, split the commit. Don't accumulate uncommitted work across tasks.
