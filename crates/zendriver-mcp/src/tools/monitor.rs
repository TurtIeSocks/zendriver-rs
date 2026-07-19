//! Network monitor tools — `browser_monitor_start` / `_read` / `_stop`.
//!
//! ## Drain-task model
//!
//! `browser_monitor_start` builds a lib-side `tab.monitor()` `Stream` and
//! spawns a *drain task* that owns that stream. The task pulls events, converts
//! each `zendriver::NetworkEvent` into the wire-level [`MonitorEvent`], and
//! pushes them into a per-handle bounded ring ([`MonitorBuffer`], capped at
//! [`MONITOR_BUFFER_CAP`]). The handle is returned synchronously; the task runs
//! until cancelled.
//!
//! `browser_monitor_read` drains the ring (up to `max`) and reports the running
//! `dropped` count (events evicted while the ring was full), then resets it.
//! `browser_monitor_stop` cancels the task's [`CancellationToken`] and aborts
//! its join handle, then drops the handle from the session registry.
//!
//! ## Why bodies are fetched in the drain task (not at read time)
//!
//! Chrome only retains a response body for a short window after the response
//! completes. So when the monitor was started with `capture_bodies`, the drain
//! task calls `exchange.body().await` *at observe-time* — the moment the
//! exchange is pulled from the stream — and inlines the result on the
//! [`MonitorEvent::Http`]. Deferring the fetch to `read` time would routinely
//! race Chrome's eviction and return empty bodies.
//!
//! ## Concurrency
//!
//! The ring lives behind its own `Arc<Mutex<MonitorBuffer>>`, shared by the
//! drain task and `read`. Each side locks it only briefly — the drain task per
//! pushed event, `read` per drain — and never holds it across an `.await` that
//! waits on the other side, so the pair cannot deadlock. Building the ring +
//! cancel token *before* spawning the task lets the real `JoinHandle` go
//! straight into [`MonitorState`] with no placeholder spawn.

#![cfg(feature = "monitor")]

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zendriver::{
    BoundedBody, FrameDirection, NetworkDeliveryBoundary, NetworkEvent, ZendriverError,
};

use crate::errors::map_error;
use crate::state::{MONITOR_BUFFER_CAP, MonitorBuffer, MonitorEvent, MonitorState, SessionState};
use crate::tools::common::current_tab;

// ---------- browser_monitor_start -----------------------------------------

/// Input for `browser_monitor_start`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartInput {
    /// Restrict buffered events to those whose URL contains this substring.
    /// Omit to buffer every observed event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_pattern: Option<String>,
    /// Fetch each HTTP response body at observe-time and inline it on the
    /// `http` event (`body` + `body_base64`). One extra CDP round-trip per
    /// exchange; off by default to keep the buffer light.
    #[serde(default)]
    pub capture_bodies: bool,
    /// Maximum bytes of a response body to capture per `http` event when
    /// `capture_bodies` is set (bounding is against the raw decoded length,
    /// never a base64 length). A body larger than this is truncated to that
    /// many bytes with `body_truncated: true`; `body_full_bytes` always
    /// reports the full pre-truncation length. Ignored when `capture_bodies`
    /// is `false`.
    ///
    /// Defaults to `0` (unbounded) — `capture_bodies` keeps its historical
    /// full-body behavior when the field is omitted. Set a positive cap
    /// (`1048576` = 1 MiB is a sensible choice when an agent forwards bodies
    /// into its own context) to bound large bodies and keep
    /// `browser_monitor_read` output manageable.
    #[serde(default)]
    pub capture_body_max_bytes: usize,
    /// Opt in to incremental HTTP response body delivery: as each response
    /// streams in, `browser_monitor_read` emits `http_data` chunk events
    /// (`request_id` + base64 `chunk_base64`) ahead of the completed `http`
    /// event for the same request. Uses passive CDP
    /// `Network.streamResourceContent` — no request interception, no
    /// response pausing. Filtered by `url_pattern` like every other event
    /// (no separate filter). Falls back gracefully to the whole-body
    /// `capture_bodies` / on-demand path on Chrome versions that don't
    /// support it (roughly pre-124) — the monitor never errors out over
    /// this. Off by default: streaming every response is wasted CDP
    /// round-trips when bodies are small.
    #[serde(default)]
    pub stream_bodies: bool,
}

/// Output of `browser_monitor_start`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StartOutput {
    /// Opaque handle for the running monitor. Pass to `browser_monitor_read`
    /// and `browser_monitor_stop`.
    pub handle: String,
}

/// Start a network monitor over the current tab and begin buffering events.
///
/// Resolves the current tab, builds the lib-side monitor stream, spawns the
/// drain task, and registers the resulting [`MonitorState`] under a fresh
/// handle. Returns immediately — events accumulate in the background.
pub async fn start(
    state: Arc<Mutex<SessionState>>,
    input: StartInput,
) -> Result<StartOutput, ErrorData> {
    let tab = {
        let s = state.lock().await;
        current_tab(&s).await?
    };

    let mut mb = tab.monitor().stream_bodies(input.stream_bodies);
    if let Some(p) = &input.url_pattern {
        mb = mb.url_pattern(p.clone());
    }
    let mut stream = mb.start().await.map_err(map_error)?;

    // Build the shared ring + cancel token FIRST, then spawn the drain task
    // capturing clones, then construct `MonitorState` with the real join
    // handle — no placeholder spawn, no self-referential cycle.
    let buffer = Arc::new(Mutex::new(MonitorBuffer::default()));
    let cancel = CancellationToken::new();
    let capture = input.capture_bodies;
    let max_bytes = input.capture_body_max_bytes;

    let task = {
        let buffer = buffer.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    next = stream.next() => {
                        let Some(ev) = next else { break };
                        // Convert (and, when capturing, fetch the body) BEFORE
                        // taking the buffer lock so the lock is never held
                        // across the `body().await` CDP round-trip.
                        let wire = convert(ev, capture, max_bytes).await;
                        let mut buf = buffer.lock().await;
                        push_capped(&mut buf, wire);
                    }
                }
            }
        })
    };

    let handle = uuid::Uuid::new_v4().to_string();
    let mon = Arc::new(Mutex::new(MonitorState {
        buffer,
        cancel,
        task,
    }));
    state.lock().await.monitors.insert(handle.clone(), mon);
    Ok(StartOutput { handle })
}

/// Push `event` into the bounded ring, evicting the oldest (and bumping
/// `dropped`) when the ring is already at [`MONITOR_BUFFER_CAP`].
fn push_capped(buf: &mut MonitorBuffer, event: MonitorEvent) {
    if buf.events.len() >= MONITOR_BUFFER_CAP {
        buf.events.pop_front();
        buf.dropped += 1;
    }
    buf.events.push_back(event);
}

/// Convert a lib-side [`NetworkEvent`] into the wire mirror, fetching the HTTP
/// response body at observe-time when `capture` is set (before Chrome evicts
/// it), bounded at `max_bytes` via [`BoundedBody`]. A body-fetch failure sets
/// `body_capture_error` (distinct from a genuinely empty body) rather than
/// silently degrading to a body-less event — the exchange's metadata is still
/// worth surfacing either way.
async fn convert(ev: NetworkEvent, capture: bool, max_bytes: usize) -> MonitorEvent {
    match ev {
        NetworkEvent::Http(ex) => {
            let captured = if capture {
                Some(wire_body_fields(ex.body().await, max_bytes))
            } else {
                None
            };
            let CapturedBody {
                body,
                body_base64,
                body_truncated,
                body_full_bytes,
                body_capture_error,
            } = captured.unwrap_or_default();
            MonitorEvent::Http {
                request_id: ex.request_id().to_string(),
                url: ex.request.url.clone(),
                method: ex.request.method.clone(),
                status: ex.status(),
                error: ex.error.clone(),
                body,
                body_base64,
                body_truncated,
                body_full_bytes,
                body_capture_error,
            }
        }
        NetworkEvent::HttpData { request_id, chunk } => MonitorEvent::HttpData {
            request_id,
            chunk_base64: BASE64.encode(&chunk),
        },
        NetworkEvent::WebSocketOpen { request_id, url } => {
            MonitorEvent::WebSocketOpen { request_id, url }
        }
        NetworkEvent::WebSocketFrame {
            request_id,
            direction,
            opcode,
            payload,
        } => MonitorEvent::WebSocketFrame {
            request_id,
            direction: match direction {
                FrameDirection::Sent => "sent",
                FrameDirection::Received => "received",
            }
            .to_string(),
            opcode,
            payload,
        },
        NetworkEvent::WebSocketClose { request_id } => MonitorEvent::WebSocketClose { request_id },
        NetworkEvent::EventSourceMessage {
            request_id,
            event_name,
            event_id,
            data,
        } => MonitorEvent::EventSourceMessage {
            request_id,
            event_name,
            event_id,
            data,
        },
        NetworkEvent::DeliveryBoundary(boundary) => convert_boundary(boundary),
    }
}

/// The wire-level body fields for one `http` [`MonitorEvent`], extracted so
/// [`wire_body_fields`] is unit-testable without a live CDP body fetch.
#[derive(Debug, Default, PartialEq, Eq)]
struct CapturedBody {
    body: Option<String>,
    body_base64: Option<String>,
    body_truncated: Option<bool>,
    body_full_bytes: Option<u64>,
    body_capture_error: Option<String>,
}

/// Turn a `NetworkExchange::body()` result into the wire-level body fields,
/// bounding capture at `max_bytes` via [`BoundedBody::capture`] (`0` means
/// unbounded — see [`BoundedBody::capture`]'s own docs).
///
/// Pulled out as a pure function (rather than inlined in [`convert`]) so the
/// truncation / error wiring is unit-testable directly against a synthetic
/// `Ok`/`Err` result, without needing a live `NetworkExchange` (whose
/// `request_id` / `session` fields are `pub(crate)` to the `zendriver` crate
/// and can't be constructed from here).
fn wire_body_fields(result: Result<Vec<u8>, ZendriverError>, max_bytes: usize) -> CapturedBody {
    match result {
        Ok(bytes) => {
            let bounded = BoundedBody::capture(&bytes, max_bytes);
            CapturedBody {
                body: Some(String::from_utf8_lossy(&bounded.bytes).into_owned()),
                body_base64: Some(BASE64.encode(&bounded.bytes)),
                body_truncated: Some(bounded.truncated),
                body_full_bytes: Some(bounded.full_len),
                body_capture_error: None,
            }
        }
        Err(e) => CapturedBody {
            body_capture_error: Some(e.to_string()),
            ..Default::default()
        },
    }
}

/// Convert a lib-side [`NetworkDeliveryBoundary`] into the wire
/// [`MonitorEvent::DeliveryBoundary`].
fn convert_boundary(boundary: NetworkDeliveryBoundary) -> MonitorEvent {
    match boundary {
        NetworkDeliveryBoundary::Lagged { missed, generation } => MonitorEvent::DeliveryBoundary {
            boundary: "lagged".to_string(),
            generation: Some(generation),
            missed: Some(missed),
            previous: None,
            url: None,
        },
        NetworkDeliveryBoundary::Reconnected {
            previous,
            generation,
        } => MonitorEvent::DeliveryBoundary {
            boundary: "reconnected".to_string(),
            generation: Some(generation),
            missed: None,
            previous: Some(previous),
            url: None,
        },
        NetworkDeliveryBoundary::Disconnected { generation } => MonitorEvent::DeliveryBoundary {
            boundary: "disconnected".to_string(),
            generation: Some(generation),
            missed: None,
            previous: None,
            url: None,
        },
        NetworkDeliveryBoundary::CorrelationEvicted { url } => MonitorEvent::DeliveryBoundary {
            boundary: "correlation_evicted".to_string(),
            generation: None,
            missed: None,
            previous: None,
            url: Some(url),
        },
        NetworkDeliveryBoundary::DecodeFailed => MonitorEvent::DeliveryBoundary {
            boundary: "decode_failed".to_string(),
            generation: None,
            missed: None,
            previous: None,
            url: None,
        },
        NetworkDeliveryBoundary::Unknown => MonitorEvent::DeliveryBoundary {
            boundary: "unknown".to_string(),
            generation: None,
            missed: None,
            previous: None,
            url: None,
        },
    }
}

// ---------- browser_monitor_read ------------------------------------------

/// Input for `browser_monitor_read`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput {
    /// Handle returned by an earlier `browser_monitor_start`.
    pub handle: String,
    /// Maximum number of events to drain this call. Omit to drain all buffered
    /// events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

/// Output of `browser_monitor_read`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ReadOutput {
    /// Drained events, oldest first.
    pub events: Vec<MonitorEvent>,
    /// Events evicted (ring full) since the previous `read`. Non-zero means the
    /// buffer overflowed — read more often or stop the monitor.
    pub dropped: usize,
}

/// Drain up to `max` buffered events from a running monitor.
///
/// Looks the handle up in the session registry, then drains its ring. The
/// running `dropped` count is returned and reset to 0.
pub async fn read(
    state: Arc<Mutex<SessionState>>,
    input: ReadInput,
) -> Result<ReadOutput, ErrorData> {
    let mon = {
        let s = state.lock().await;
        s.monitors.get(&input.handle).cloned()
    }
    .ok_or_else(|| unknown_handle(&input.handle))?;

    // Lock the MonitorState briefly to reach its shared ring, then the ring
    // itself — neither lock is held across an `.await` that the drain task
    // waits on.
    let buffer = {
        let m = mon.lock().await;
        m.buffer.clone()
    };
    let mut buf = buffer.lock().await;
    let (events, dropped) = drain(&mut buf, input.max.unwrap_or(usize::MAX));
    Ok(ReadOutput { events, dropped })
}

/// Drain up to `max` events from `buf`, returning them (oldest first) plus the
/// `dropped` count, which is reset to 0.
///
/// Extracted as a free fn so the drain + dropped-reset semantics are
/// unit-testable without a browser, a `SessionState`, or the drain task.
fn drain(buf: &mut MonitorBuffer, max: usize) -> (Vec<MonitorEvent>, usize) {
    let take = max.min(buf.events.len());
    let events: Vec<MonitorEvent> = buf.events.drain(..take).collect();
    let dropped = std::mem::take(&mut buf.dropped);
    (events, dropped)
}

// ---------- browser_monitor_stop ------------------------------------------

/// Input for `browser_monitor_stop`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopInput {
    /// Handle returned by an earlier `browser_monitor_start`.
    pub handle: String,
}

/// Output of `browser_monitor_stop`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StopOutput {
    /// `true` if a live monitor was found and stopped; `false` if the handle
    /// was unknown (already stopped, or never started).
    pub stopped: bool,
}

/// Stop a running monitor: cancel its drain task and drop its registry entry.
///
/// Removing the handle from the registry drops the `MonitorState` (and its
/// shared ring) once this function's lock guard releases; cancelling + aborting
/// the task first guarantees it stops touching the stream promptly. An unknown
/// handle returns `{ stopped: false }` (idempotent stop) rather than an error.
pub async fn stop(
    state: Arc<Mutex<SessionState>>,
    input: StopInput,
) -> Result<StopOutput, ErrorData> {
    let Some(mon) = state.lock().await.monitors.remove(&input.handle) else {
        return Ok(StopOutput { stopped: false });
    };
    let m = mon.lock().await;
    m.cancel.cancel();
    m.task.abort();
    Ok(StopOutput { stopped: true })
}

/// Wire error for a `read` / `stop` against a handle that isn't registered.
fn unknown_handle(handle: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!(
            "unknown monitor handle `{handle}`. Start one with `browser_monitor_start`; the handle may have already been stopped."
        ),
        None,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    //! No-browser unit coverage of the buffer drain + cap-eviction logic.
    //!
    //! The drain-task path (which constructs a real `tab.monitor()` stream and
    //! spawns the correlator) needs a live Chrome and is exercised in the gated
    //! integration tests. Here we drive `drain` / `push_capped` directly against
    //! a synthetic `MonitorBuffer`.
    use std::collections::VecDeque;

    use super::*;

    /// A cheap synthetic event for buffer tests.
    fn ev(n: usize) -> MonitorEvent {
        MonitorEvent::Http {
            request_id: format!("r{n}"),
            url: format!("https://example.com/{n}"),
            method: "GET".into(),
            status: Some(200),
            error: None,
            body: None,
            body_base64: None,
            body_truncated: None,
            body_full_bytes: None,
            body_capture_error: None,
        }
    }

    #[test]
    fn drain_returns_up_to_max_and_clears_them() {
        let mut buf = MonitorBuffer {
            events: (0..5).map(ev).collect(),
            dropped: 0,
        };
        let (events, dropped) = drain(&mut buf, 3);
        assert_eq!(events.len(), 3, "drains exactly `max`");
        assert_eq!(events[0], ev(0), "oldest first");
        assert_eq!(events[2], ev(2));
        assert_eq!(dropped, 0);
        // Remaining events stay, in order, for the next read.
        assert_eq!(buf.events.len(), 2);
        assert_eq!(buf.events.front().unwrap(), &ev(3));
    }

    #[test]
    fn drain_caps_at_buffer_len_when_max_exceeds_it() {
        let mut buf = MonitorBuffer {
            events: (0..2).map(ev).collect(),
            dropped: 0,
        };
        let (events, _) = drain(&mut buf, usize::MAX);
        assert_eq!(events.len(), 2, "never over-drains past what's buffered");
        assert!(buf.events.is_empty());
    }

    #[test]
    fn drain_returns_and_resets_dropped() {
        let mut buf = MonitorBuffer {
            events: VecDeque::new(),
            dropped: 3,
        };
        let (events, dropped) = drain(&mut buf, usize::MAX);
        assert!(events.is_empty());
        assert_eq!(dropped, 3, "reports the running dropped count");
        assert_eq!(buf.dropped, 0, "and resets it to zero");
    }

    #[test]
    fn push_capped_evicts_oldest_and_counts_dropped_over_cap() {
        let mut buf = MonitorBuffer::default();
        // Fill exactly to capacity — no drops yet.
        for n in 0..MONITOR_BUFFER_CAP {
            push_capped(&mut buf, ev(n));
        }
        assert_eq!(buf.events.len(), MONITOR_BUFFER_CAP);
        assert_eq!(buf.dropped, 0);

        // One more over the cap evicts the oldest (event 0) and counts it.
        push_capped(&mut buf, ev(MONITOR_BUFFER_CAP));
        assert_eq!(buf.events.len(), MONITOR_BUFFER_CAP, "stays at the cap");
        assert_eq!(buf.dropped, 1, "the evicted event is counted");
        assert_eq!(
            buf.events.front().unwrap(),
            &ev(1),
            "event 0 was evicted; event 1 is now oldest"
        );
        assert_eq!(
            buf.events.back().unwrap(),
            &ev(MONITOR_BUFFER_CAP),
            "the newest push is retained"
        );
    }

    #[tokio::test]
    async fn read_unknown_handle_is_invalid_params() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = read(
            state,
            ReadInput {
                handle: "nope".into(),
                max: None,
            },
        )
        .await
        .expect_err("expected unknown-handle error");
        assert!(err.message.contains("unknown monitor handle"));
    }

    #[tokio::test]
    async fn stop_unknown_handle_reports_not_stopped() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let out = stop(
            state,
            StopInput {
                handle: "nope".into(),
            },
        )
        .await
        .expect("stop is idempotent for unknown handles");
        assert!(!out.stopped, "unknown handle reports stopped=false");
    }

    // ---------- wire_body_fields (bounded body capture) --------------------
    //
    // `NetworkExchange`'s `request_id` / `session` fields are `pub(crate)` to
    // `zendriver`, so a live exchange can't be constructed from this crate —
    // `wire_body_fields` is a pure function precisely so the truncation /
    // error wiring is testable against a synthetic `Ok`/`Err` result instead.

    #[test]
    fn wire_body_fields_under_cap_is_not_truncated() {
        let full = b"hello".to_vec();
        let fields = wire_body_fields(Ok(full.clone()), 1024);

        assert_eq!(fields.body.as_deref(), Some("hello"));
        assert_eq!(fields.body_base64, Some(BASE64.encode(&full)));
        assert_eq!(fields.body_truncated, Some(false));
        assert_eq!(fields.body_full_bytes, Some(full.len() as u64));
        assert!(fields.body_capture_error.is_none());
    }

    /// A body larger than `max_bytes` must report `body_truncated: true` and
    /// `body_full_bytes` equal to the FULL pre-truncation length — not the
    /// truncated length — so a caller can tell "captured N of M bytes".
    #[test]
    fn wire_body_fields_over_cap_is_truncated_and_reports_full_length() {
        let full = vec![b'x'; 10_000];
        let max_bytes = 100;
        let fields = wire_body_fields(Ok(full.clone()), max_bytes);

        assert_eq!(fields.body_truncated, Some(true));
        assert_eq!(
            fields.body_full_bytes,
            Some(full.len() as u64),
            "body_full_bytes must be the full length, not the truncated length"
        );
        assert_eq!(
            fields.body.as_ref().map(String::len),
            Some(max_bytes),
            "the captured body prefix must be exactly max_bytes"
        );
        assert_eq!(
            fields.body_base64,
            Some(BASE64.encode(&full[..max_bytes])),
            "body_base64 must be the base64 of the truncated prefix, not the full body"
        );
        assert!(fields.body_capture_error.is_none());
    }

    #[test]
    fn wire_body_fields_zero_max_bytes_is_unbounded() {
        // ASCII (not `0xAB`) so UTF-8-lossy decoding doesn't expand each byte
        // into a multi-byte replacement character — `body.len()` must equal
        // the raw byte count for this assertion to mean anything.
        let full = vec![b'z'; 50_000];
        let fields = wire_body_fields(Ok(full.clone()), 0);

        assert_eq!(fields.body_truncated, Some(false));
        assert_eq!(fields.body_full_bytes, Some(full.len() as u64));
        assert_eq!(fields.body.as_ref().map(String::len), Some(full.len()));
    }

    /// A body-fetch failure (e.g. Chrome already evicted the response body)
    /// must set `body_capture_error` and leave every other field `None` —
    /// distinct from a genuinely empty body, which would set `body: Some("")`
    /// / `body_truncated: Some(false)` instead.
    #[test]
    fn wire_body_fields_fetch_error_sets_capture_error_only() {
        let err = zendriver::ZendriverError::NetworkMonitor("getResponseBody: boom".into());
        let fields = wire_body_fields(Err(err), 1024);

        assert_eq!(
            fields.body_capture_error.as_deref(),
            Some("network monitor: getResponseBody: boom")
        );
        assert!(fields.body.is_none());
        assert!(fields.body_base64.is_none());
        assert!(fields.body_truncated.is_none());
        assert!(fields.body_full_bytes.is_none());
    }

    // ---------- convert_boundary (DeliveryBoundary wire mapping) -----------

    #[test]
    fn convert_boundary_maps_every_variant() {
        let cases = [
            (
                NetworkDeliveryBoundary::Lagged {
                    missed: 3,
                    generation: 1,
                },
                "lagged",
            ),
            (
                NetworkDeliveryBoundary::Reconnected {
                    previous: 1,
                    generation: 2,
                },
                "reconnected",
            ),
            (
                NetworkDeliveryBoundary::Disconnected { generation: 1 },
                "disconnected",
            ),
            (
                NetworkDeliveryBoundary::CorrelationEvicted {
                    url: "https://example.com/evicted".into(),
                },
                "correlation_evicted",
            ),
            (NetworkDeliveryBoundary::DecodeFailed, "decode_failed"),
            (NetworkDeliveryBoundary::Unknown, "unknown"),
        ];
        for (boundary, expected_kind) in cases {
            let wire = convert_boundary(boundary);
            let MonitorEvent::DeliveryBoundary { boundary, .. } = &wire else {
                panic!("expected MonitorEvent::DeliveryBoundary, got {wire:?}");
            };
            assert_eq!(boundary, expected_kind);
        }
    }

    #[test]
    fn convert_boundary_correlation_evicted_carries_url() {
        let wire = convert_boundary(NetworkDeliveryBoundary::CorrelationEvicted {
            url: "https://example.com/evicted".into(),
        });
        let MonitorEvent::DeliveryBoundary { url, .. } = wire else {
            panic!("expected MonitorEvent::DeliveryBoundary");
        };
        assert_eq!(url.as_deref(), Some("https://example.com/evicted"));
    }

    #[test]
    fn convert_boundary_lagged_carries_missed_and_generation() {
        let wire = convert_boundary(NetworkDeliveryBoundary::Lagged {
            missed: 7,
            generation: 4,
        });
        let MonitorEvent::DeliveryBoundary {
            missed, generation, ..
        } = wire
        else {
            panic!("expected MonitorEvent::DeliveryBoundary");
        };
        assert_eq!(missed, Some(7));
        assert_eq!(generation, Some(4));
    }

    // ---------- HttpData chunk wire mapping ---------------------------------

    #[tokio::test]
    async fn convert_maps_http_data_chunk_to_base64() {
        let wire = convert(
            NetworkEvent::HttpData {
                request_id: "r1".into(),
                chunk: b"hello".to_vec(),
            },
            false,
            0,
        )
        .await;
        let MonitorEvent::HttpData {
            request_id,
            chunk_base64,
        } = wire
        else {
            panic!("expected MonitorEvent::HttpData, got {wire:?}");
        };
        assert_eq!(request_id, "r1");
        assert_eq!(chunk_base64, BASE64.encode(b"hello"));
    }

    // ---------- StartInput.stream_bodies wire deserialization ---------------

    #[test]
    fn start_input_stream_bodies_defaults_to_false() {
        let input: StartInput = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!input.stream_bodies);
    }

    #[test]
    fn start_input_stream_bodies_true_deserializes() {
        let input: StartInput =
            serde_json::from_value(serde_json::json!({ "stream_bodies": true })).unwrap();
        assert!(input.stream_bodies);
    }
}
