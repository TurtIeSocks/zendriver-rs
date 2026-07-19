//! [`FileChooserExpectation`] + [`MatchedFileChooser`] +
//! [`crate::Tab::expect_file_chooser`] (gated `expect`).
//!
//! Unlike [`crate::expect::dialog`] / [`crate::expect::download`], this
//! expectation is *active*: matching the event doesn't just hand back data,
//! it drives `DOM.setFileInputFiles` with the paths captured at construction
//! time, then restores normal dialog behavior. This is what lets
//! [`crate::Tab::expect_file_chooser`] answer a **button/label-triggered**
//! file picker (`hiddenInput.click()` from a JS handler, or any custom
//! widget that ultimately opens a native chooser) — a case
//! [`crate::Element::upload_files`] can't reach because it only knows how to
//! dispatch straight at a direct `<input type="file">`'s backend node.
//!
//! CDP sequence:
//!
//! 1. `Page.setInterceptFileChooserDialog { enabled: true }` — dispatched
//!    (and awaited) by [`crate::Tab::expect_file_chooser`] before it hands
//!    back the [`FileChooserExpectation`], so a trigger action issued
//!    immediately after can't race Chrome into opening the real OS dialog
//!    instead of firing the intercept event. This is why
//!    `expect_file_chooser` is `async` (unlike the sync `expect_dialog`
//!    /`expect_request`/`expect_response`, which need no such setup call) —
//!    the same reason [`crate::Tab::expect_download`] is `async` for its own
//!    `Browser.setDownloadBehavior` setup call.
//! 2. Caller triggers the picker (a button/label click that ultimately
//!    clicks a hidden `<input type="file">`, or a direct click on a visible
//!    file input).
//! 3. Chrome emits `Page.fileChooserOpened { frameId, mode, backendNodeId }`
//!    instead of opening the OS dialog. `backendNodeId` is only present when
//!    the chooser was opened via an `<input type="file">` element (per the
//!    CDP protocol docs) — the case this module targets.
//! 4. The subscriber dispatches `DOM.setFileInputFiles { files,
//!    backendNodeId }`, wiring the captured paths into the input's
//!    `FileList` (the page sees a normal `change` event).
//! 5. `Page.setInterceptFileChooserDialog { enabled: false }` restores
//!    normal dialog behavior.
//!
//! **Disable-on-drop:** if the expectation is dropped before a chooser opens
//! (timeout, early return, panic unwind), a best-effort detached
//! `Page.setInterceptFileChooserDialog { enabled: false }` still runs so a
//! later *real* file dialog isn't silently swallowed. Implemented via a
//! [`CancellationToken`] the spawned subscriber task races against
//! `Page.fileChooserOpened` in a `tokio::select!` — mirrors
//! `zendriver_interception::actor::run_actor`'s cancel-races-the-event-stream
//! shape (subscribe before enable; `() = cancel.cancelled() => { disable }`).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::Sleep;
use tokio_util::sync::CancellationToken;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};

/// Default outer timeout for a [`FileChooserExpectation`] — matches the rest
/// of the high-level surface (`wait_for_load`, etc).
const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Picker mode reported by Chrome on `Page.fileChooserOpened`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChooserMode {
    /// `<input type="file">` (no `multiple` attribute).
    SelectSingle,
    /// `<input type="file" multiple>`.
    SelectMultiple,
}

impl FileChooserMode {
    fn from_cdp(s: &str) -> Self {
        match s {
            "selectMultiple" => Self::SelectMultiple,
            // CDP only ever reports the two above; fall back to the more
            // conservative SelectSingle rather than introducing an Unknown
            // variant.
            _ => Self::SelectSingle,
        }
    }
}

/// Outcome of a matched, already-handled file chooser.
///
/// By the time this is returned, the constructor's captured paths have
/// already been wired into the input via `DOM.setFileInputFiles` and the
/// intercept has already been disabled — there is nothing left to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchedFileChooser {
    /// Whether the chooser accepted one file or multiple.
    pub mode: FileChooserMode,
}

/// Awaitable handle returned by [`crate::Tab::expect_file_chooser`].
/// Resolves with [`MatchedFileChooser`] once the chooser has opened *and*
/// been answered, or [`ZendriverError::Timeout`] if no chooser opens within
/// the configured timeout.
///
/// Implements [`Future`] directly — `.await` works without calling
/// `.matched()`. The `.matched()` accessor exists for parity with the
/// Playwright-style fluent API.
#[derive(Debug)]
pub struct FileChooserExpectation {
    rx: oneshot::Receiver<Result<MatchedFileChooser>>,
    timeout: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
    /// Cancelled on `Drop` (including the drop that follows a timeout) so
    /// the spawned subscriber task's `tokio::select!` takes the
    /// cancellation branch and disables the intercept — see the module doc.
    cancel: CancellationToken,
}

impl FileChooserExpectation {
    /// Override the default 30s timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let fc = tab
    ///     .expect_file_chooser(&["/tmp/photo.jpg"])
    ///     .await?
    ///     .timeout(Duration::from_secs(5));
    /// # let _ = fc;
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
    /// let fc = tab.expect_file_chooser(&["/tmp/photo.jpg"]).await?;
    /// let button = tab.find().css("#upload-btn").one().await?;
    /// button.click().await?;
    /// let matched = fc.matched().await?;
    /// # let _ = matched;
    /// # Ok(()) }
    /// ```
    pub async fn matched(self) -> Result<MatchedFileChooser> {
        self.await
    }
}

impl Future for FileChooserExpectation {
    type Output = Result<MatchedFileChooser>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the oneshot first — if the subscriber already sent, return
        // without ever arming the sleep timer.
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(Ok(matched))) => return Poll::Ready(Ok(matched)),
            // Subscriber observed a delivery-loss boundary (disconnect,
            // reconnect, lag, or a same-method decode failure), or the
            // matched chooser had no backendNodeId to answer — see
            // `crate::expect::watch` / `handle_event` below.
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

impl Drop for FileChooserExpectation {
    fn drop(&mut self) {
        // Fire-and-forget: if the chooser already matched (or errored), the
        // spawned task already disabled the intercept and exited — nobody
        // is listening on the token anymore, so this is a no-op. If we're
        // being dropped early (timeout, early return, panic unwind), this
        // unblocks the task's `tokio::select!` cancellation branch, which
        // disables the intercept best-effort so a later real dialog isn't
        // silently swallowed.
        self.cancel.cancel();
    }
}

/// CDP `Page.fileChooserOpened` payload. Field names follow the protocol
/// (camelCase) via serde rename.
///
/// `backend_node_id` is `Option` because CDP only populates it for choosers
/// opened via an `<input type="file">` element — the case this module
/// answers. `frame_id` is intentionally not captured: mirrors
/// [`crate::expect::dialog`]'s lack of a URL filter — any chooser opened
/// during the expectation window matches, there's exactly one paths list to
/// apply.
#[derive(Debug, Deserialize)]
struct FileChooserOpenedEvent {
    mode: String,
    #[serde(rename = "backendNodeId", default)]
    backend_node_id: Option<i64>,
}

/// Handle one matched `Page.fileChooserOpened` event: dispatch
/// `DOM.setFileInputFiles`, then disable the intercept regardless of whether
/// that dispatch succeeded (never leave Chrome's intercept dangling on a
/// partial failure).
async fn handle_event(
    session: &SessionHandle,
    files: Vec<String>,
    res: Result<FileChooserOpenedEvent>,
) -> Result<MatchedFileChooser> {
    let ev = res?;
    let Some(backend_node_id) = ev.backend_node_id else {
        let _ = session
            .call(
                "Page.setInterceptFileChooserDialog",
                json!({ "enabled": false }),
            )
            .await;
        return Err(ZendriverError::FileChooser(
            "Page.fileChooserOpened reported no backendNodeId (only choosers opened via an \
             <input type=\"file\"> element carry one)"
                .to_string(),
        ));
    };
    let mode = FileChooserMode::from_cdp(&ev.mode);

    let set_result = session
        .call(
            "DOM.setFileInputFiles",
            json!({ "files": files, "backendNodeId": backend_node_id }),
        )
        .await;
    // Always disable, even if setFileInputFiles failed — don't leave
    // Chrome's intercept dangling on a partial failure.
    let _ = session
        .call(
            "Page.setInterceptFileChooserDialog",
            json!({ "enabled": false }),
        )
        .await;
    set_result?;
    Ok(MatchedFileChooser { mode })
}

/// Enable the intercept, subscribe to `Page.fileChooserOpened`, and return a
/// [`FileChooserExpectation`] that resolves once the chooser opens and is
/// answered with `files`.
///
/// `async` (unlike `dialog::register`/most `expect_*` registration) because
/// `Page.setInterceptFileChooserDialog { enabled: true }` must reach Chrome
/// before the caller's next line (typically a click) — see the module doc.
/// Subscribes BEFORE dispatching the enable call (mirrors
/// `zendriver_interception::actor::run_actor`'s subscribe-before-enable
/// pattern) so a chooser opened in the round-trip window can't be missed —
/// though in practice the enable call is itself awaited before this
/// function returns, so no chooser can legally open before that point
/// anyway.
pub(crate) async fn register(
    session: &SessionHandle,
    files: Vec<String>,
) -> Result<FileChooserExpectation> {
    let mut stream =
        crate::expect::watch::<FileChooserOpenedEvent>(session, "Page.fileChooserOpened");

    session
        .call(
            "Page.setInterceptFileChooserDialog",
            json!({ "enabled": true }),
        )
        .await?;

    let (tx, rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let session_for_task = session.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = cancel_for_task.cancelled() => {
                // Dropped/timed out before a chooser opened. Best-effort
                // disable so a later real dialog isn't silently swallowed.
                let _ = session_for_task
                    .call(
                        "Page.setInterceptFileChooserDialog",
                        json!({ "enabled": false }),
                    )
                    .await;
            }
            item = stream.next() => {
                if let Some(res) = item {
                    let outcome = handle_event(&session_for_task, files, res).await;
                    // Send is fallible only if the receiver was dropped; in
                    // that case the caller no longer cares and we just exit
                    // (the intercept was already disabled by handle_event).
                    let _ = tx.send(outcome);
                }
                // else: stream ended cleanly with no event (e.g. a clean
                // `Connection::shutdown`) — tx drops, surfacing as Timeout
                // at the Future::poll layer, matching dialog/download.
            }
        }
    });

    Ok(FileChooserExpectation {
        rx,
        timeout: DEFAULT_EXPECT_TIMEOUT,
        sleep: None,
        cancel,
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    /// Drive `register(...)` end-to-end against a [`MockConnection`]:
    /// assert the enable call fires, emit `Page.fileChooserOpened`, assert
    /// the response `DOM.setFileInputFiles` carries the files + backendNodeId,
    /// then assert the disable call fires and the expectation resolves.
    #[tokio::test]
    async fn expect_file_chooser_enables_sets_files_then_disables() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let files = vec!["/tmp/a.txt".to_string(), "/tmp/b.pdf".to_string()];
        let register_fut = {
            let session = session.clone();
            let files = files.clone();
            tokio::spawn(async move { register(&session, files).await })
        };

        // 1. Page.setInterceptFileChooserDialog { enabled: true }.
        let id = mock.expect_cmd("Page.setInterceptFileChooserDialog").await;
        assert_eq!(mock.last_sent()["params"]["enabled"], true);
        mock.reply(id, json!({})).await;

        let expectation = register_fut
            .await
            .expect("register task panicked")
            .expect("register returned Err");

        // 2. Chrome emits Page.fileChooserOpened.
        mock.emit_event_for_session(
            "Page.fileChooserOpened",
            json!({
                "frameId": "F0",
                "mode": "selectMultiple",
                "backendNodeId": 42,
            }),
            "S1",
        )
        .await;

        // 3. DOM.setFileInputFiles { files, backendNodeId }.
        let id = mock.expect_cmd("DOM.setFileInputFiles").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["backendNodeId"], 42);
        let sent_files = sent["params"]["files"].as_array().unwrap();
        assert_eq!(sent_files.len(), 2);
        assert_eq!(sent_files[0], "/tmp/a.txt");
        assert_eq!(sent_files[1], "/tmp/b.pdf");
        mock.reply(id, json!({})).await;

        // 4. Page.setInterceptFileChooserDialog { enabled: false }.
        let id = mock.expect_cmd("Page.setInterceptFileChooserDialog").await;
        assert_eq!(mock.last_sent()["params"]["enabled"], false);
        mock.reply(id, json!({})).await;

        let matched = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s")
            .expect("expectation returned Err");
        assert_eq!(matched.mode, FileChooserMode::SelectMultiple);

        conn.shutdown();
    }

    /// Register an expectation, drop it before any chooser opens (mimics a
    /// timeout / early return), and assert a best-effort
    /// `Page.setInterceptFileChooserDialog { enabled: false }` still fires —
    /// the disable-on-drop safety net from the module doc.
    #[tokio::test]
    async fn expect_file_chooser_disables_on_drop_without_matching() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let register_fut = {
            let session = session.clone();
            tokio::spawn(async move { register(&session, vec!["/tmp/a.txt".to_string()]).await })
        };

        let id = mock.expect_cmd("Page.setInterceptFileChooserDialog").await;
        assert_eq!(mock.last_sent()["params"]["enabled"], true);
        mock.reply(id, json!({})).await;

        let expectation = register_fut
            .await
            .expect("register task panicked")
            .expect("register returned Err");

        // Drop without a matching Page.fileChooserOpened.
        drop(expectation);

        let id = tokio::time::timeout(
            Duration::from_secs(2),
            mock.expect_cmd("Page.setInterceptFileChooserDialog"),
        )
        .await
        .expect("disable-on-drop did not fire within 2s");
        assert_eq!(mock.last_sent()["params"]["enabled"], false);
        mock.reply(id, json!({})).await;

        conn.shutdown();
    }

    /// Register an expectation with a short timeout, emit no
    /// `Page.fileChooserOpened` and no teardown, and assert a genuine
    /// no-show still returns `ZendriverError::Timeout`.
    #[tokio::test]
    async fn expect_file_chooser_times_out() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let register_fut = {
            let session = session.clone();
            tokio::spawn(async move { register(&session, vec!["/tmp/a.txt".to_string()]).await })
        };

        let id = mock.expect_cmd("Page.setInterceptFileChooserDialog").await;
        mock.reply(id, json!({})).await;

        let expectation = register_fut
            .await
            .expect("register task panicked")
            .expect("register returned Err")
            .timeout(Duration::from_millis(50));

        let res = expectation.await;
        match res {
            Err(ZendriverError::Timeout(d)) => {
                assert_eq!(d, Duration::from_millis(50));
            }
            other => panic!("expected Timeout(50ms), got {other:?}"),
        }

        conn.shutdown();
    }

    /// A `Page.fileChooserOpened` event with no `backendNodeId` (a chooser
    /// not backed by an `<input type="file">` element) can't be answered —
    /// assert it still disables the intercept and surfaces
    /// `ZendriverError::FileChooser` rather than hanging or panicking.
    #[tokio::test]
    async fn expect_file_chooser_without_backend_node_id_disables_and_errors() {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        let register_fut = {
            let session = session.clone();
            tokio::spawn(async move { register(&session, vec!["/tmp/a.txt".to_string()]).await })
        };

        let id = mock.expect_cmd("Page.setInterceptFileChooserDialog").await;
        mock.reply(id, json!({})).await;

        let expectation = register_fut
            .await
            .expect("register task panicked")
            .expect("register returned Err");

        mock.emit_event_for_session(
            "Page.fileChooserOpened",
            json!({ "frameId": "F0", "mode": "selectSingle" }),
            "S1",
        )
        .await;

        let id = mock.expect_cmd("Page.setInterceptFileChooserDialog").await;
        assert_eq!(mock.last_sent()["params"]["enabled"], false);
        mock.reply(id, json!({})).await;

        let res = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s");
        assert!(
            matches!(res, Err(ZendriverError::FileChooser(_))),
            "expected FileChooser error, got {res:?}",
        );

        conn.shutdown();
    }
}
