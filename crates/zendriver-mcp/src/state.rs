//! Per-MCP-session mutable state.
//!
//! Wrapped in `Arc<tokio::sync::Mutex<_>>` and shared across tool handlers.
//! In stdio mode there is one global instance; in HTTP mode one per session.

#[cfg(any(feature = "expect", feature = "interception", feature = "monitor"))]
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zendriver::Browser;

/// Opaque registration id for [`SessionState::expectations`].
#[cfg(feature = "expect")]
pub type ExpectationId = String;

/// Opaque registration id for [`SessionState::rules`].
#[cfg(feature = "interception")]
pub type RuleId = String;

/// Opaque handle id for a running monitor in [`SessionState::monitors`].
#[cfg(feature = "monitor")]
pub type MonitorId = String;

/// Bounded capacity of each monitor's in-memory event ring.
///
/// Once a monitor's drain task has buffered this many unread events, the
/// oldest is evicted on each new push and [`MonitorBuffer::dropped`] is
/// incremented — so a slow `browser_monitor_read` caller bounds memory rather
/// than growing it without limit. The next `read` surfaces the running
/// `dropped` count so callers know events were lost.
#[cfg(feature = "monitor")]
pub const MONITOR_BUFFER_CAP: usize = 4096;

/// Stealth profile choice carried over the MCP wire.
///
/// Concrete `StealthProfile` resolution happens inside the lifecycle
/// handler (it depends on platform detection that only matters at launch
/// time); the wire-level enum stays stable and platform-agnostic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StealthProfileChoice {
    /// Defer to `zendriver_stealth::StealthProfile::native()` (auto-detect
    /// platform via `sysinfo`).
    #[default]
    Auto,
    /// Force native (same as `Auto`, but explicit).
    Native,
    /// Spoof a macOS fingerprint regardless of host platform.
    SpoofMacos,
    /// Spoof a Linux fingerprint regardless of host platform.
    SpoofLinux,
    /// Spoof a Windows fingerprint regardless of host platform.
    SpoofWindows,
}

/// Platform to spoof via a fine-grained stealth override.
///
/// Wire mirror of `zendriver::stealth::Platform`; kept here (platform-agnostic)
/// so the override schema stays stable independent of host detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StealthPlatformChoice {
    /// Windows.
    Win32,
    /// macOS (Intel).
    MacIntel,
    /// Linux x86_64.
    LinuxX86_64,
}

/// Fine-grained stealth fingerprint overrides layered onto the chosen
/// [`StealthProfileChoice`] at the next `browser_open`.
///
/// Every field is optional — an unset field leaves the base profile's value
/// in place. Most meaningful paired with a `spoof_*` profile; applying these
/// to `native` overrides the auto-detected real fingerprint and can *reduce*
/// stealth if the values disagree with the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StealthOverrides {
    /// Spoofed `navigator.platform` / UA platform token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<StealthPlatformChoice>,
    /// Spoofed locale (e.g. `"en-US"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Spoofed timezone (IANA name, e.g. `"America/New_York"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Spoofed `navigator.deviceMemory` in GiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<u32>,
    /// Spoofed `navigator.hardwareConcurrency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_count: Option<u32>,
    /// Spoofed Chrome major version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chrome_version: Option<u32>,
    /// Full User-Agent string override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Toggle Content-Security-Policy bypass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_csp: Option<bool>,
    /// Opt in to Chrome's real site isolation (`IsolateOrigins`/
    /// `site-per-process` stay enabled) and, for `spoof_*` profiles, skip
    /// the WebGL vendor/renderer patch — the host's real WebGL renderer
    /// passes through unpatched. **Trade-off, not a strict stealth
    /// improvement**: dropping the WebGL patch removes an anti-WAF
    /// coherence defense (see `StealthProfile::native_isolation` rustdoc).
    /// Off (`false`/unset) by default — existing behavior unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_isolation: Option<bool>,
    /// Derive a coherent `locale` + `languages` from a country code
    /// (ISO 3166-1 alpha-2, e.g. `"US"`). Overridden by an explicit `locale`.
    ///
    /// Always present in the schema so the wire shape is feature-stable; it
    /// only takes effect when the server is built with the `geo` feature
    /// (otherwise the field is accepted and ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_country: Option<String>,
}

/// Input-timing profile choice carried over the MCP wire.
///
/// Wire mirror of `zendriver::stealth::InputProfile`'s two presets.
/// Decoupled from [`StealthProfileChoice`] — an explicit choice here always
/// wins over anything stealth would otherwise imply. When `browser_open`'s
/// `input_profile` field is left unset, the *effective* profile instead
/// follows the resolved stealth profile (spoofed stealth implies
/// `Coherent`, `auto`/`native` stealth implies `Native`) — see
/// `crate::tools::lifecycle::input_profile_choice_for`. This type's own
/// `#[default]` (`Native`) is not consulted on that unset path; nothing in
/// this crate currently calls `InputProfileChoice::default()`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputProfileChoice {
    /// Zero-overhead, deterministic timing (`InputProfile::native`). Default.
    #[default]
    Native,
    /// Non-mechanical, humanized timing (`InputProfile::coherent`) — human-
    /// paced typing and jittery mouse motion.
    Coherent,
}

/// State held for the duration of a single MCP session.
///
/// `browser` is `None` until `browser_open` is called. `current_tab_id`
/// tracks the focused tab (matches `zendriver::Tab::target_id`).
pub struct SessionState {
    pub browser: Option<Browser>,
    pub current_tab_id: Option<String>,
    pub stealth_profile_choice: StealthProfileChoice,
    pub stealth_overrides: StealthOverrides,

    #[cfg(feature = "expect")]
    pub expectations: HashMap<ExpectationId, ExpectationHandle>,

    #[cfg(feature = "interception")]
    pub rules: HashMap<RuleId, InterceptRuleHandle>,

    #[cfg(feature = "monitor")]
    pub monitors: HashMap<MonitorId, std::sync::Arc<tokio::sync::Mutex<MonitorState>>>,
}

/// Live handle to a pending `expect_*` expectation.
///
/// The expectation is awaited inside a tokio task spawned by
/// `browser_expect_register`; the task forwards the result through
/// [`Self::rx`] (a `oneshot::Receiver`) carrying either the JSON-encoded
/// matched-event or a textual error from the spawned task. The
/// [`Self::task`] handle is retained so `browser_expect_cancel` can `.abort()`
/// the in-flight `.matched()` future instead of leaving it orphaned until its
/// inner timeout fires.
///
/// `kind` is a static label ("request" / "response" / "dialog" / "download")
/// for diagnostics — not currently surfaced, but cheap to keep alongside.
#[cfg(feature = "expect")]
pub struct ExpectationHandle {
    pub kind: &'static str,
    pub task: tokio::task::JoinHandle<()>,
    pub rx: tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>,
}

/// One MCP interception rule = one `zendriver_interception::InterceptHandle`.
///
/// Holding the handle is what keeps the rule live: dropping it (via
/// [`HashMap::remove`] or [`HashMap::clear`]) cancels the actor and tears
/// down `Fetch.enable` on that rule's session. `pattern` + `action_kind`
/// are kept alongside so `browser_intercept_list_rules` can report back
/// what each id corresponds to without poking at the handle's internals.
#[cfg(feature = "interception")]
pub struct InterceptRuleHandle {
    pub pattern: String,
    pub action_kind: &'static str,
    pub _handle: zendriver::InterceptHandle,
}

/// A serde + JSON-Schema mirror of `zendriver::NetworkEvent` for the MCP wire.
///
/// One variant per observed network event. HTTP bodies are captured at
/// observe-time by the drain task (before Chrome evicts them) when the monitor
/// was started with `capture_bodies` — so `body` / `body_base64` (and the
/// `body_truncated` / `body_full_bytes` / `body_capture_error` triple) are
/// present only on `http` events from a body-capturing monitor.
#[cfg(feature = "monitor")]
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonitorEvent {
    /// A completed HTTP request/response pair (or a failed request).
    Http {
        /// Request URL.
        url: String,
        /// HTTP method (e.g. `"GET"`, `"POST"`).
        method: String,
        /// Response status code, if a response was received before the request
        /// finished (`None` for a network-level failure).
        status: Option<u16>,
        /// Network-level error text, if the request failed.
        error: Option<String>,
        /// UTF-8–lossy decode of the captured response body, bounded by
        /// `capture_body_max_bytes`. Present only when the monitor captured
        /// bodies and the fetch succeeded — a truncated body still decodes
        /// (lossily) whatever prefix was kept.
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// Base64 of the captured (possibly bounded) raw response body bytes.
        /// Present only when the monitor captured bodies and the fetch
        /// succeeded.
        #[serde(skip_serializing_if = "Option::is_none")]
        body_base64: Option<String>,
        /// `true` if the body's full length exceeded `capture_body_max_bytes`
        /// and `body` / `body_base64` hold only a prefix. `false` means the
        /// entire body was captured. Present only alongside `body`.
        #[serde(skip_serializing_if = "Option::is_none")]
        body_truncated: Option<bool>,
        /// The full (pre-truncation) response body length in bytes,
        /// regardless of how much was kept in `body` / `body_base64`. Present
        /// only alongside `body`.
        #[serde(skip_serializing_if = "Option::is_none")]
        body_full_bytes: Option<u64>,
        /// Set instead of `body` / `body_base64` when body capture was
        /// requested (`capture_bodies: true`) but the `getResponseBody` fetch
        /// itself failed (e.g. Chrome already evicted the body) — distinct
        /// from a genuinely empty body, which decodes to `body: ""` rather
        /// than setting this field.
        #[serde(skip_serializing_if = "Option::is_none")]
        body_capture_error: Option<String>,
    },
    /// A new WebSocket connection was opened.
    WebSocketOpen {
        /// CDP request ID for this WebSocket connection.
        request_id: String,
        /// The WebSocket URL.
        url: String,
    },
    /// A WebSocket frame was sent or received.
    WebSocketFrame {
        /// CDP request ID for the owning WebSocket connection.
        request_id: String,
        /// `"sent"` (page → server) or `"received"` (server → page).
        direction: String,
        /// WebSocket opcode (1 = text, 2 = binary, 8 = close, …).
        opcode: u8,
        /// Frame payload (text frames as UTF-8; binary frames as base64).
        payload: String,
    },
    /// A WebSocket connection was closed.
    WebSocketClose {
        /// CDP request ID for the closed WebSocket connection.
        request_id: String,
    },
    /// An SSE `EventSource` message was received.
    EventSourceMessage {
        /// CDP request ID for the `EventSource` stream.
        request_id: String,
        /// The SSE `event:` field (empty string if omitted).
        event_name: String,
        /// The SSE `id:` field (empty string if omitted).
        event_id: String,
        /// The SSE `data:` payload.
        data: String,
    },
    /// A delivery-loss boundary on the monitor's underlying event stream (or
    /// its own correlation bookkeeping) — a wire mirror of
    /// `zendriver::NetworkDeliveryBoundary`. Previously these losses were
    /// silent; they're now explicit events on the same stream. A
    /// `disconnected` boundary means the underlying lib-side monitor's
    /// correlator task has ended — no further events will be buffered for
    /// this handle; start a new monitor to resume.
    DeliveryBoundary {
        /// `"lagged"` | `"reconnected"` | `"disconnected"` |
        /// `"correlation_evicted"` | `"decode_failed"` | `"unknown"`.
        boundary: String,
        /// Present for `lagged` / `reconnected` / `disconnected`: the
        /// transport connection generation active when the boundary occurred.
        #[serde(skip_serializing_if = "Option::is_none")]
        generation: Option<u64>,
        /// Present for `lagged`: number of events this subscription missed.
        #[serde(skip_serializing_if = "Option::is_none")]
        missed: Option<u64>,
        /// Present for `reconnected`: generation of the connection actor that
        /// was replaced.
        #[serde(skip_serializing_if = "Option::is_none")]
        previous: Option<u64>,
        /// Present for `correlation_evicted`: URL of the evicted in-flight
        /// exchange.
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
}

/// The bounded event ring shared between a monitor's drain task and its
/// `read` handler.
///
/// Holding this behind its own `Arc<Mutex<_>>` (separate from [`MonitorState`])
/// is what lets the drain task be spawned *before* [`MonitorState`] is
/// constructed: the task captures a clone of the buffer `Arc`, and the real
/// `JoinHandle` then goes into [`MonitorState`] with no placeholder spawn and
/// no self-referential cycle.
#[cfg(feature = "monitor")]
#[derive(Debug, Default)]
pub struct MonitorBuffer {
    /// Bounded ring of buffered, not-yet-read events.
    pub events: std::collections::VecDeque<MonitorEvent>,
    /// Count of events evicted because the buffer was full since the last
    /// `read`. Reset to 0 by `read`.
    pub dropped: usize,
}

/// Live state for one running monitor.
///
/// A drain task (spawned by `browser_monitor_start`) owns the lib-side
/// `NetworkMonitor` stream and pushes each converted [`MonitorEvent`] into the
/// shared [`Self::buffer`] — a bounded ring capped at [`MONITOR_BUFFER_CAP`].
/// When the buffer is full the oldest event is evicted and `dropped` is bumped.
/// `browser_monitor_read` drains the buffer; `browser_monitor_stop` cancels the
/// task via [`Self::cancel`] and aborts [`Self::task`].
///
/// Each mutex is held only briefly — the drain task locks the buffer per event,
/// `read` locks it per drain, neither across an `.await` that waits on the
/// other — so the drain/read pair cannot deadlock.
#[cfg(feature = "monitor")]
pub struct MonitorState {
    /// Shared bounded ring (also held by the drain task).
    pub buffer: std::sync::Arc<tokio::sync::Mutex<MonitorBuffer>>,
    /// Cancels the drain task cooperatively (checked in its `select!`).
    pub cancel: tokio_util::sync::CancellationToken,
    /// The drain task's join handle, aborted on `stop`.
    pub task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "monitor")]
impl Drop for MonitorState {
    /// Stop the drain task on drop (session teardown / `browser_close` /
    /// map eviction). Dropping the `JoinHandle` alone detaches the task — it
    /// holds its own `CancellationToken` clone and the live stream — so we
    /// cancel (breaks the task's `select!`) and abort here.
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

impl SessionState {
    /// Construct an empty session — no browser, no tabs, default profile.
    pub fn new() -> Self {
        Self {
            browser: None,
            current_tab_id: None,
            stealth_profile_choice: StealthProfileChoice::default(),
            stealth_overrides: StealthOverrides::default(),
            #[cfg(feature = "expect")]
            expectations: HashMap::new(),
            #[cfg(feature = "interception")]
            rules: HashMap::new(),
            #[cfg(feature = "monitor")]
            monitors: HashMap::new(),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_empty() {
        let s = SessionState::new();
        assert!(s.browser.is_none());
        assert!(s.current_tab_id.is_none());
        assert_eq!(s.stealth_profile_choice, StealthProfileChoice::Auto);
        #[cfg(feature = "expect")]
        assert!(s.expectations.is_empty());
        #[cfg(feature = "interception")]
        assert!(s.rules.is_empty());
        #[cfg(feature = "monitor")]
        assert!(s.monitors.is_empty());
    }
}
