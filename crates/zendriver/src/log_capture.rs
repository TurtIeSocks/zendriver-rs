//! Capture `tracing` events emitted by the code under test — test-only.
//!
//! Some behaviour in this crate is observable *only* as a log line. A geo
//! probe that fails returns `None` and tells the operator why; the `None`
//! was already there before the telling was, so a test asserting on the
//! return value alone stays green against code that logs nothing at all.
//! That is precisely the shape of the two geo tests this module was added
//! to repair: both passed against the unfixed `resolve()`.
//!
//! Asserting the warning is what makes such a test non-vacuous.

// Today every consumer sits behind the `geo` feature, so a default-feature
// test build legitimately uses none of this. Unused here means "no test in
// this configuration needed it", not "nothing needs it".
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing::field::{Field, Visit};

/// One captured event: its level, and its `message` field rendered.
pub(crate) type CapturedLog = (Level, String);

/// A `tracing_subscriber` layer recording every event that carries a
/// `message` field into a shared buffer.
#[derive(Clone, Default)]
pub(crate) struct LogCapture(Arc<Mutex<Vec<CapturedLog>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor(None);
        event.record(&mut visitor);
        if let Some(message) = visitor.0 {
            // A poisoned buffer means some other thread panicked mid-push.
            // There is nothing here worth recovering, and turning it into a
            // second panic would bury the first one's message.
            if let Ok(mut buf) = self.0.lock() {
                buf.push((*event.metadata().level(), message));
            }
        }
    }
}

/// Pulls out the `message` field. `tracing` records it as the `Debug` of a
/// `format_args!`, whose `Debug` rendering is the plain text — no
/// surrounding quotes, no escaping.
struct MessageVisitor(Option<String>);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

/// Run `fut` with a capturing subscriber attached, returning its output
/// alongside everything it logged.
///
/// Attached to the *future* rather than installed as a thread-local
/// default, so it survives the executor moving the future across threads.
pub(crate) async fn with_captured_logs<T>(fut: impl Future<Output = T>) -> (T, Vec<CapturedLog>) {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let out = fut.with_subscriber(subscriber).await;
    let logs = capture.0.lock().expect("log buffer poisoned").clone();
    (out, logs)
}

/// Synchronous sibling of [`with_captured_logs`], for code under test that
/// is not a future. Installed as the thread-local default subscriber for the
/// duration of `f`.
pub(crate) fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<CapturedLog>) {
    use tracing_subscriber::layer::SubscriberExt;

    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let out = tracing::subscriber::with_default(subscriber, || {
        // `tracing` caches each callsite's interest globally the first time it
        // is evaluated, and a site first reached with no subscriber installed
        // caches "never" — which is every other test in this binary. Without
        // this rebuild a capture test passes alone and fails in a full run.
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    let logs = capture.0.lock().expect("log buffer poisoned").clone();
    (out, logs)
}

/// Just the `WARN` messages.
///
/// Capture is deliberately unfiltered — a caller may want to assert on any
/// level — but dependencies are chatty (hyper alone emits a dozen
/// connection-pool traces per request), so anything asserting on *absence*
/// has to say which level it means.
pub(crate) fn warnings(logs: &[CapturedLog]) -> Vec<&str> {
    logs.iter()
        .filter(|(level, _)| *level == Level::WARN)
        .map(|(_, message)| message.as_str())
        .collect()
}

/// The message of the single `WARN` in `logs`, panicking unless there is
/// exactly one.
///
/// Every failure exit these tests cover is meant to log exactly one
/// reason. Zero means the operator gets silence; several means the test is
/// not pinning down which one actually fired.
#[track_caller]
pub(crate) fn sole_warning(logs: &[CapturedLog]) -> &str {
    let warnings = warnings(logs);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one WARN, got {warnings:?}",
    );
    warnings[0]
}
