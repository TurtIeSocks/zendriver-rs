//! Test-only helpers — gated behind `cfg(any(test, feature = "testing"))`.
//!
//! Provides [`MockConnection`], a paired pseudo-Chrome that lets downstream
//! tests drive a real [`Connection`] without spawning a WebSocket. The mock
//! and the connection share an in-memory duplex pipe built on the same
//! `DriverStream` plumbing used by this crate's internal actor tests.

#![allow(clippy::expect_used, clippy::panic, clippy::missing_panics_doc)]

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::connection::{Connection, spawn_actor, spawn_actor_with_observers};
use crate::observer::TargetObserver;

/// A paired pseudo-Chrome: tests push frames the driver would read, and read
/// frames the driver sent. Driving an end-to-end interaction looks like:
///
/// ```ignore
/// use serde_json::json;
/// use zendriver_transport::testing::MockConnection;
///
/// # tokio_test::block_on(async {
/// let (mut mock, conn) = MockConnection::pair();
/// let call = tokio::spawn({
///     let c = conn.clone();
///     async move { c.call_raw("Page.navigate", json!({}), None).await }
/// });
/// let id = mock.expect_cmd("Page.navigate").await;
/// mock.reply(id, json!({ "frameId": "F1" })).await;
/// let res = call.await.unwrap().unwrap();
/// # });
/// ```
#[derive(Debug)]
pub struct MockConnection {
    server_in: mpsc::Sender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    server_out: mpsc::Receiver<Message>,
    last_sent: Option<Value>,
}

impl MockConnection {
    /// Pair a `MockConnection` with a driver-side [`Connection`]. The
    /// connection's actor is spawned onto the current tokio runtime; drop the
    /// connection (and call [`Connection::shutdown`]) to stop the actor.
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

    /// Variant of [`Self::pair`] that spawns the actor with the given
    /// `observers` chain. Used by downstream crates (notably
    /// `zendriver-stealth`) to assert their observer drives the correct
    /// sequence of CDP calls on `Target.attachedToTarget`.
    #[must_use]
    pub fn pair_with_observers(observers: Vec<Arc<dyn TargetObserver>>) -> (Self, Connection) {
        let (tx_to_driver, rx_driver) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream {
            tx: tx_from_driver,
            rx: rx_driver,
        };
        let conn = spawn_actor_with_observers(driver, observers);
        let mock = MockConnection {
            server_in: tx_to_driver,
            server_out: rx_test,
            last_sent: None,
        };
        (mock, conn)
    }

    /// Variant of [`Self::pair`] with a caller-controlled
    /// [`crate::AccountedRawEvent`] bus capacity, letting a downstream test
    /// force a deterministic [`crate::AccountedRawEvent::Lagged`] by
    /// emitting more events than `accounted_capacity` before the
    /// `subscribe_raw_accounted()` subscriber ever polls — without pushing
    /// thousands of frames through the real (1024-deep) production bus.
    ///
    /// ```ignore
    /// use zendriver_transport::{AccountedRawEvent, testing::MockConnection};
    ///
    /// # tokio_test::block_on(async {
    /// let (mock, conn) = MockConnection::pair_with_accounted_capacity(2);
    /// let mut events = conn.subscribe_raw_accounted();
    /// // ...emit more than 2 events on `mock` without polling `events`...
    /// # });
    /// ```
    #[must_use]
    pub fn pair_with_accounted_capacity(accounted_capacity: usize) -> (Self, Connection) {
        let (tx_to_driver, rx_driver) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream {
            tx: tx_from_driver,
            rx: rx_driver,
        };
        let conn = crate::connection::spawn_actor_with_observers_timeout_and_capacity(
            driver,
            Vec::new(),
            crate::connection::DEFAULT_OBSERVER_TIMEOUT,
            accounted_capacity,
        );
        let mock = MockConnection {
            server_in: tx_to_driver,
            server_out: rx_test,
            last_sent: None,
        };
        (mock, conn)
    }

    /// Block until the driver sends a command whose `method` field matches.
    /// Returns the command id. Wrap in [`tokio::time::timeout`] at test sites
    /// — there is no built-in timeout.
    ///
    /// # Panics
    /// Panics if the driver channel closes, the frame is not a text frame, or
    /// the frame cannot be parsed as JSON with `id` and `method` fields.
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

    /// Returns the most recently observed outbound frame as a JSON value.
    ///
    /// # Panics
    /// Panics if called before any command has been observed via
    /// [`Self::expect_cmd`].
    #[must_use]
    pub fn last_sent(&self) -> &Value {
        self.last_sent.as_ref().expect("no command observed yet")
    }

    /// Non-blocking probe: returns `(method, id)` for the next queued
    /// outbound command, or `None` if the channel is empty. Use in negative
    /// assertions like "no second CDP call landed after X" — the canonical
    /// example is verifying that [`Drop`] paths do not double-fire a CDP
    /// method already dispatched by a `mut self` consuming method.
    pub fn try_recv_cmd(&mut self) -> Option<(String, u64)> {
        let msg = self.server_out.try_recv().ok()?;
        self.decode_cmd(msg)
    }

    /// Bounded read of the next outbound command: waits up to `budget` for one
    /// to arrive and returns `None` if none does.
    ///
    /// Fills the gap between [`Self::expect_cmd`], which waits forever and so
    /// hangs a test whose driver has already returned, and
    /// [`Self::try_recv_cmd`], which reports `None` on a channel that is idle
    /// but still live. Draining a driver whose command sequence is not fully
    /// deterministic — a poll loop whose tick count depends on timing — wants
    /// exactly this: keep serving until the driver goes quiet for `budget`,
    /// then stop.
    pub async fn recv_cmd_timeout(&mut self, budget: std::time::Duration) -> Option<(String, u64)> {
        let msg = tokio::time::timeout(budget, self.server_out.recv())
            .await
            .ok()??;
        self.decode_cmd(msg)
    }

    /// Decode one outbound frame into `(method, id)`, recording it as the
    /// frame [`Self::last_sent`] will return. `None` for anything that is not
    /// a text frame carrying both fields.
    fn decode_cmd(&mut self, msg: Message) -> Option<(String, u64)> {
        let Message::Text(text) = msg else {
            return None;
        };
        let v: Value = serde_json::from_str(&text).ok()?;
        self.last_sent = Some(v.clone());
        let method = v["method"].as_str()?.to_string();
        let id = v["id"].as_u64()?;
        Some((method, id))
    }

    /// Reply to command `id` with a success `result`.
    pub async fn reply(&self, id: u64, result: Value) {
        let frame = serde_json::json!({ "id": id, "result": result }).to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }

    /// Reply to command `id` with an error payload (`code`, `message`).
    pub async fn reply_err(&self, id: u64, code: i32, message: &str) {
        let frame = serde_json::json!({
            "id": id,
            "error": { "code": code, "message": message }
        })
        .to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }

    /// Emit a CDP event with no session id.
    pub async fn emit_event(&self, method: &str, params: Value) {
        let frame = serde_json::json!({ "method": method, "params": params }).to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }

    /// Simulate an *unexpected* WebSocket disconnect: drop the server→driver
    /// channel so the actor's stream returns `None` (the socket vanished).
    /// In-flight CDP calls then drain with
    /// [`crate::error::TransportError::Disconnected`] — distinct from the
    /// clean shutdown produced by [`Connection::shutdown`]. Mirrors Chrome
    /// dying or the socket being severed mid-session; the canonical way for a
    /// downstream test to exercise the typed-disconnect path without a real
    /// WebSocket.
    ///
    /// Consumes the mock: once the channel is closed there is nothing more to
    /// drive from the server side.
    pub fn disconnect(self) {
        drop(self.server_in);
        // `server_out` is dropped with `self`; the driver's sink writes will
        // start failing, but the stream-end (`None`) is what trips the
        // disconnect drain.
    }

    /// Simulate a transport *reconnect*: swap `conn` onto a fresh in-memory
    /// socket via [`Connection::reconnect`]. That bumps the connection
    /// generation and emits a single [`crate::AccountedRawEvent::Reconnected`]
    /// on the accounted bus — distinct from [`Self::disconnect`], which emits
    /// `Disconnected` (the connection is *replaced*, not lost). The accounted
    /// bus itself survives, so a `subscribe_raw_accounted` subscriber observes
    /// the `Reconnected` boundary and keeps receiving afterwards.
    ///
    /// Unlike [`Self::disconnect`], this keeps the mock usable: it rewires the
    /// mock onto the new socket, so [`Self::emit_event`] / [`Self::expect_cmd`]
    /// drive the reconnected actor. The generation observed by the subscriber
    /// goes `1 -> 2` on the first call (`previous = 1`, `generation = 2`).
    pub fn reconnect(&mut self, conn: &Connection) {
        let (tx_to_driver, rx_driver) =
            mpsc::channel::<Result<Message, tokio_tungstenite::tungstenite::Error>>(64);
        let (tx_from_driver, rx_test) = mpsc::channel::<Message>(64);
        let driver = crate::connection::test_only::DriverStream {
            tx: tx_from_driver,
            rx: rx_driver,
        };
        conn.reconnect(driver);
        self.server_in = tx_to_driver;
        self.server_out = rx_test;
        self.last_sent = None;
    }

    /// Emit a CDP event scoped to a specific session.
    pub async fn emit_event_for_session(&self, method: &str, params: Value, session_id: &str) {
        let frame = serde_json::json!({
            "method": method,
            "params": params,
            "sessionId": session_id,
        })
        .to_string();
        self.server_in
            .send(Ok(Message::text(frame)))
            .await
            .expect("driver closed");
    }
}

/// Collects the `tracing` events a future emits, so a test can assert on a
/// diagnostic whose only effect is a log line.
///
/// Drivers in this workspace emit hints that no return value carries — "your
/// bypass is stuck because stealth is off" being the canonical one. Without a
/// capture the hint is deletable with the suite still green, which is how a
/// diagnostic quietly stops working.
///
/// # Scope
/// [`capture`](Self::capture) collects the events emitted while the future it
/// wraps is being polled. That scope is the *task*, so events from a
/// `tokio::spawn`ed task are **not** collected — drive the code under test in
/// the same task instead, `tokio::join!`-ing the driver future with the one
/// serving its [`MockConnection`], rather than spawning it.
///
/// # Why this installs a global subscriber
/// The obvious implementation — a scoped default subscriber over the future —
/// silently collects nothing as soon as a sibling test touches the same
/// `warn!` callsite first. `tracing` caches per-callsite interest, a callsite
/// evaluated with no subscriber installed caches as "never", and a later
/// scoped subscriber is never consulted for it again. Because test binaries
/// run tests concurrently, that turns into a capture that passes alone and
/// fails in the suite. Installing one permissive global subscriber (once per
/// process) and routing events to a task-local buffer removes the race:
/// every callsite is registered against a subscriber that is always enabled.
///
/// Two consequences for the caller, since that subscriber is process-wide.
/// It is installed on the first [`capture`](Self::capture) in the binary and
/// stays for the run, so every other test's events go through it as well;
/// they are dropped rather than printed, because only a task inside a
/// `capture` has anywhere to put them. And if the binary already installed a
/// global subscriber of its own, that one keeps the slot and a capture
/// collects **nothing** rather than fighting for it. An empty
/// [`events`](Self::events) therefore means "nothing was logged *or* someone
/// else owns the subscriber", which is worth remembering before asserting a
/// count of zero: write the assertion so the hint's presence is what proves
/// the capture works.
#[derive(Debug, Clone, Default)]
pub struct LogCapture {
    events: Arc<std::sync::Mutex<Vec<String>>>,
}

tokio::task_local! {
    /// Buffer the global subscriber writes to while a [`LogCapture::capture`]
    /// future is being polled. Absent outside one, in which case events are
    /// dropped.
    static ACTIVE_CAPTURE: Arc<std::sync::Mutex<Vec<String>>>;
}

/// Install the collecting subscriber as the process-wide default, once.
fn install_collector() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // A binary that installed its own global subscriber keeps it; capture
        // then collects nothing rather than fighting over the slot.
        if tracing::subscriber::set_global_default(CollectingSubscriber).is_ok() {
            // Callsites already registered against the no-op subscriber are
            // cached as "never"; re-register them against this one.
            tracing::callsite::rebuild_interest_cache();
        }
    });
}

impl LogCapture {
    /// A fresh, empty capture.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `fut` to completion, collecting the `tracing` events emitted while
    /// it is polled, and return whatever `fut` returns.
    pub async fn capture<F: std::future::Future>(&self, fut: F) -> F::Output {
        install_collector();
        ACTIVE_CAPTURE.scope(Arc::clone(&self.events), fut).await
    }

    /// Every captured event, formatted as `LEVEL field=value ...`.
    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.events.lock().expect("log capture poisoned").clone()
    }

    /// Whether any captured event contains `needle`.
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.events().iter().any(|e| e.contains(needle))
    }

    /// How many captured events contain `needle`.
    #[must_use]
    pub fn count(&self, needle: &str) -> usize {
        self.events().iter().filter(|e| e.contains(needle)).count()
    }
}

/// Minimal `tracing::Subscriber` backing [`LogCapture`]. Records events into
/// whichever capture is active on the current task and ignores spans — the
/// drivers under test emit bare events.
#[derive(Debug)]
struct CollectingSubscriber;

impl tracing::Subscriber for CollectingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        // Outside a `LogCapture::capture` scope there is nowhere to put it,
        // which is the normal case for the rest of the suite.
        let _ = ACTIVE_CAPTURE.try_with(|events| {
            let mut visitor = FieldVisitor(String::new());
            event.record(&mut visitor);
            events.lock().expect("log capture poisoned").push(format!(
                "{} {}",
                event.metadata().level(),
                visitor.0
            ));
        });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Flattens an event's fields into `name=value` pairs. The `message` field of
/// a `warn!("literal")` arrives here as `format_args!`, whose `Debug` is the
/// rendered text.
struct FieldVisitor(String);

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0
            .push_str(&format!("{name}={value:?}", name = field.name()));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0
            .push_str(&format!("{name}={value}", name = field.name()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn log_capture_collects_events_from_the_future_it_wraps() {
        let capture = LogCapture::new();
        capture
            .capture(async {
                tracing::warn!(answer = 42, "a hint worth asserting");
            })
            .await;
        assert!(
            capture.contains("a hint worth asserting"),
            "captured {:?}",
            capture.events()
        );
        assert!(capture.contains("answer=42"));
    }

    #[tokio::test]
    async fn mock_round_trips_a_call() {
        let (mut mock, conn) = MockConnection::pair();
        let call = tokio::spawn({
            let c = conn.clone();
            async move {
                c.call_raw("Page.navigate", json!({ "url": "https://x.test" }), None)
                    .await
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
