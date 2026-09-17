//! Event expectation helpers (`expect_request` / `expect_response` /
//! `expect_dialog` / `expect_download` / `expect_file_chooser`).
//!
//! Each helper registers a one-shot subscription on a Tab's CDP event stream
//! and resolves with the first matching event. [`UrlMatcher`] is the shared
//! pattern type used by request/response expectations.
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! // Pre-register before triggering the action that causes the request.
//! let exp = tab.expect_response("/api/users");
//! tab.find().css("button#load").one().await?.click().await?;
//! let resp = exp.await?;
//! let body = resp.body().await?;
//! # let _ = body;
//! # Ok(()) }
//! ```

pub mod dialog;
pub mod download;
pub mod file_chooser;
pub mod request;
pub mod response;

pub use crate::url_matcher::UrlMatcher;

use futures::{Stream, StreamExt};
use zendriver_transport::{AccountedRawEvent, SessionHandle};

use crate::error::{Result, ZendriverError};

/// Subscribe to `method`-shaped events scoped to `session`, decoded via
/// `T`'s [`serde::de::DeserializeOwned`] impl. Backs every `expect_*`
/// subscriber task — riding [`Connection::subscribe_raw_accounted`]
/// (rather than the plain [`SessionHandle::subscribe`]) so a delivery-loss
/// boundary crossed while a wait is in progress is visible instead of
/// silently vanishing.
///
/// [`Connection::subscribe_raw_accounted`]: zendriver_transport::Connection::subscribe_raw_accounted
///
/// Yields `Ok(T)` for every successfully-decoded `method` event on this
/// session. Yields `Err(`[`ZendriverError::EventStreamIncomplete`]`)` on:
///
/// - [`AccountedRawEvent::Disconnected`] / [`AccountedRawEvent::Reconnected`]
///   — the watched stream is gone, so the caller can no longer prove the
///   awaited condition didn't occur just before the boundary.
/// - [`AccountedRawEvent::Lagged`] — this subscriber missed frames it can't
///   recover. There's no way to prove the awaited event wasn't among them,
///   so a "no show" (`Timeout`) would be a possibly-wrong claim. Treated the
///   same as an outright disconnect — mirrors the `IdleLossPolicy::Strict`
///   precedent in `Tab::wait_for_idle_opts`, which makes the identical call
///   for the same reason.
/// - A decode failure on an event that otherwise matches `method` +
///   `session` — the event we were waiting for arrived in a shape we can't
///   parse, which leaves us just as unable to confirm the awaited condition
///   as an outright loss would.
///
/// Events for any other method or session are silently skipped: they were
/// never candidates for the awaited condition, so their absence from the
/// output isn't a loss of anything this caller cares about.
pub(crate) fn watch<T>(
    session: &SessionHandle,
    method: &'static str,
) -> impl Stream<Item = Result<T>> + Send + Unpin + use<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let sid = session.session_id().map(str::to_string);
    let raw = session.connection().subscribe_raw_accounted();
    Box::pin(raw.filter_map(move |acc| {
        let sid = sid.clone();
        async move {
            match acc {
                AccountedRawEvent::Event { event, .. } => {
                    if event.session_id.as_deref() == sid.as_deref() && event.method == method {
                        match serde_json::from_value::<T>(event.params) {
                            Ok(v) => Some(Ok(v)),
                            Err(_) => Some(Err(ZendriverError::EventStreamIncomplete)),
                        }
                    } else {
                        None
                    }
                }
                // coverage: `Reconnected` shares this arm with `Lagged` /
                // `Disconnected`. Each boundary has its own end-to-end test
                // (`expect_response_returns_event_stream_incomplete_on_{reconnect,disconnect}`,
                // plus the `Lagged` capacity harness) — if this arm is ever
                // split (e.g. reconnect-specific retry semantics), keep those
                // per-boundary tests so coverage doesn't silently regress.
                AccountedRawEvent::Lagged { .. }
                | AccountedRawEvent::Reconnected { .. }
                | AccountedRawEvent::Disconnected { .. } => {
                    Some(Err(ZendriverError::EventStreamIncomplete))
                }
            }
        }
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::time::timeout;
    use zendriver_transport::testing::MockConnection;

    // A flat session filters events by its `sessionId`: an event tagged with
    // the same id is delivered. This is the control for the root case below —
    // if it broke, the harness (not the model) would be at fault.
    #[tokio::test]
    async fn flat_session_watch_still_receives_its_events() {
        let (mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn, "S1");
        let mut stream = watch::<Value>(&sess, "Page.javascriptDialogOpening");
        mock.emit_event_for_session(
            "Page.javascriptDialogOpening",
            json!({ "message": "hi" }),
            "S1",
        )
        .await;
        let got = timeout(Duration::from_millis(500), stream.next()).await;
        assert!(
            matches!(got, Ok(Some(Ok(_)))),
            "flat session dropped its own event: {got:?}"
        );
    }

    // A root (per-tab socket) session carries no `sessionId`, and neither do
    // the events on its own socket. `session_id()` must therefore be `None`
    // here so the filter matches sessionless frames — a `""` sentinel would
    // make `None != Some("")` and drop every event the tab ever sees.
    #[tokio::test]
    async fn root_session_watch_receives_sessionless_events() {
        let (mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new_root(conn);
        let mut stream = watch::<Value>(&sess, "Page.javascriptDialogOpening");
        mock.emit_event("Page.javascriptDialogOpening", json!({ "message": "hi" }))
            .await;
        let got = timeout(Duration::from_millis(500), stream.next()).await;
        assert!(
            matches!(got, Ok(Some(Ok(_)))),
            "root session dropped its own event: {got:?}"
        );
    }
}
