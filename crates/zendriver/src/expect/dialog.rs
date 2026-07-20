//! [`DialogExpectation`] + [`MatchedDialog`] + [`crate::Tab::expect_dialog`]
//! (gated `expect`).
//!
//! Registers a one-shot subscription against `Page.javascriptDialogOpening`
//! on a tab's session, resolves with the first dialog event, and exposes
//! [`MatchedDialog::accept`] / [`MatchedDialog::dismiss`] which dispatch
//! `Page.handleJavaScriptDialog`. No URL matcher: dialogs don't carry a URL
//! the way requests/responses do; the page URL is captured on the matched
//! dialog for context but isn't a filter — any dialog opened during the
//! expectation window matches.
//!
//! `Page.enable` is already on for every Tab via P1's `Tab::goto`, so this
//! module does not re-enable the domain.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::Sleep;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};

/// Default outer timeout for a [`DialogExpectation`] — matches the rest of
/// the high-level surface (`wait_for_load`, etc).
const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(30);

/// JavaScript dialog flavor reported by Chrome on
/// `Page.javascriptDialogOpening`.
///
/// `Beforeunload` corresponds to the browser's leave-confirmation dialog;
/// the others mirror the `window.alert` / `confirm` / `prompt` builtins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogType {
    /// `window.alert(...)`.
    Alert,
    /// Navigation-away confirmation (`beforeunload` handler).
    Beforeunload,
    /// `window.confirm(...)`.
    Confirm,
    /// `window.prompt(...)`.
    Prompt,
}

impl DialogType {
    fn from_cdp(s: &str) -> Self {
        match s {
            "alert" => Self::Alert,
            "beforeunload" => Self::Beforeunload,
            "confirm" => Self::Confirm,
            "prompt" => Self::Prompt,
            // CDP only ever reports the four above; fall back to Alert as
            // the safest default rather than introducing an Unknown variant.
            _ => Self::Alert,
        }
    }
}

/// A JavaScript dialog observed via `Page.javascriptDialogOpening`.
///
/// The session handle is retained so [`Self::accept`] / [`Self::dismiss`]
/// can dispatch `Page.handleJavaScriptDialog` against the same target the
/// event arrived on. Consumed by value on accept/dismiss — each dialog can
/// only be handled once.
///
/// `Debug` is manually implemented since [`SessionHandle`] does not derive
/// it; the session is rendered as a placeholder.
#[derive(Clone)]
pub struct MatchedDialog {
    /// Dialog flavor (alert/beforeunload/confirm/prompt).
    pub dialog_type: DialogType,
    /// Message text shown by the dialog.
    pub message: String,
    /// Default value for `prompt(...)` dialogs. `None` for alert/confirm/
    /// beforeunload (which Chrome reports with an empty default).
    pub default_prompt: Option<String>,
    /// URL of the page that opened the dialog.
    pub url: String,
    /// Session this dialog arrived on. Retained so accept/dismiss dispatch
    /// against the correct target.
    pub session: SessionHandle,
}

impl std::fmt::Debug for MatchedDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchedDialog")
            .field("dialog_type", &self.dialog_type)
            .field("message", &self.message)
            .field("default_prompt", &self.default_prompt)
            .field("url", &self.url)
            .field("session", &"<SessionHandle>")
            .finish()
    }
}

impl MatchedDialog {
    /// Accept the dialog.
    ///
    /// For `prompt` dialogs, pass the value to submit via `prompt_text`;
    /// for alert/confirm/beforeunload, pass `None`. Dispatches
    /// `Page.handleJavaScriptDialog { accept: true, promptText }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let dialog = tab.expect_dialog().await?;
    /// // ... trigger something that opens a prompt ...
    /// dialog.accept(Some("Alice".into())).await?;
    /// # Ok(()) }
    /// ```
    pub async fn accept(self, prompt_text: Option<String>) -> Result<()> {
        let _ = self
            .session
            .call(
                "Page.handleJavaScriptDialog",
                json!({
                    "accept": true,
                    "promptText": prompt_text.unwrap_or_default(),
                }),
            )
            .await?;
        Ok(())
    }

    /// Dismiss the dialog.
    ///
    /// Dispatches `Page.handleJavaScriptDialog { accept: false }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let dialog = tab.expect_dialog().await?;
    /// dialog.dismiss().await?;
    /// # Ok(()) }
    /// ```
    pub async fn dismiss(self) -> Result<()> {
        let _ = self
            .session
            .call("Page.handleJavaScriptDialog", json!({ "accept": false }))
            .await?;
        Ok(())
    }
}

/// Awaitable handle returned by [`crate::Tab::expect_dialog`]. Resolves with
/// the first matched [`MatchedDialog`] or [`ZendriverError::Timeout`] if no
/// dialog opens within the configured timeout.
///
/// Implements [`Future`] directly — `.await` works without calling
/// `.matched()`. The `.matched()` accessor exists for parity with the
/// Playwright-style fluent API.
#[derive(Debug)]
pub struct DialogExpectation {
    rx: oneshot::Receiver<Result<MatchedDialog>>,
    timeout: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl DialogExpectation {
    /// Override the default 30s timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let dialog = tab.expect_dialog().timeout(Duration::from_secs(5)).await?;
    /// # let _ = dialog;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        // Reset any already-armed sleep — the next poll will rebuild it
        // with the new deadline.
        self.sleep = None;
        self
    }

    /// Playwright-style alias for `.await`.
    ///
    /// Functionally identical to awaiting the expectation directly.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let dialog = tab.expect_dialog().matched().await?;
    /// dialog.dismiss().await?;
    /// # Ok(()) }
    /// ```
    pub async fn matched(self) -> Result<MatchedDialog> {
        self.await
    }
}

impl Future for DialogExpectation {
    type Output = Result<MatchedDialog>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the oneshot first — if the subscriber already sent, return
        // without ever arming the sleep timer.
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(Ok(dialog))) => return Poll::Ready(Ok(dialog)),
            // Subscriber observed a delivery-loss boundary (disconnect,
            // reconnect, lag, or a same-method decode failure) before a
            // dialog opened — see `crate::expect::watch`. We can no longer
            // prove a dialog didn't open just before the boundary, so
            // reporting a plain timeout would be a lie.
            Poll::Ready(Ok(Err(e))) => return Poll::Ready(Err(e)),
            Poll::Ready(Err(_)) => {
                // Sender dropped without sending anything — the accounted
                // stream itself ended (e.g. a clean `Connection::shutdown`,
                // which never emits a `Disconnected` boundary) with no
                // boundary and no match observed. Surface as timeout: same
                // observable shape as a genuine no-show.
                return Poll::Ready(Err(ZendriverError::Timeout(self.timeout)));
            }
            Poll::Pending => {}
        }

        // Lazily arm the timer on first poll so `timeout(...)` overrides
        // take effect.
        let timeout = self.timeout;
        let sleep = self
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(ZendriverError::Timeout(timeout))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// CDP `Page.javascriptDialogOpening` payload. Field names follow the
/// protocol (camelCase) via serde rename.
#[derive(Debug, Deserialize)]
struct JavascriptDialogOpeningEvent {
    url: String,
    message: String,
    #[serde(rename = "type")]
    dialog_type: String,
    #[serde(rename = "defaultPrompt", default)]
    default_prompt: Option<String>,
}

/// Spawn a one-shot subscriber that watches `Page.javascriptDialogOpening`
/// on `session`, sends the first event through the `tx`, and exits.
/// Subscription registers synchronously before the returned
/// [`DialogExpectation`] is constructed so dialogs fired immediately after
/// a trigger action cannot slip past us.
pub(crate) fn register(session: &SessionHandle) -> DialogExpectation {
    let (tx, rx) = oneshot::channel();
    let mut stream = crate::expect::watch::<JavascriptDialogOpeningEvent>(
        session,
        "Page.javascriptDialogOpening",
    );
    let session_for_dialog = session.clone();
    tokio::spawn(async move {
        if let Some(res) = stream.next().await {
            let outcome = res.map(|ev| MatchedDialog {
                dialog_type: DialogType::from_cdp(&ev.dialog_type),
                message: ev.message,
                // CDP sends an empty string for non-prompt dialogs; normalize
                // to None so the field is meaningful only for `prompt`.
                //
                // Note: this collapses two distinct CDP states into one:
                // (a) `defaultPrompt` field absent (alert/confirm), and
                // (b) `defaultPrompt: ""` (a prompt with empty default).
                // The protocol carries no signal that distinguishes them
                // either — Chrome sends "" in both cases — so the
                // collapse loses nothing observable and gives users one
                // less invariant to track.
                default_prompt: ev.default_prompt.filter(|s| !s.is_empty()),
                url: ev.url,
                session: session_for_dialog,
            });
            // Send is fallible only if the receiver was dropped; in that
            // case the caller no longer cares and we just exit.
            let _ = tx.send(outcome);
        }
    });
    DialogExpectation {
        rx,
        timeout: DEFAULT_EXPECT_TIMEOUT,
        sleep: None,
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    /// Register an expectation, emit a `Page.javascriptDialogOpening`, and
    /// assert the expectation resolves with the decoded [`MatchedDialog`].
    #[tokio::test]
    async fn expect_dialog_resolves_on_javascript_dialog_opened() {
        let (mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session);

        mock.emit_event_for_session(
            "Page.javascriptDialogOpening",
            json!({
                "url": "https://example.com/form",
                "message": "What is your name?",
                "type": "prompt",
                "defaultPrompt": "Anonymous",
                "hasBrowserHandler": false,
            }),
            "S1",
        )
        .await;

        let matched = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s")
            .expect("expectation returned Err");

        assert_eq!(matched.dialog_type, DialogType::Prompt);
        assert_eq!(matched.message, "What is your name?");
        assert_eq!(matched.default_prompt.as_deref(), Some("Anonymous"));
        assert_eq!(matched.url, "https://example.com/form");

        conn.shutdown();
    }

    /// Register an expectation, then simulate a transport teardown
    /// (`AccountedRawEvent::Disconnected`, via `MockConnection::disconnect`)
    /// before any dialog opens. The wait must resolve with
    /// `EventStreamIncomplete`, not `Timeout` — we lost the ability to
    /// observe the event, we didn't observe its genuine absence.
    #[tokio::test]
    async fn expect_dialog_returns_event_stream_incomplete_on_disconnect() {
        let (mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session);

        mock.disconnect();

        let res = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s after disconnect");
        assert!(
            matches!(res, Err(ZendriverError::EventStreamIncomplete)),
            "expected EventStreamIncomplete after transport teardown, got {res:?}",
        );

        conn.shutdown();
    }

    /// Register an expectation with a short timeout, emit no dialog and no
    /// teardown, and assert a genuine no-show still returns
    /// `ZendriverError::Timeout` — the teardown fix must not change this
    /// path.
    #[tokio::test]
    async fn expect_dialog_times_out() {
        let (_mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let expectation = register(&session).timeout(Duration::from_millis(50));

        let res = expectation.await;
        match res {
            Err(ZendriverError::Timeout(d)) => {
                assert_eq!(d, Duration::from_millis(50));
            }
            other => panic!("expected Timeout(50ms), got {other:?}"),
        }

        conn.shutdown();
    }

    /// Call `MatchedDialog::accept(Some("hello"))`, assert the outgoing CDP
    /// request is `Page.handleJavaScriptDialog { accept: true, promptText:
    /// "hello" }`.
    #[tokio::test]
    async fn accept_dispatches_handle_javascript_dialog() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let dialog = MatchedDialog {
            dialog_type: DialogType::Prompt,
            message: "What is your name?".into(),
            default_prompt: Some("Anonymous".into()),
            url: "https://example.com/form".into(),
            session: session.clone(),
        };

        let fut = tokio::spawn(async move { dialog.accept(Some("hello".into())).await });

        let id = mock.expect_cmd("Page.handleJavaScriptDialog").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["accept"], true);
        assert_eq!(sent["params"]["promptText"], "hello");
        mock.reply(id, json!({})).await;

        fut.await
            .expect("accept task panicked")
            .expect("accept returned Err");

        conn.shutdown();
    }
}
