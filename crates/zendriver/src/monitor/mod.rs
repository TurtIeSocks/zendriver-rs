//! Persistent network monitor: a `Stream<NetworkEvent>` over HTTP exchanges,
//! WebSocket frames, and EventSource messages. Passive (Network domain) —
//! read-only; use the `interception` feature (Fetch domain) to modify requests.
//!
//! HTTP response bodies are whole-body-buffered by default (fetched on demand
//! via [`NetworkExchange::body`]). [`MonitorBuilder::stream_bodies`] opts a
//! monitor into incremental delivery instead: [`NetworkEvent::HttpData`]
//! chunk events, emitted as the response streams in, via the passive CDP
//! `Network.streamResourceContent` mechanism — no `Fetch` interception, no
//! response pausing. See [`MonitorBuilder::stream_bodies`] for the full
//! contract, including its graceful degrade on Chrome versions that don't
//! support it.
//!
//! The correlator subscribes to the connection's loss-accounted event stream
//! ([`zendriver_transport::Connection::subscribe_raw_accounted`]), so a
//! delivery gap (a lagging subscriber, a transport reconnect or disconnect),
//! a correlation-map eviction past [`MAX_TRACKED`], or an undecodable
//! payload is surfaced as an explicit [`NetworkEvent::DeliveryBoundary`]
//! instead of silently stitching a possibly-bogus "complete" exchange,
//! silently dropping an entry, or silently skipping a malformed event. See
//! [`NetworkDeliveryBoundary`] for the full set of boundaries and
//! [`MonitorBuilder::start`] for the `Disconnected` restart contract.

mod events;

use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use zendriver_transport::{AccountedRawEvent, SessionHandle};

use crate::url_matcher::UrlMatcher;
use events::{
    DataReceived, EventSourceMessage, LoadingFailed, RequestIdOnly, RequestWillBeSent,
    ResponseReceived, WebSocketCreated, WebSocketFrameEvent,
};

/// Bounded capacity of the `NetworkMonitor` event channel. Slow consumers
/// apply backpressure on the correlator task once this many events queue.
const CHANNEL_CAP: usize = 1024;

/// Upper bound on the in-flight `requestId → url` correlation maps. A
/// pathological page that opens requests it never finishes must not let the
/// maps grow without limit; past this size one entry is evicted.
const MAX_TRACKED: usize = 10_000;

/// One observed network event emitted by a running `NetworkMonitor`.
///
/// Produced by the correlator task that subscribes to CDP `Network.*` events
/// and assembles them into completed exchanges or per-frame notifications.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// A completed HTTP request/response pair (or a failed request).
    Http(NetworkExchange),
    /// One incrementally-delivered chunk of an HTTP response body, emitted
    /// only when the monitor was started with
    /// [`MonitorBuilder::stream_bodies`]. Sibling to [`Self::WebSocketFrame`]
    /// / [`Self::EventSourceMessage`] — like those, it's delivered as it
    /// arrives rather than accumulated by the correlator.
    ///
    /// `request_id` matches [`NetworkExchange::request_id`] on the eventual
    /// [`Self::Http`] event for the same request, so a consumer can
    /// correlate the streamed chunks with the completed exchange's request
    /// URL / status once it arrives. Chunks may arrive before, interleaved
    /// with, or after other events for unrelated requests — but always
    /// before the matching `Http` event, since that only fires on
    /// `loadingFinished` / `loadingFailed`.
    HttpData {
        /// The CDP `requestId` this chunk belongs to.
        request_id: String,
        /// The chunk's raw bytes, in arrival order relative to other chunks
        /// for the same `request_id`.
        chunk: Vec<u8>,
    },
    /// A new WebSocket connection was opened.
    WebSocketOpen {
        /// The CDP request ID for this WebSocket connection.
        request_id: String,
        /// The WebSocket URL.
        url: String,
    },
    /// A WebSocket frame was sent or received.
    WebSocketFrame {
        /// The CDP request ID for the owning WebSocket connection.
        request_id: String,
        /// Whether the frame was sent by the page or received from the server.
        direction: FrameDirection,
        /// WebSocket opcode (1 = text, 2 = binary, 8 = close, …).
        opcode: u8,
        /// Frame payload (text frames as UTF-8; binary frames as base64).
        payload: String,
    },
    /// A WebSocket connection was closed.
    WebSocketClose {
        /// The CDP request ID for the closed WebSocket connection.
        request_id: String,
    },
    /// An SSE `EventSource` message was received.
    EventSourceMessage {
        /// The CDP request ID for the `EventSource` stream.
        request_id: String,
        /// The SSE `event:` field (empty string if omitted).
        event_name: String,
        /// The SSE `id:` field (empty string if omitted).
        event_id: String,
        /// The SSE `data:` payload.
        data: String,
    },
    /// A delivery-loss boundary on the monitor's underlying event stream (or
    /// its own correlation bookkeeping) — see [`NetworkDeliveryBoundary`].
    ///
    /// Additive: a consumer that ignores this variant still sees every
    /// fully-observed exchange exactly as before. What it loses is the
    /// ability to tell "nothing happened" apart from "something was lost and
    /// I never heard about it" — which is exactly the silent failure mode
    /// this variant replaces.
    DeliveryBoundary(NetworkDeliveryBoundary),
}

/// A delivery-loss boundary observed on the monitor's underlying
/// loss-accounted CDP event stream, or in its own in-memory correlation
/// bookkeeping.
///
/// [`NetworkEvent::Http`] exchanges are assembled by correlating
/// `requestWillBeSent` → `responseReceived` → `loadingFinished` /
/// `loadingFailed` by `requestId`. If any leg of that correlation is lost —
/// a broadcast-bus lag, a transport reconnect or disconnect, a
/// correlation-map eviction, or an undecodable payload — an exchange
/// assembled across the gap would be silently wrong: a bogus "complete"
/// exchange missing its response, or worse, a response glued onto an
/// unrelated later request that reused the same `requestId`. Every such gap
/// now surfaces as one of these variants instead, and the correlator clears
/// its in-flight state on `Lagged` / `Reconnected` / `Disconnected` /
/// `Unknown` so a partial exchange spanning the gap is never emitted as
/// complete.
///
/// `DeliveryBoundary` events bypass any [`MonitorBuilder::url_pattern`]
/// filter — they describe the monitor's own health, not a specific
/// exchange, so a filter that would exclude the affected URL must not hide
/// the fact that data was lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkDeliveryBoundary {
    /// This monitor's subscription to the connection's accounted event bus
    /// fell behind and `missed` events were overwritten before the
    /// correlator could process them. Any HTTP exchange in flight when this
    /// fires is unrecoverable — its correlation state is cleared rather than
    /// risk emitting a possibly-bogus completed [`NetworkEvent::Http`].
    Lagged {
        /// Number of events this subscription missed.
        missed: u64,
        /// Connection generation active when the loss was detected.
        generation: u64,
    },
    /// The underlying transport re-established a fresh WebSocket
    /// (`Connection::reconnect`). All in-flight correlation state from the
    /// previous connection is cleared — a `loadingFinished` for a request
    /// seen before the reconnect will never arrive on the new socket.
    Reconnected {
        /// Generation of the connection actor that was replaced.
        previous: u64,
        /// Generation of the newly established connection.
        generation: u64,
    },
    /// The underlying transport's WebSocket died unexpectedly (not a caller
    /// requested shutdown or reconnect). The monitor task ends immediately
    /// after emitting this event — fail closed, per [`MonitorBuilder::start`].
    /// A consumer must start a new monitor to resume observing after the
    /// transport recovers; this one will never emit again.
    Disconnected {
        /// Generation whose WebSocket died.
        generation: u64,
    },
    /// The in-flight correlation map exceeded [`MAX_TRACKED`] and one entry
    /// was evicted to bound memory. `url` is the evicted entry's request (or
    /// WebSocket connection) URL. Previously this happened with only a
    /// `tracing` warning; the eviction is now also observable on the event
    /// stream.
    CorrelationEvicted {
        /// URL of the evicted in-flight exchange.
        url: String,
    },
    /// A CDP event's payload could not be decoded into the shape the
    /// correlator expected for its `method`. Previously the event was
    /// silently skipped; the failure is now surfaced. The raw undecodable
    /// payload is intentionally never included here — only the fact that
    /// something was lost.
    DecodeFailed,
    /// A conservative default for any future
    /// [`AccountedRawEvent`](zendriver_transport::AccountedRawEvent) variant
    /// this correlator doesn't yet know how to interpret. Treated like a
    /// delivery gap — correlation state is cleared — but unlike
    /// [`Self::Disconnected`] the monitor task keeps running, since an
    /// unrecognized variant isn't known to mean the transport is gone.
    Unknown,
}

/// Direction of a WebSocket frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDirection {
    /// Frame sent by the page to the server.
    Sent,
    /// Frame received by the page from the server.
    Received,
}

/// The request half of a completed HTTP exchange.
#[derive(Debug, Clone)]
pub struct MonitoredRequest {
    /// The full request URL.
    pub url: String,
    /// HTTP method (e.g. `"GET"`, `"POST"`).
    pub method: String,
    /// Request headers as sent.
    pub headers: HashMap<String, String>,
    /// Request body for POST/PUT requests, if present.
    pub post_data: Option<String>,
}

/// The response half of a completed HTTP exchange.
#[derive(Debug, Clone)]
pub struct MonitoredResponse {
    /// HTTP status code.
    pub status: u16,
    /// HTTP status text (e.g. `"OK"`, `"Not Found"`).
    pub status_text: String,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// MIME type reported by Chrome (e.g. `"application/json"`).
    pub mime_type: String,
}

/// A completed HTTP request/response pair observed by the network monitor.
///
/// The `session` field is `pub(crate)` and excluded from the `Debug` impl
/// because `SessionHandle` does not implement `Debug`. Body bytes are fetched
/// on demand via [`Self::body`] / [`Self::text`].
#[derive(Clone)]
pub struct NetworkExchange {
    /// The observed request.
    pub request: MonitoredRequest,
    /// The response, if one was received before the request finished.
    pub response: Option<MonitoredResponse>,
    /// Network-level error text, if the request failed (`loadingFailed`).
    pub error: Option<String>,
    /// CDP `requestId` — used by `body()` / `text()` to call `getResponseBody`.
    pub(crate) request_id: String,
    /// Session handle used to issue `getResponseBody` CDP calls.
    pub(crate) session: SessionHandle,
}

impl std::fmt::Debug for NetworkExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkExchange")
            .field("request", &self.request)
            .field("response", &self.response)
            .field("error", &self.error)
            .finish()
    }
}

impl NetworkExchange {
    /// Returns the CDP `requestId` this exchange was correlated by.
    ///
    /// Matches the `request_id` on [`NetworkEvent::HttpData`] events for the
    /// same request when the monitor was started with
    /// [`MonitorBuilder::stream_bodies`] — chunk events arrive (and must be
    /// correlated) before this exchange completes, since `HttpData` carries
    /// no URL of its own.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Returns the HTTP status code of the response, if one was received.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|r| r.status)
    }

    /// Returns `true` if the response has a 2xx status code.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self.status(), Some(s) if (200..300).contains(&s))
    }

    /// Fetch the response body on demand via `Network.getResponseBody`.
    ///
    /// Per CDP the result carries a `body` string plus a `base64Encoded: bool`
    /// flag: when true the bytes are base64-decoded; when false the UTF-8 bytes
    /// are returned verbatim. Mirrors [`crate::expect::response::MatchedResponse::body`].
    ///
    /// Chrome only retains response bodies for a short window after the
    /// response completes, so call this promptly after observing the
    /// [`NetworkEvent::Http`] exchange.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ZendriverError::NetworkMonitor`] if Chrome rejected the
    /// `getResponseBody` call (e.g. the body is no longer retained) or returned
    /// invalid base64.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let mut monitor = tab.monitor().start().await?;
    /// while let Some(event) = monitor.next().await {
    ///     if let zendriver::NetworkEvent::Http(exchange) = event {
    ///         let bytes = exchange.body().await?;
    ///         println!("{} bytes", bytes.len());
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn body(&self) -> crate::Result<Vec<u8>> {
        let res = self
            .session
            .call(
                "Network.getResponseBody",
                serde_json::json!({ "requestId": self.request_id }),
            )
            .await
            .map_err(|e| crate::ZendriverError::NetworkMonitor(format!("getResponseBody: {e}")))?;
        let body = res
            .get("body")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if res
            .get("base64Encoded")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            BASE64
                .decode(body)
                .map_err(|e| crate::ZendriverError::NetworkMonitor(format!("base64: {e}")))
        } else {
            Ok(body.as_bytes().to_vec())
        }
    }

    /// Fetch the response body and decode it as UTF-8 (lossily).
    ///
    /// # Errors
    ///
    /// Propagates any error from [`Self::body`].
    pub async fn text(&self) -> crate::Result<String> {
        Ok(String::from_utf8_lossy(&self.body().await?).into_owned())
    }
}

/// Builder for a [`NetworkMonitor`]. Configure an optional URL filter, then
/// call [`Self::start`] to spawn the correlator task.
///
/// Obtained via [`crate::Tab::monitor`].
pub struct MonitorBuilder {
    session: SessionHandle,
    url_pattern: Option<UrlMatcher>,
    stream_bodies: bool,
}

impl MonitorBuilder {
    /// Construct a builder over `session`. Crate-internal — callers use
    /// [`crate::Tab::monitor`].
    pub(crate) fn new(session: SessionHandle) -> Self {
        Self {
            session,
            url_pattern: None,
            stream_bodies: false,
        }
    }

    /// Restrict emitted events to those whose URL matches `pattern`.
    ///
    /// Accepts anything convertible into a [`UrlMatcher`]: a `&str` / `String`
    /// (substring match) or a `regex::Regex`. For HTTP exchanges the request
    /// URL is matched; for WebSocket / EventSource events the connection URL
    /// observed at open time is matched.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// // Only surface requests whose URL contains "/api/".
    /// let monitor = tab.monitor().url_pattern("/api/").start().await?;
    /// # let _ = monitor;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn url_pattern(mut self, pattern: impl Into<UrlMatcher>) -> Self {
        self.url_pattern = Some(pattern.into());
        self
    }

    /// Opt in to incremental HTTP response body delivery. Default `false`.
    ///
    /// When `true`, the correlator enables CDP `Network.streamResourceContent`
    /// for each request that reaches `requestWillBeSent` and passes the
    /// [`Self::url_pattern`] filter (there is no separate filter for
    /// streaming — narrow `url_pattern` to narrow what streams). Enabling
    /// this early — before the request is even sent, rather than waiting for
    /// `responseReceived` — wins the race against a fast (especially
    /// loopback) response's `loadingFinished`, past which Chrome rejects the
    /// enable call; `bufferedData` on the enable reply covers whatever bytes
    /// arrived in the meantime either way, so nothing is lost by enabling
    /// early. Each received chunk is emitted as a [`NetworkEvent::HttpData`]
    /// on the monitor's stream, interleaved with the other event kinds; any
    /// bytes buffered before the enable call lands are emitted as the first
    /// chunk. This is passive — no `Fetch` domain interception, no response
    /// pausing — unlike `Fetch.takeResponseBodyAsStream`, which was rejected
    /// for exactly that reason (see the module docs).
    ///
    /// `Network.streamResourceContent` requires roughly Chrome 124+. On an
    /// older Chrome the enable call errors; the correlator logs one
    /// `tracing::warn!` and skips streaming for that request — the monitor
    /// never fails, and [`NetworkExchange::body`] keeps working as the
    /// whole-body fallback. Leave this `false` (the default) if every
    /// response body is small enough that whole-body buffering is fine —
    /// streaming every response is wasted CDP round-trips otherwise.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let mut monitor = tab.monitor().stream_bodies(true).start().await?;
    /// while let Some(event) = monitor.next().await {
    ///     if let zendriver::NetworkEvent::HttpData { request_id, chunk } = event {
    ///         println!("{request_id}: {} bytes", chunk.len());
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn stream_bodies(mut self, enabled: bool) -> Self {
        self.stream_bodies = enabled;
        self
    }

    /// Spawn the correlator task and return a live [`NetworkMonitor`].
    ///
    /// The task subscribes to the session's loss-accounted CDP event stream
    /// and runs until the monitor is dropped, [`NetworkMonitor::stop`] is
    /// called, or — **fail closed** — the underlying transport reports
    /// [`NetworkDeliveryBoundary::Disconnected`]. In the `Disconnected` case
    /// the task emits that boundary event and then ends; the stream returns
    /// `None` on the next poll. There is no automatic reconnect: a consumer
    /// that wants to keep observing across a transport blip must call
    /// [`crate::Tab::monitor`]`().start()` again to spawn a fresh correlator.
    ///
    /// # Errors
    ///
    /// Currently infallible, but returns [`crate::Result`] so future setup
    /// (e.g. an explicit `Network.enable` round-trip) can surface errors
    /// without an API break.
    pub async fn start(self) -> crate::Result<NetworkMonitor> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAP);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_monitor(
            self.session,
            self.url_pattern,
            self.stream_bodies,
            tx,
            cancel.clone(),
        ));
        Ok(NetworkMonitor {
            rx,
            cancel,
            _task: task,
        })
    }
}

/// A live network monitor. Implements [`Stream`]`<Item = `[`NetworkEvent`]`>`.
///
/// Poll it (e.g. via [`futures::StreamExt::next`]) to receive observed events.
/// Dropping the monitor — or calling [`Self::stop`] — cancels the background
/// correlator task.
pub struct NetworkMonitor {
    rx: mpsc::Receiver<NetworkEvent>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl NetworkMonitor {
    /// Stop the monitor, cancelling its background correlator task.
    ///
    /// Equivalent to dropping the monitor, but consumes `self` for an explicit
    /// teardown point.
    pub fn stop(self) {
        self.cancel.cancel();
    }
}

impl Drop for NetworkMonitor {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Stream for NetworkMonitor {
    type Item = NetworkEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<NetworkEvent>> {
        self.rx.poll_recv(cx)
    }
}

/// In-flight correlation state: the request half plus the response half once
/// `responseReceived` arrives. Completed on `loadingFinished` / `loadingFailed`.
type PartialExchange = (MonitoredRequest, Option<MonitoredResponse>);

/// The correlator task. Drives a single loss-accounted CDP event subscription,
/// dispatching by `method` and correlating `requestId`s into completed
/// [`NetworkExchange`]s plus WebSocket / EventSource notifications.
///
/// A single subscription (rather than several typed subscriptions in a
/// `tokio::select!`) is deliberate: `select!` picks a ready arm at random and
/// can deliver `loadingFinished` before the matching `requestWillBeSent`,
/// dropping the exchange. One stream preserves CDP's wire order — this mirrors
/// [`crate::network_idle`].
///
/// Riding [`zendriver_transport::Connection::subscribe_raw_accounted`]
/// (rather than the plain `subscribe_raw`) is what makes delivery loss
/// observable at all: a lag, reconnect, or disconnect is reported as an
/// explicit [`AccountedRawEvent`] instead of vanishing. See
/// [`NetworkDeliveryBoundary`] for how each case is handled.
async fn run_monitor(
    session: SessionHandle,
    filter: Option<UrlMatcher>,
    stream_bodies: bool,
    tx: mpsc::Sender<NetworkEvent>,
    cancel: CancellationToken,
) {
    let session_id = session.session_id().to_string();
    // Shared across every spawned `streamResourceContent` enable task so an
    // unsupported-Chrome error is logged once for the whole monitor, not once
    // per streamed request (every subsequent call would fail identically).
    let warned_stream_unsupported = Arc::new(AtomicBool::new(false));
    // Subscribe to the accounted event stream BEFORE issuing `Network.enable`:
    // the accounted bus is a `broadcast` receiver that only sees frames sent
    // after it registers, so any event Chrome fires between `enable`'s reply
    // and our registration would be lost. Subscribing first plugs that race
    // (and gives tests a deterministic `expect_cmd("Network.enable")` sync
    // point — see `network_idle.rs`, which mirrors this ordering).
    //
    // ONE subscription, dispatched by method — preserves CDP wire order; a
    // typed `select!` could reorder `loadingFinished` ahead of the matching
    // `requestWillBeSent` and drop the exchange.
    let mut events = session.connection().subscribe_raw_accounted();
    let mut partial: HashMap<String, PartialExchange> = HashMap::new();
    // requestId → url, kept so frame/close/SSE events (which omit the URL) can
    // still be matched against `filter`.
    let mut urls: HashMap<String, String> = HashMap::new();
    // Insertion order of tracked requestIds, so an over-cap eviction drops the
    // OLDEST tracked request (the one most likely abandoned) rather than a
    // hash-arbitrary one. Holds lazy tombstones for ids already removed by
    // normal completion; compacted when it drifts past the live cap so it
    // can't grow without bound on a long-lived page.
    let mut order: VecDeque<String> = VecDeque::new();
    // requestIds that already had `Network.streamResourceContent` enabled
    // (only populated when `stream_bodies` is set). Guards against
    // re-enabling on a stray duplicate `requestWillBeSent` (e.g. a redirect
    // hop reusing the same requestId), and is pruned on `loadingFinished` /
    // `loadingFailed` alongside `urls` / `partial` so it can't grow without
    // bound either — no accumulated bytes live here, just the fact that
    // streaming was already turned on for this id.
    let mut streaming: HashSet<String> = HashSet::new();

    // Fire-and-forget `Network.enable` so the monitor works on its own even if
    // nothing else enabled the domain. We don't await the reply: the mock test
    // harness never replies, and our subscription above is already live either
    // way. A failure (e.g. session torn down) just means no events arrive.
    let enable_session = session.clone();
    tokio::spawn(async move {
        if let Err(e) = enable_session
            .call("Network.enable", serde_json::json!({}))
            .await
        {
            warn!(error = %e, "network monitor: Network.enable failed; events may be inactive");
        }
    });

    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            next = events.next() => {
                let Some(acc) = next else { return };
                match acc {
                    AccountedRawEvent::Event { event: ev, .. } => {
                        if ev.session_id.as_deref() != Some(session_id.as_str()) {
                            continue;
                        }
                        match ev.method.as_str() {
                            "Network.requestWillBeSent" => {
                                let Ok(p) = serde_json::from_value::<RequestWillBeSent>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                urls.insert(p.request_id.clone(), p.request.url.clone());
                                track_order(&mut order, &urls, p.request_id.clone());
                                if urls.len() > MAX_TRACKED {
                                    let evicted = evict_oldest(&mut urls, &mut partial, &mut order);
                                    if let Some(url) = evicted {
                                        if emit_boundary(&tx, NetworkDeliveryBoundary::CorrelationEvicted { url })
                                            .await
                                        {
                                            return;
                                        }
                                    }
                                }
                                // Enable streaming as early as possible — right
                                // when the request is first observed, before
                                // it's even sent on the wire — rather than
                                // waiting for `responseReceived`. Empirically
                                // (real-Chrome testing against a local mock
                                // server), `responseReceived` leaves too
                                // little headroom: `Network.streamResourceContent`
                                // is a CDP round-trip via a spawned task, and
                                // Chrome rejects it with `-32602 Request with
                                // the provided ID has already finished
                                // loading` for a fast (especially loopback)
                                // response that reaches `loadingFinished`
                                // before that call lands. Enabling this early
                                // costs nothing extra: `bufferedData` on the
                                // enable reply still covers whatever bytes
                                // arrived in the meantime, so no data is lost
                                // either way — only the odds of winning the
                                // race improve.
                                if stream_bodies
                                    && !warned_stream_unsupported.load(Ordering::Relaxed)
                                    && !streaming.contains(&p.request_id)
                                    && filter_allows(filter.as_ref(), Some(p.request.url.as_str()))
                                {
                                    // Once one `streamResourceContent` call has
                                    // failed (old Chrome / unsupported), skip
                                    // future enable attempts — the whole-body
                                    // `body()` path is the fallback, and there's
                                    // no point re-spending a doomed CDP round-trip
                                    // per request. Matches the warn message.
                                    streaming.insert(p.request_id.clone());
                                    spawn_stream_resource_content(
                                        &session,
                                        &tx,
                                        &warned_stream_unsupported,
                                        p.request_id.clone(),
                                    );
                                }
                                let req = MonitoredRequest {
                                    url: p.request.url,
                                    method: p.request.method,
                                    headers: p.request.headers,
                                    post_data: p.request.post_data,
                                };
                                partial.insert(p.request_id, (req, None));
                            }
                            "Network.responseReceived" => {
                                let Ok(p) = serde_json::from_value::<ResponseReceived>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                if let Some(entry) = partial.get_mut(&p.request_id) {
                                    entry.1 = Some(MonitoredResponse {
                                        status: p.response.status,
                                        status_text: p.response.status_text,
                                        headers: p.response.headers,
                                        mime_type: p.response.mime_type,
                                    });
                                }
                            }
                            "Network.dataReceived" if stream_bodies => {
                                let Ok(p) = serde_json::from_value::<DataReceived>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                // `data` is populated only when streaming was
                                // enabled for this requestId — absent for the
                                // vast majority of `dataReceived` events
                                // (Chrome fires it as progress info for every
                                // in-flight request regardless).
                                let Some(data) = p.data else { continue };
                                match BASE64.decode(&data) {
                                    Ok(chunk) => {
                                        if tx
                                            .send(NetworkEvent::HttpData { request_id: p.request_id, chunk })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    Err(_) => {
                                        if emit_decode_failed(&tx).await {
                                            return;
                                        }
                                    }
                                }
                            }
                            "Network.loadingFinished" => {
                                let Ok(p) = serde_json::from_value::<RequestIdOnly>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                if let Some((req, resp)) = partial.remove(&p.request_id) {
                                    if filter_allows(filter.as_ref(), Some(&req.url)) {
                                        let exchange = NetworkExchange {
                                            request: req,
                                            response: resp,
                                            error: None,
                                            request_id: p.request_id.clone(),
                                            session: session.clone(),
                                        };
                                        if tx.send(NetworkEvent::Http(exchange)).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                urls.remove(&p.request_id);
                                streaming.remove(&p.request_id);
                            }
                            "Network.loadingFailed" => {
                                let Ok(p) = serde_json::from_value::<LoadingFailed>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                if let Some((req, resp)) = partial.remove(&p.request_id) {
                                    if filter_allows(filter.as_ref(), Some(&req.url)) {
                                        let exchange = NetworkExchange {
                                            request: req,
                                            response: resp,
                                            error: Some(p.error_text),
                                            request_id: p.request_id.clone(),
                                            session: session.clone(),
                                        };
                                        if tx.send(NetworkEvent::Http(exchange)).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                urls.remove(&p.request_id);
                                streaming.remove(&p.request_id);
                            }
                            "Network.webSocketCreated" => {
                                let Ok(p) = serde_json::from_value::<WebSocketCreated>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                urls.insert(p.request_id.clone(), p.url.clone());
                                track_order(&mut order, &urls, p.request_id.clone());
                                if urls.len() > MAX_TRACKED {
                                    let evicted = evict_oldest(&mut urls, &mut partial, &mut order);
                                    if let Some(url) = evicted {
                                        if emit_boundary(&tx, NetworkDeliveryBoundary::CorrelationEvicted { url })
                                            .await
                                        {
                                            return;
                                        }
                                    }
                                }
                                if filter_allows(filter.as_ref(), Some(&p.url))
                                    && tx
                                        .send(NetworkEvent::WebSocketOpen {
                                            request_id: p.request_id,
                                            url: p.url,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                            }
                            "Network.webSocketFrameSent" | "Network.webSocketFrameReceived" => {
                                let direction = if ev.method.ends_with("Sent") {
                                    FrameDirection::Sent
                                } else {
                                    FrameDirection::Received
                                };
                                let Ok(p) = serde_json::from_value::<WebSocketFrameEvent>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                if filter_allows(filter.as_ref(), urls.get(&p.request_id).map(String::as_str))
                                    && tx
                                        .send(NetworkEvent::WebSocketFrame {
                                            request_id: p.request_id,
                                            direction,
                                            opcode: p.response.opcode,
                                            payload: p.response.payload_data,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                            }
                            "Network.webSocketClosed" => {
                                let Ok(p) = serde_json::from_value::<RequestIdOnly>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                if filter_allows(filter.as_ref(), urls.get(&p.request_id).map(String::as_str))
                                    && tx
                                        .send(NetworkEvent::WebSocketClose {
                                            request_id: p.request_id.clone(),
                                        })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                urls.remove(&p.request_id);
                            }
                            "Network.eventSourceMessageReceived" => {
                                let Ok(p) = serde_json::from_value::<EventSourceMessage>(ev.params) else {
                                    if emit_decode_failed(&tx).await {
                                        return;
                                    }
                                    continue;
                                };
                                if filter_allows(filter.as_ref(), urls.get(&p.request_id).map(String::as_str))
                                    && tx
                                        .send(NetworkEvent::EventSourceMessage {
                                            request_id: p.request_id,
                                            event_name: p.event_name,
                                            event_id: p.event_id,
                                            data: p.data,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                            }
                            _ => {}
                        }
                    }
                    AccountedRawEvent::Lagged { generation, missed } => {
                        // A partial exchange spanning this gap can never be
                        // proven complete — clear it rather than risk
                        // stitching a bogus "complete" `Http` across the
                        // loss.
                        partial.clear();
                        urls.clear();
                        streaming.clear();
                        if emit_boundary(&tx, NetworkDeliveryBoundary::Lagged { missed, generation }).await {
                            return;
                        }
                    }
                    // coverage: exercised end-to-end by
                    // `reconnected_mid_exchange_clears_partial_and_emits_boundary`
                    // (its own arm, structurally mirroring `Lagged` above) —
                    // keep that test if this arm's handling ever diverges from
                    // `Lagged` (e.g. reconnect-specific resume semantics).
                    AccountedRawEvent::Reconnected { previous, generation } => {
                        partial.clear();
                        urls.clear();
                        streaming.clear();
                        if emit_boundary(&tx, NetworkDeliveryBoundary::Reconnected { previous, generation }).await {
                            return;
                        }
                    }
                    AccountedRawEvent::Disconnected { generation } => {
                        partial.clear();
                        urls.clear();
                        streaming.clear();
                        // Fail closed: report the boundary, then end the
                        // task regardless of whether the send succeeded —
                        // the transport is gone either way, so there is
                        // nothing more this correlator can observe. A
                        // consumer that wants to keep watching must start a
                        // fresh monitor.
                        let _ = emit_boundary(&tx, NetworkDeliveryBoundary::Disconnected { generation }).await;
                        return;
                    }
                    // Conservative default for any future `AccountedRawEvent`
                    // variant. `AccountedRawEvent` has exactly the four
                    // variants above today, so this arm is unreachable until
                    // the transport crate adds one — kept so that addition
                    // is a silent (if degraded) fallback here instead of a
                    // compile break.
                    #[allow(unreachable_patterns)]
                    _ => {
                        partial.clear();
                        urls.clear();
                        streaming.clear();
                        if emit_boundary(&tx, NetworkDeliveryBoundary::Unknown).await {
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Send a [`NetworkEvent::DeliveryBoundary`] wrapping `boundary`. Returns
/// `true` if the receiver was dropped, signalling the caller should end the
/// task.
async fn emit_boundary(tx: &mpsc::Sender<NetworkEvent>, boundary: NetworkDeliveryBoundary) -> bool {
    tx.send(NetworkEvent::DeliveryBoundary(boundary))
        .await
        .is_err()
}

/// Send a [`NetworkDeliveryBoundary::DecodeFailed`] boundary. The caller's
/// `else` branch on a failed `serde_json::from_value` never has the raw
/// payload in scope by the time this is called — nothing to leak.
async fn emit_decode_failed(tx: &mpsc::Sender<NetworkEvent>) -> bool {
    emit_boundary(tx, NetworkDeliveryBoundary::DecodeFailed).await
}

/// Enable `Network.streamResourceContent` for `request_id` in a spawned
/// task, mirroring the correlator's existing fire-and-forget
/// `Network.enable` call: the CDP round-trip must not block the main
/// dispatch loop from processing the next event in wire order.
///
/// On success, any `bufferedData` (bytes Chrome already had before the
/// enable call landed) is emitted as the first [`NetworkEvent::HttpData`]
/// chunk for this request — so nothing received in that pre-enable window is
/// lost. On failure (old Chrome / unsupported), logs one `tracing::warn!`
/// via `warned` (shared across every call this monitor makes, so the warning
/// fires once per monitor rather than once per request) and otherwise does
/// nothing — the monitor keeps running and [`NetworkExchange::body`] remains
/// the working fallback for this request.
fn spawn_stream_resource_content(
    session: &SessionHandle,
    tx: &mpsc::Sender<NetworkEvent>,
    warned: &Arc<AtomicBool>,
    request_id: String,
) {
    let session = session.clone();
    let tx = tx.clone();
    let warned = Arc::clone(warned);
    tokio::spawn(async move {
        match session
            .call(
                "Network.streamResourceContent",
                serde_json::json!({ "requestId": request_id }),
            )
            .await
        {
            Ok(res) => {
                let buffered = res
                    .get("bufferedData")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if buffered.is_empty() {
                    return;
                }
                if let Ok(chunk) = BASE64.decode(buffered) {
                    if !chunk.is_empty() {
                        let _ = tx.send(NetworkEvent::HttpData { request_id, chunk }).await;
                    }
                }
            }
            Err(e) => {
                if !warned.swap(true, Ordering::Relaxed) {
                    warn!(
                        error = %e,
                        "network monitor: Network.streamResourceContent failed (needs Chrome ~124+); \
                         stream_bodies falling back to whole-body capture for this and future requests"
                    );
                }
            }
        }
    });
}

/// Apply the optional URL filter. With no filter every event passes. With a
/// filter, an event passes only if its URL is known and matches; events whose
/// URL we never observed (e.g. a frame for an evicted connection) are dropped.
fn filter_allows(filter: Option<&UrlMatcher>, url: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(m) => url.is_some_and(|u| m.matches(u)),
    }
}

/// Record a newly-inserted `requestId` in insertion order for FIFO eviction.
///
/// Appends to `order`, then — only when the deque has drifted past twice the
/// live cap — drops tombstones (ids already removed from `urls` by normal
/// completion) so `order` stays bounded on a long-lived page that never
/// exceeds the concurrent cap. The compaction preserves relative order and is
/// amortized O(1) (it runs at most once per `MAX_TRACKED` insertions).
fn track_order(order: &mut VecDeque<String>, urls: &HashMap<String, String>, id: String) {
    order.push_back(id);
    if order.len() > MAX_TRACKED * 2 {
        order.retain(|k| urls.contains_key(k));
    }
}

/// Evict the insertion-oldest still-live entry from the correlation maps once
/// they exceed [`MAX_TRACKED`]. Bounds memory against a pathological page that
/// opens requests it never finishes. Returns the evicted entry's URL so the
/// caller can emit [`NetworkDeliveryBoundary::CorrelationEvicted`] — this
/// eviction used to be silent (a `tracing` warning only); it no longer is.
///
/// FIFO by insertion order (via `order`): the oldest still-tracked request is
/// the one most likely abandoned, so dropping it targets the actual leak
/// source rather than a hash-arbitrary victim (which could drop a request
/// about to complete while a stuck one survives). Removing the id from `urls`
/// drops both the entry and its mirrored `partial` row in one pass. `order`
/// carries lazy tombstones for ids already removed by normal completion; they
/// are skipped here. Returns `None` only when no live entry remains (a
/// defensive no-op — the caller only invokes this once
/// `urls.len() > MAX_TRACKED`, so this shouldn't occur in practice).
fn evict_oldest(
    urls: &mut HashMap<String, String>,
    partial: &mut HashMap<String, PartialExchange>,
    order: &mut VecDeque<String>,
) -> Option<String> {
    while let Some(id) = order.pop_front() {
        if let Some(url) = urls.remove(&id) {
            partial.remove(&id);
            warn!("network monitor correlation map exceeded {MAX_TRACKED}; evicting oldest entry");
            return Some(url);
        }
        // Tombstone: `id` was already removed by normal completion — skip it.
    }
    None
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    use super::*;

    const SID: &str = "S1";

    /// Spawn a monitor over a fresh mock session and return the live monitor
    /// plus the mock (to emit events) and connection (to shut down).
    ///
    /// Crucially, this awaits the correlator's fire-and-forget
    /// `Network.enable` command before returning. `subscribe_raw_accounted` is
    /// a `broadcast` receiver that only sees frames sent after it registers,
    /// and the correlator subscribes *before* issuing `Network.enable` — so
    /// once that command lands the subscription is guaranteed live, and any
    /// event the test emits afterwards is observed. This mirrors the
    /// `network_idle` harness's `expect_cmd("Network.enable")`
    /// synchronization.
    async fn spawn_monitor(
        filter: Option<UrlMatcher>,
    ) -> (
        NetworkMonitor,
        MockConnection,
        zendriver_transport::Connection,
    ) {
        spawn_monitor_with(MockConnection::pair(), filter).await
    }

    /// Like [`spawn_monitor`], but over a caller-supplied `(MockConnection,
    /// Connection)` pair — lets a test force a deterministic `Lagged`
    /// boundary via [`MockConnection::pair_with_accounted_capacity`] while
    /// still getting the same `Network.enable` synchronization.
    async fn spawn_monitor_with(
        pair: (MockConnection, zendriver_transport::Connection),
        filter: Option<UrlMatcher>,
    ) -> (
        NetworkMonitor,
        MockConnection,
        zendriver_transport::Connection,
    ) {
        let (mut mock, conn) = pair;
        let session = SessionHandle::new(conn.clone(), SID);
        let mut builder = MonitorBuilder::new(session);
        if let Some(f) = filter {
            builder = builder.url_pattern(f);
        }
        let monitor = builder.start().await.unwrap();
        // Synchronize: the correlator subscribed before sending this.
        let id = mock.expect_cmd("Network.enable").await;
        mock.reply(id, json!({})).await;
        (monitor, mock, conn)
    }

    /// Await the next emitted event, failing if none arrives within 2s. The
    /// correlator task is async, so a bare `try_recv` would race the spawn.
    async fn next_event(monitor: &mut NetworkMonitor) -> NetworkEvent {
        tokio::time::timeout(Duration::from_secs(2), monitor.next())
            .await
            .expect("timed out waiting for a NetworkEvent")
            .expect("monitor stream ended unexpectedly")
    }

    /// Assert that no event arrives within a short window (negative case).
    async fn assert_no_event(monitor: &mut NetworkMonitor) {
        let res = tokio::time::timeout(Duration::from_millis(300), monitor.next()).await;
        assert!(res.is_err(), "expected no event, got {res:?}");
    }

    #[tokio::test]
    async fn http_request_correlates_to_one_exchange() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        // requestWillBeSent -> responseReceived -> loadingFinished for one id.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "1",
                "request": {
                    "url": "https://example.com/api/users",
                    "method": "GET",
                    "headers": { "Accept": "application/json" }
                }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({
                "requestId": "1",
                "response": {
                    "status": 200,
                    "statusText": "OK",
                    "mimeType": "application/json"
                }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "1" }), SID)
            .await;

        let event = next_event(&mut monitor).await;
        let NetworkEvent::Http(exchange) = event else {
            panic!("expected NetworkEvent::Http, got {event:?}");
        };
        assert_eq!(exchange.request.url, "https://example.com/api/users");
        assert_eq!(exchange.request.method, "GET");
        assert_eq!(exchange.status(), Some(200));
        assert!(exchange.is_success());
        assert!(exchange.error.is_none());

        monitor.stop();
        conn.shutdown();
    }

    #[tokio::test]
    async fn loading_failed_emits_error_exchange() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "7",
                "request": { "url": "https://example.com/boom", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.loadingFailed",
            json!({ "requestId": "7", "errorText": "net::ERR_ABORTED" }),
            SID,
        )
        .await;

        let event = next_event(&mut monitor).await;
        let NetworkEvent::Http(exchange) = event else {
            panic!("expected NetworkEvent::Http, got {event:?}");
        };
        assert_eq!(exchange.request.url, "https://example.com/boom");
        assert!(exchange.response.is_none());
        assert_eq!(exchange.status(), None);
        assert_eq!(exchange.error.as_deref(), Some("net::ERR_ABORTED"));

        monitor.stop();
        conn.shutdown();
    }

    #[tokio::test]
    async fn ws_frames_emit_tagged_events() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        mock.emit_event_for_session(
            "Network.webSocketCreated",
            json!({ "requestId": "ws1", "url": "wss://echo.example.com/socket" }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.webSocketFrameSent",
            json!({ "requestId": "ws1", "response": { "opcode": 1, "payloadData": "ping" } }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.webSocketFrameReceived",
            json!({ "requestId": "ws1", "response": { "opcode": 1, "payloadData": "pong" } }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.webSocketClosed",
            json!({ "requestId": "ws1" }),
            SID,
        )
        .await;

        // Open
        match next_event(&mut monitor).await {
            NetworkEvent::WebSocketOpen { request_id, url } => {
                assert_eq!(request_id, "ws1");
                assert_eq!(url, "wss://echo.example.com/socket");
            }
            other => panic!("expected WebSocketOpen, got {other:?}"),
        }
        // Sent frame
        match next_event(&mut monitor).await {
            NetworkEvent::WebSocketFrame {
                request_id,
                direction,
                opcode,
                payload,
            } => {
                assert_eq!(request_id, "ws1");
                assert_eq!(direction, FrameDirection::Sent);
                assert_eq!(opcode, 1);
                assert_eq!(payload, "ping");
            }
            other => panic!("expected WebSocketFrame(Sent), got {other:?}"),
        }
        // Received frame
        match next_event(&mut monitor).await {
            NetworkEvent::WebSocketFrame {
                direction, payload, ..
            } => {
                assert_eq!(direction, FrameDirection::Received);
                assert_eq!(payload, "pong");
            }
            other => panic!("expected WebSocketFrame(Received), got {other:?}"),
        }
        // Close
        match next_event(&mut monitor).await {
            NetworkEvent::WebSocketClose { request_id } => assert_eq!(request_id, "ws1"),
            other => panic!("expected WebSocketClose, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    #[tokio::test]
    async fn event_source_message_emits_event() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        mock.emit_event_for_session(
            "Network.eventSourceMessageReceived",
            json!({
                "requestId": "sse1",
                "eventName": "update",
                "eventId": "42",
                "data": "tick"
            }),
            SID,
        )
        .await;

        match next_event(&mut monitor).await {
            NetworkEvent::EventSourceMessage {
                request_id,
                event_name,
                event_id,
                data,
            } => {
                assert_eq!(request_id, "sse1");
                assert_eq!(event_name, "update");
                assert_eq!(event_id, "42");
                assert_eq!(data, "tick");
            }
            other => panic!("expected EventSourceMessage, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    // ---------- stream_bodies (Network.streamResourceContent) --------------

    /// Like [`spawn_monitor`], but with [`MonitorBuilder::stream_bodies`] set.
    async fn spawn_monitor_streaming(
        filter: Option<UrlMatcher>,
    ) -> (
        NetworkMonitor,
        MockConnection,
        zendriver_transport::Connection,
    ) {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), SID);
        let mut builder = MonitorBuilder::new(session).stream_bodies(true);
        if let Some(f) = filter {
            builder = builder.url_pattern(f);
        }
        let monitor = builder.start().await.unwrap();
        let id = mock.expect_cmd("Network.enable").await;
        mock.reply(id, json!({})).await;
        (monitor, mock, conn)
    }

    /// End-to-end happy path: `requestWillBeSent` triggers
    /// `Network.streamResourceContent`, its `bufferedData` is emitted as the
    /// first `HttpData` chunk, a subsequent `Network.dataReceived` with
    /// `data` set emits a second chunk, and the exchange still completes
    /// normally on `loadingFinished` — with [`NetworkExchange::request_id`]
    /// matching the chunks' `request_id`.
    #[tokio::test]
    async fn stream_bodies_prepends_buffered_data_then_streams_data_received() {
        let (mut monitor, mut mock, conn) = spawn_monitor_streaming(None).await;

        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "s1",
                "request": { "url": "https://example.com/big", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "s1", "response": { "status": 200 } }),
            SID,
        )
        .await;

        let id = mock.expect_cmd("Network.streamResourceContent").await;
        assert_eq!(mock.last_sent()["params"]["requestId"], "s1");
        mock.reply(id, json!({ "bufferedData": BASE64.encode("hello-") }))
            .await;

        match next_event(&mut monitor).await {
            NetworkEvent::HttpData { request_id, chunk } => {
                assert_eq!(request_id, "s1");
                assert_eq!(chunk, b"hello-");
            }
            other => panic!("expected HttpData (bufferedData), got {other:?}"),
        }

        mock.emit_event_for_session(
            "Network.dataReceived",
            json!({
                "requestId": "s1",
                "timestamp": 1.0,
                "dataLength": 5,
                "encodedDataLength": 5,
                "data": BASE64.encode("world")
            }),
            SID,
        )
        .await;

        match next_event(&mut monitor).await {
            NetworkEvent::HttpData { request_id, chunk } => {
                assert_eq!(request_id, "s1");
                assert_eq!(chunk, b"world");
            }
            other => panic!("expected HttpData (dataReceived), got {other:?}"),
        }

        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "s1" }), SID)
            .await;
        match next_event(&mut monitor).await {
            NetworkEvent::Http(exchange) => {
                assert_eq!(exchange.request_id(), "s1");
                assert_eq!(exchange.request.url, "https://example.com/big");
            }
            other => panic!("expected NetworkEvent::Http, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    /// Empty `bufferedData` must not emit a spurious leading chunk — only the
    /// real `dataReceived` bytes show up.
    #[tokio::test]
    async fn stream_bodies_empty_buffered_data_emits_no_leading_chunk() {
        let (mut monitor, mut mock, conn) = spawn_monitor_streaming(None).await;

        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "s2",
                "request": { "url": "https://example.com/empty-buffer", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "s2", "response": { "status": 200 } }),
            SID,
        )
        .await;
        let id = mock.expect_cmd("Network.streamResourceContent").await;
        mock.reply(id, json!({ "bufferedData": "" })).await;

        mock.emit_event_for_session(
            "Network.dataReceived",
            json!({
                "requestId": "s2",
                "timestamp": 1.0,
                "dataLength": 6,
                "encodedDataLength": 6,
                "data": BASE64.encode("chunk1")
            }),
            SID,
        )
        .await;

        match next_event(&mut monitor).await {
            NetworkEvent::HttpData { request_id, chunk } => {
                assert_eq!(request_id, "s2");
                assert_eq!(chunk, b"chunk1", "the only chunk — no empty leading one");
            }
            other => panic!("expected HttpData, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    /// A `Network.streamResourceContent` error (old / unsupported Chrome)
    /// must not emit any `HttpData` and must not break the monitor — the
    /// exchange still completes via the whole-body path.
    #[tokio::test]
    async fn stream_resource_content_error_degrades_without_breaking_monitor() {
        let (mut monitor, mut mock, conn) = spawn_monitor_streaming(None).await;

        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "s3",
                "request": { "url": "https://example.com/old-chrome", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "s3", "response": { "status": 200 } }),
            SID,
        )
        .await;
        let id = mock.expect_cmd("Network.streamResourceContent").await;
        mock.reply_err(id, -32601, "'Network.streamResourceContent' wasn't found")
            .await;

        // No HttpData ever arrives for the failed enable.
        assert_no_event(&mut monitor).await;

        // The monitor is still alive: loadingFinished completes normally.
        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "s3" }), SID)
            .await;
        match next_event(&mut monitor).await {
            NetworkEvent::Http(exchange) => {
                assert_eq!(exchange.request.url, "https://example.com/old-chrome");
            }
            other => panic!("expected NetworkEvent::Http, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    /// `stream_bodies: false` (the default) must never call
    /// `Network.streamResourceContent` and must never emit `HttpData`, even
    /// if a `dataReceived` event happens to carry `data` (e.g. some other
    /// domain consumer enabled streaming out of band).
    #[tokio::test]
    async fn stream_bodies_false_never_enables_or_emits_http_data() {
        let (mut monitor, mut mock, conn) = spawn_monitor(None).await;

        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "s4",
                "request": { "url": "https://example.com/opt-out", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "s4", "response": { "status": 200 } }),
            SID,
        )
        .await;
        assert!(
            mock.try_recv_cmd().is_none(),
            "stream_bodies: false must never issue Network.streamResourceContent"
        );

        mock.emit_event_for_session(
            "Network.dataReceived",
            json!({
                "requestId": "s4",
                "timestamp": 1.0,
                "dataLength": 4,
                "encodedDataLength": 4,
                "data": BASE64.encode("data")
            }),
            SID,
        )
        .await;
        assert_no_event(&mut monitor).await;

        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "s4" }), SID)
            .await;
        match next_event(&mut monitor).await {
            NetworkEvent::Http(exchange) => {
                assert_eq!(exchange.request.url, "https://example.com/opt-out");
            }
            other => panic!("expected NetworkEvent::Http, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    /// A duplicate `requestWillBeSent` for the same `requestId` (e.g. a
    /// redirect hop reusing the same id) must not re-issue
    /// `Network.streamResourceContent`: the `streaming` dedup set guards it.
    #[tokio::test]
    async fn stream_bodies_dedups_repeated_request_will_be_sent_for_same_request() {
        let (mut monitor, mut mock, conn) = spawn_monitor_streaming(None).await;

        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "s5",
                "request": { "url": "https://example.com/dup", "method": "GET" }
            }),
            SID,
        )
        .await;
        let id = mock.expect_cmd("Network.streamResourceContent").await;
        mock.reply(id, json!({ "bufferedData": "" })).await;

        // A second requestWillBeSent for the SAME requestId (e.g. a
        // redirect hop).
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "s5",
                "request": { "url": "https://example.com/dup-redirected", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "s5", "response": { "status": 200 } }),
            SID,
        )
        .await;

        // loadingFinished should still be the very next thing the mock
        // observes sent — no second streamResourceContent call in between.
        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "s5" }), SID)
            .await;
        let _ = next_event(&mut monitor).await; // drain the Http exchange
        assert!(
            mock.try_recv_cmd().is_none(),
            "a duplicate requestWillBeSent must not re-issue Network.streamResourceContent"
        );

        monitor.stop();
        conn.shutdown();
    }

    #[tokio::test]
    async fn dropping_monitor_cancels_correlator_task() {
        let (monitor, mock, conn) = spawn_monitor(None).await;
        let cancel = monitor.cancel.clone();
        assert!(!cancel.is_cancelled());
        drop(monitor);
        assert!(cancel.is_cancelled(), "Drop must cancel the correlator");
        drop(mock);
        conn.shutdown();
    }

    #[tokio::test]
    async fn url_filter_drops_unmatched() {
        let (mut monitor, mock, conn) = spawn_monitor(Some("/api/".into())).await;

        // Non-matching request id "2" (does NOT contain "/api/").
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "2",
                "request": { "url": "https://example.com/static/app.js", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "2" }), SID)
            .await;
        // No event should be emitted for the static asset.
        assert_no_event(&mut monitor).await;

        // Matching request id "3" (contains "/api/").
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "3",
                "request": { "url": "https://example.com/api/orders", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.responseReceived",
            json!({ "requestId": "3", "response": { "status": 201 } }),
            SID,
        )
        .await;
        mock.emit_event_for_session("Network.loadingFinished", json!({ "requestId": "3" }), SID)
            .await;

        let event = next_event(&mut monitor).await;
        let NetworkEvent::Http(exchange) = event else {
            panic!("expected NetworkEvent::Http, got {event:?}");
        };
        // The matching request passes through — never the dropped one.
        assert_eq!(exchange.request.url, "https://example.com/api/orders");
        assert_eq!(exchange.status(), Some(201));

        monitor.stop();
        conn.shutdown();
    }

    #[tokio::test]
    async fn events_for_other_sessions_are_ignored() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        // Emit a fully-formed exchange on a DIFFERENT session id.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "x",
                "request": { "url": "https://other.example.com/api/x", "method": "GET" }
            }),
            "OTHER",
        )
        .await;
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "x" }),
            "OTHER",
        )
        .await;
        assert_no_event(&mut monitor).await;

        monitor.stop();
        conn.shutdown();
    }

    /// A `Lagged` boundary arriving while a request is mid-exchange must
    /// clear its partial correlation state and surface as a
    /// `DeliveryBoundary::Lagged` — never as a stitched-together "complete"
    /// `Http` exchange. Forces the gap deterministically the same way
    /// `wait_for_idle_opts_strict_aborts_on_lagged_boundary` (`tab.rs`) and
    /// `expect_request_returns_event_stream_incomplete_on_lagged_boundary`
    /// (`expect/request.rs`) do: a 2-slot accounted bus overflowed by 5
    /// unrelated events pushed before the correlator's subscriber polls them.
    #[tokio::test]
    async fn lagged_mid_exchange_clears_partial_and_emits_boundary() {
        let (mut monitor, mock, conn) =
            spawn_monitor_with(MockConnection::pair_with_accounted_capacity(2), None).await;

        // Start (but never finish) an exchange.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "mid1",
                "request": { "url": "https://example.com/mid", "method": "GET" }
            }),
            SID,
        )
        .await;
        // Give the correlator a chance to actually drain and correlate that
        // single event (well under the 2-slot capacity) before the overflow
        // below — otherwise this test couldn't distinguish "the entry was
        // cleared by the Lagged handler" from "the entry was never inserted
        // because it, too, was lost in the overflow".
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Overflow the 2-slot accounted bus with unrelated events.
        for i in 0..5u32 {
            mock.emit_event("Test.dummy", json!({ "i": i })).await;
        }

        match next_event(&mut monitor).await {
            NetworkEvent::DeliveryBoundary(NetworkDeliveryBoundary::Lagged {
                generation,
                missed,
            }) => {
                assert_eq!(generation, 1);
                assert!(missed > 0, "expected a nonzero missed count, got {missed}");
            }
            other => panic!("expected DeliveryBoundary::Lagged, got {other:?}"),
        }

        // The matching `loadingFinished` for the pre-gap request must NOT
        // stitch into a bogus "complete" exchange — the partial entry was
        // cleared on the boundary, so this now completes nothing.
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "mid1" }),
            SID,
        )
        .await;
        assert_no_event(&mut monitor).await;

        monitor.stop();
        conn.shutdown();
    }

    /// A `Reconnected` boundary mid-exchange must clear correlation state (so
    /// no exchange is stitched across the socket swap) and surface an explicit
    /// `DeliveryBoundary::Reconnected`. Gives the `Reconnected` arm its own
    /// end-to-end coverage, distinct from the structurally-identical `Lagged`
    /// arm it mirrors.
    #[tokio::test]
    async fn reconnected_mid_exchange_clears_partial_and_emits_boundary() {
        let (mut monitor, mut mock, conn) = spawn_monitor(None).await;

        // Start (but never finish) an exchange, and give the correlator a
        // chance to correlate it before the reconnect — so this distinguishes
        // "cleared by the Reconnected handler" from "never inserted".
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "recon1",
                "request": { "url": "https://example.com/recon", "method": "GET" }
            }),
            SID,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Swap onto a fresh socket: bumps generation 1 -> 2, emits Reconnected.
        mock.reconnect(&conn);

        match next_event(&mut monitor).await {
            NetworkEvent::DeliveryBoundary(NetworkDeliveryBoundary::Reconnected {
                previous,
                generation,
            }) => {
                assert_eq!(previous, 1);
                assert_eq!(generation, 2);
            }
            other => panic!("expected DeliveryBoundary::Reconnected, got {other:?}"),
        }

        // The matching `loadingFinished` for the pre-reconnect request (emitted
        // over the new socket) must NOT stitch into a bogus "complete"
        // exchange — the partial entry was cleared on the boundary.
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "recon1" }),
            SID,
        )
        .await;
        assert_no_event(&mut monitor).await;

        monitor.stop();
        conn.shutdown();
    }

    /// Saturating the correlation map past [`MAX_TRACKED`] must surface a
    /// `DeliveryBoundary::CorrelationEvicted` — previously this was a silent
    /// `tracing` warning only.
    #[tokio::test]
    async fn correlation_cap_exceeded_emits_correlation_evicted() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        // MAX_TRACKED + 1 unique, never-completed requests: the map grows
        // past the bound on the last insert, triggering exactly one
        // eviction. None of these produce an `Http` event (never completed),
        // so the only `NetworkEvent` this loop can produce is the eviction.
        for i in 0..=MAX_TRACKED {
            mock.emit_event_for_session(
                "Network.requestWillBeSent",
                json!({
                    "requestId": format!("r{i}"),
                    "request": { "url": format!("https://example.com/{i}"), "method": "GET" }
                }),
                SID,
            )
            .await;
        }

        match next_event(&mut monitor).await {
            NetworkEvent::DeliveryBoundary(NetworkDeliveryBoundary::CorrelationEvicted { url }) => {
                assert!(
                    url.starts_with("https://example.com/"),
                    "evicted url should be one of the inserted entries, got {url}"
                );
            }
            other => panic!("expected DeliveryBoundary::CorrelationEvicted, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    /// A payload that fails to deserialize into the shape expected for its
    /// `method` must surface a `DeliveryBoundary::DecodeFailed` — the
    /// undecodable payload is never forwarded (the variant carries no
    /// fields at all).
    #[tokio::test]
    async fn malformed_payload_emits_decode_failed_without_raw_payload() {
        let (mut monitor, mock, conn) = spawn_monitor(None).await;

        // `RequestWillBeSent` requires a `request` object; omitting it fails
        // to deserialize.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({ "requestId": "bad1" }),
            SID,
        )
        .await;

        match next_event(&mut monitor).await {
            NetworkEvent::DeliveryBoundary(NetworkDeliveryBoundary::DecodeFailed) => {}
            other => panic!("expected DeliveryBoundary::DecodeFailed, got {other:?}"),
        }

        // Confirm the correlator kept running afterwards (decode failure
        // isn't a fatal gap): a well-formed exchange right after still
        // completes normally.
        mock.emit_event_for_session(
            "Network.requestWillBeSent",
            json!({
                "requestId": "good1",
                "request": { "url": "https://example.com/ok", "method": "GET" }
            }),
            SID,
        )
        .await;
        mock.emit_event_for_session(
            "Network.loadingFinished",
            json!({ "requestId": "good1" }),
            SID,
        )
        .await;
        match next_event(&mut monitor).await {
            NetworkEvent::Http(exchange) => {
                assert_eq!(exchange.request.url, "https://example.com/ok");
            }
            other => panic!("expected NetworkEvent::Http, got {other:?}"),
        }

        monitor.stop();
        conn.shutdown();
    }

    /// A `Disconnected` boundary must be emitted, and the correlator task
    /// must then end (fail closed) — the monitor stream returns `None` on
    /// the next poll rather than idling forever. A consumer that wants to
    /// keep observing must start a fresh monitor.
    #[tokio::test]
    async fn disconnected_emits_boundary_and_ends_monitor_task() {
        let (mut monitor, mock, _conn) = spawn_monitor(None).await;

        mock.disconnect();

        match next_event(&mut monitor).await {
            NetworkEvent::DeliveryBoundary(NetworkDeliveryBoundary::Disconnected {
                generation,
            }) => {
                assert_eq!(generation, 1);
            }
            other => panic!("expected DeliveryBoundary::Disconnected, got {other:?}"),
        }

        // The task ended: the stream closes (`None`), it does not hang.
        let next = tokio::time::timeout(Duration::from_secs(2), monitor.next())
            .await
            .expect("monitor stream did not end within 2s after Disconnected");
        assert!(
            next.is_none(),
            "expected the monitor stream to end after Disconnected, got {next:?}"
        );
    }

    // `MockConnection::pair()` spawns the connection actor, which requires a
    // tokio runtime — so all tests that construct a `NetworkExchange` are async.
    async fn make_exchange(status: Option<u16>, error: Option<&str>) -> NetworkExchange {
        let (_mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn, "test-session");
        let req = MonitoredRequest {
            url: "https://example.com/api".into(),
            method: "GET".into(),
            headers: HashMap::new(),
            post_data: None,
        };
        let resp = status.map(|s| MonitoredResponse {
            status: s,
            status_text: "OK".into(),
            headers: HashMap::new(),
            mime_type: "application/json".into(),
        });
        NetworkExchange {
            request: req,
            response: resp,
            error: error.map(ToOwned::to_owned),
            request_id: "r1".into(),
            session,
        }
    }

    #[tokio::test]
    async fn status_returns_none_when_no_response() {
        let ex = make_exchange(None, None).await;
        assert!(ex.status().is_none());
        assert!(!ex.is_success());
    }

    #[tokio::test]
    async fn status_returns_some_for_200() {
        let ex = make_exchange(Some(200), None).await;
        assert_eq!(ex.status(), Some(200));
        assert!(ex.is_success());
    }

    #[tokio::test]
    async fn status_304_is_not_success() {
        let ex = make_exchange(Some(304), None).await;
        assert!(!ex.is_success());
    }

    #[tokio::test]
    async fn status_404_is_not_success() {
        let ex = make_exchange(Some(404), None).await;
        assert!(!ex.is_success());
    }

    #[tokio::test]
    async fn debug_does_not_include_session_field() {
        let ex = make_exchange(Some(200), None).await;
        let s = format!("{ex:?}");
        assert!(s.contains("NetworkExchange"));
        assert!(s.contains("request"));
        assert!(s.contains("response"));
        assert!(!s.contains("session"));
    }

    #[tokio::test]
    async fn error_field_is_set_on_failed_exchange() {
        let ex = make_exchange(None, Some("net::ERR_ABORTED")).await;
        assert_eq!(ex.error.as_deref(), Some("net::ERR_ABORTED"));
    }

    #[test]
    fn frame_direction_copy_and_eq() {
        let d = FrameDirection::Sent;
        let d2 = d;
        assert_eq!(d, d2);
        assert_ne!(FrameDirection::Sent, FrameDirection::Received);
    }

    #[test]
    fn network_event_debug_roundtrip() {
        let ev = NetworkEvent::WebSocketOpen {
            request_id: "r1".into(),
            url: "wss://echo.example.com".into(),
        };
        let s = format!("{ev:?}");
        assert!(s.contains("WebSocketOpen"));
        assert!(s.contains("wss://echo.example.com"));
    }

    /// Every `NetworkDeliveryBoundary` variant is constructible, `Debug`,
    /// `Clone`, and `Eq` — including `Reconnected` and `Unknown`, which the
    /// live correlator tests above don't otherwise exercise (`Reconnected`
    /// has no public `MockConnection` injection point; `Unknown` is a
    /// defensive fallback unreachable while `AccountedRawEvent` has exactly
    /// its current four variants).
    #[test]
    fn network_delivery_boundary_variants_construct_and_debug() {
        let variants = [
            NetworkDeliveryBoundary::Lagged {
                missed: 3,
                generation: 1,
            },
            NetworkDeliveryBoundary::Reconnected {
                previous: 1,
                generation: 2,
            },
            NetworkDeliveryBoundary::Disconnected { generation: 1 },
            NetworkDeliveryBoundary::CorrelationEvicted {
                url: "https://example.com/evicted".into(),
            },
            NetworkDeliveryBoundary::DecodeFailed,
            NetworkDeliveryBoundary::Unknown,
        ];
        for v in &variants {
            let cloned = v.clone();
            assert_eq!(v, &cloned);
            let ev = NetworkEvent::DeliveryBoundary(cloned);
            let s = format!("{ev:?}");
            assert!(s.contains("DeliveryBoundary"), "got {s}");
        }
    }

    fn partial_entry(url: &str) -> PartialExchange {
        (
            MonitoredRequest {
                url: url.into(),
                method: "GET".into(),
                headers: HashMap::new(),
                post_data: None,
            },
            None,
        )
    }

    #[test]
    fn evict_oldest_drops_partial_and_mirrored_url() {
        // An in-flight HTTP exchange has a key in BOTH maps (mirrored on the
        // `requestWillBeSent` path). Evicting must remove it from both so the
        // bound actually shrinks the live correlation state.
        let mut partial: HashMap<String, PartialExchange> = HashMap::new();
        let mut urls: HashMap<String, String> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        partial.insert("req1".into(), partial_entry("https://example.com/a"));
        urls.insert("req1".into(), "https://example.com/a".into());
        track_order(&mut order, &urls, "req1".into());

        let evicted = evict_oldest(&mut urls, &mut partial, &mut order);

        assert_eq!(
            evicted.as_deref(),
            Some("https://example.com/a"),
            "returns the evicted entry's URL so the caller can report CorrelationEvicted"
        );
        assert!(partial.is_empty(), "partial entry must be evicted");
        assert!(
            urls.is_empty(),
            "the partial entry's mirrored url must be evicted too"
        );
    }

    #[test]
    fn evict_oldest_falls_back_to_urls_only_entry() {
        // A WebSocket / completed-handshake entry lives only in `urls` (no
        // `partial` row). Eviction must still drop it rather than no-op and
        // leave the map over the bound.
        let mut partial: HashMap<String, PartialExchange> = HashMap::new();
        let mut urls: HashMap<String, String> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        urls.insert("ws1".into(), "wss://echo.example.com".into());
        track_order(&mut order, &urls, "ws1".into());

        let evicted = evict_oldest(&mut urls, &mut partial, &mut order);

        assert_eq!(evicted.as_deref(), Some("wss://echo.example.com"));
        assert!(urls.is_empty(), "urls-only entry must be evicted");
    }

    #[test]
    fn evict_oldest_evicts_in_insertion_order() {
        // FIFO: the oldest-inserted request is evicted first, deterministically
        // — not a hash-arbitrary victim that could drop a fresh request while a
        // stuck one survives.
        let mut partial: HashMap<String, PartialExchange> = HashMap::new();
        let mut urls: HashMap<String, String> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        for (id, url) in [
            ("req1", "https://example.com/1"),
            ("req2", "https://example.com/2"),
            ("req3", "https://example.com/3"),
        ] {
            urls.insert(id.into(), url.into());
            track_order(&mut order, &urls, id.into());
        }

        assert_eq!(
            evict_oldest(&mut urls, &mut partial, &mut order).as_deref(),
            Some("https://example.com/1"),
            "oldest (req1) evicted first"
        );
        assert_eq!(
            evict_oldest(&mut urls, &mut partial, &mut order).as_deref(),
            Some("https://example.com/2"),
            "then req2"
        );
        assert_eq!(
            evict_oldest(&mut urls, &mut partial, &mut order).as_deref(),
            Some("https://example.com/3"),
            "then req3"
        );
    }

    #[test]
    fn evict_oldest_skips_tombstones_for_completed_requests() {
        // A request removed by normal completion leaves a tombstone in `order`;
        // eviction skips it and drops the oldest still-LIVE request.
        let mut partial: HashMap<String, PartialExchange> = HashMap::new();
        let mut urls: HashMap<String, String> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        urls.insert("req1".into(), "https://example.com/1".into());
        track_order(&mut order, &urls, "req1".into());
        urls.insert("req2".into(), "https://example.com/2".into());
        track_order(&mut order, &urls, "req2".into());
        // req1 completes normally: removed from `urls`, tombstone stays in `order`.
        urls.remove("req1");

        assert_eq!(
            evict_oldest(&mut urls, &mut partial, &mut order).as_deref(),
            Some("https://example.com/2"),
            "req1 tombstone skipped; oldest live (req2) evicted"
        );
    }

    #[test]
    fn evict_oldest_on_empty_is_a_noop() {
        // Defensive: never panic when nothing is tracked.
        let mut partial: HashMap<String, PartialExchange> = HashMap::new();
        let mut urls: HashMap<String, String> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        let evicted = evict_oldest(&mut urls, &mut partial, &mut order);
        assert!(evicted.is_none());
        assert!(partial.is_empty() && urls.is_empty());
    }

    #[test]
    fn track_order_stays_bounded_despite_tombstones() {
        // A long-lived page that opens + completes far more requests than the
        // cap (without ever exceeding the concurrent cap) must not let `order`
        // grow without bound: completed-request tombstones get compacted away.
        let mut urls: HashMap<String, String> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        for i in 0..(MAX_TRACKED * 2 + 5) {
            let id = format!("req{i}");
            urls.insert(id.clone(), "https://example.com/x".into());
            track_order(&mut order, &urls, id.clone());
            urls.remove(&id); // completes immediately → becomes a tombstone
        }
        assert!(
            order.len() <= MAX_TRACKED * 2,
            "order must stay bounded via compaction; grew to {}",
            order.len()
        );
    }
}
