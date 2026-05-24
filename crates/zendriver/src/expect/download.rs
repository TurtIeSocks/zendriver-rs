//! [`DownloadExpectation`] + [`MatchedDownload`] + [`crate::Tab::expect_download`]
//! (gated `expect`).
//!
//! Registers a one-shot subscription against `Page.downloadWillBegin` on a
//! tab's session, resolves with the first download event, and exposes
//! [`MatchedDownload::path`] / [`MatchedDownload::save_to`] for accessing the
//! downloaded file once Chrome reports the transfer complete.
//!
//! Per-Tab setup (allocated lazily on the first [`crate::Tab::expect_download`]
//! call):
//!
//! 1. A [`tempfile::TempDir`] under the OS temp dir to receive Chrome's
//!    writes. Files land there named by their CDP `guid`.
//! 2. `Browser.setDownloadBehavior { behavior: "allowAndName", downloadPath
//!    }` dispatched at browser scope — Chrome routes downloads from this
//!    target into the tempdir. `allowAndName` (vs `allow`) makes Chrome
//!    write to `<downloadPath>/<guid>` rather than the suggested filename,
//!    which avoids collisions when the same name is downloaded twice.
//! 3. A long-running `Page.downloadProgress` subscriber that mutates each
//!    matched download's [`DownloadState`] in place. Lives until the Tab is
//!    dropped.
//!
//! The coordinator is reused across every `expect_download` call on the same
//! tab — held in a [`tokio::sync::OnceCell`] on `TabInner` — so the
//! per-target wiring is paid once.
//!
//! `Page.enable` is already on for every Tab via the P4 frame-lifecycle
//! subscriber, so this module does not re-enable the domain.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::Sleep;
use zendriver_transport::SessionHandle;

use crate::error::{Result, ZendriverError};

/// Default outer timeout for a [`DownloadExpectation`] — matches the rest of
/// the high-level surface (`wait_for_load`, etc).
const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Terminal/in-progress lifecycle marker for a download. Updated on every
/// `Page.downloadProgress` event by the per-Tab subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadProgressState {
    /// Chrome is actively transferring bytes.
    InProgress,
    /// Transfer finished successfully; file at `download_dir/guid` is final.
    Completed,
    /// Transfer was canceled (user cancel, browser shutdown, etc).
    Canceled,
}

impl DownloadProgressState {
    fn from_cdp(s: &str) -> Self {
        match s {
            "completed" => Self::Completed,
            "canceled" => Self::Canceled,
            // CDP only emits the three above; treat anything else as
            // still-receiving rather than inventing an Unknown variant.
            _ => Self::InProgress,
        }
    }
}

/// Snapshot of a download's progress at a point in time. Held behind a
/// [`tokio::sync::Mutex`] on [`MatchedDownload::state`] and mutated in place
/// by the per-Tab `Page.downloadProgress` subscriber.
#[derive(Debug, Clone, Copy)]
pub struct DownloadState {
    /// Bytes received so far.
    pub received_bytes: u64,
    /// Expected total bytes. `0` until Chrome reports a size (some downloads
    /// don't carry a `Content-Length`).
    pub total_bytes: u64,
    /// Lifecycle marker.
    pub state: DownloadProgressState,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            received_bytes: 0,
            total_bytes: 0,
            state: DownloadProgressState::InProgress,
        }
    }
}

/// Type alias for the shared state cell used by both [`MatchedDownload`] and
/// the per-Tab progress subscriber's routing map.
pub(crate) type SharedDownloadState = Arc<tokio::sync::Mutex<DownloadState>>;

/// Per-Tab download coordinator: owns the tempdir Chrome writes into, the
/// routing map from CDP `guid` → shared [`DownloadState`] cells, and is held
/// alive (via the OnceCell on `TabInner`) until the Tab is dropped.
///
/// Constructed lazily on the first [`crate::Tab::expect_download`] call via
/// [`ensure_download_setup`]. The constructor:
///
/// - Allocates a [`TempDir`].
/// - Dispatches `Browser.setDownloadBehavior { behavior: "allowAndName",
///   downloadPath }` at browser scope so Chrome routes downloads from this
///   target into the tempdir.
/// - Spawns a long-running subscriber on `Page.downloadProgress` that walks
///   the routing map on every event and mutates the matching
///   [`DownloadState`] in place.
#[derive(Debug)]
pub(crate) struct DownloadCoordinator {
    /// Backing tempdir. Held to keep the directory alive — drops on Tab
    /// teardown clean up the temp files.
    _tempdir: TempDir,
    /// Resolved download path (a snapshot of `_tempdir.path()`). Cached so
    /// [`MatchedDownload::path`] / [`MatchedDownload::save_to`] don't need
    /// to walk the tempdir handle again.
    download_dir: PathBuf,
    /// Per-guid shared state cells. Inserted on `Page.downloadWillBegin` by
    /// the expectation subscriber; read + mutated by the long-running
    /// progress subscriber.
    states: Arc<tokio::sync::Mutex<HashMap<String, SharedDownloadState>>>,
}

impl DownloadCoordinator {
    fn download_dir(&self) -> &PathBuf {
        &self.download_dir
    }
}

/// Ensure the per-Tab download coordinator is set up, returning a clone of
/// the shared `Arc`. Idempotent — once-cell initialization makes the
/// `Browser.setDownloadBehavior` dispatch + tempdir allocation + progress
/// subscriber spawn happen exactly once per Tab.
///
/// Called from `Tab::expect_download` on every invocation; only the first
/// call performs work.
pub(crate) async fn ensure_download_setup(
    cell: &tokio::sync::OnceCell<Arc<DownloadCoordinator>>,
    session: &SessionHandle,
) -> Result<Arc<DownloadCoordinator>> {
    let coord = cell
        .get_or_try_init(|| async {
            // Allocate a tempdir under the OS default. Naming with a
            // `zendriver-downloads-` prefix makes manual inspection easier
            // when debugging a failing test.
            let tempdir = tempfile::Builder::new()
                .prefix("zendriver-downloads-")
                .tempdir()
                .map_err(ZendriverError::Io)?;
            let download_dir = tempdir.path().to_path_buf();

            // Tell Chrome where to write. `allowAndName` writes files with
            // their CDP `guid` as the on-disk name; that avoids
            // suggested-filename collisions if two downloads share a name.
            // Browser-scope dispatch (no session_id) so the policy applies
            // to all targets — Chrome doesn't honor per-session
            // `setDownloadBehavior` reliably across versions.
            let _ = session
                .connection()
                .call_raw(
                    "Browser.setDownloadBehavior",
                    json!({
                        "behavior": "allowAndName",
                        "downloadPath": download_dir.to_string_lossy().to_string(),
                    }),
                    None,
                )
                .await?;

            let states: Arc<tokio::sync::Mutex<HashMap<String, SharedDownloadState>>> =
                Arc::new(tokio::sync::Mutex::new(HashMap::new()));

            // Spawn the long-running progress subscriber. Lives until the
            // Tab is dropped (which drops the OnceCell + this Arc, and the
            // subscriber exits when the underlying stream closes).
            let states_for_task = states.clone();
            let mut progress_stream =
                session.subscribe::<DownloadProgressEvent>("Page.downloadProgress");
            tokio::spawn(async move {
                while let Some(ev) = progress_stream.next().await {
                    let map = states_for_task.lock().await;
                    if let Some(cell) = map.get(&ev.guid) {
                        let mut s = cell.lock().await;
                        s.received_bytes = ev.received_bytes;
                        s.total_bytes = ev.total_bytes;
                        s.state = DownloadProgressState::from_cdp(&ev.state);
                    }
                }
            });

            Ok::<_, ZendriverError>(Arc::new(DownloadCoordinator {
                _tempdir: tempdir,
                download_dir,
                states,
            }))
        })
        .await?;
    Ok(coord.clone())
}

/// A download observed via `Page.downloadWillBegin`.
///
/// The internal shared state is tracked in the per-Tab download coordinator's
/// routing map; the long-running progress subscriber mutates it as
/// `Page.downloadProgress` events arrive.
///
/// `Debug` is manually implemented since [`SessionHandle`] and
/// [`tokio::sync::Mutex`] don't have useful default debug renderings.
#[derive(Clone)]
pub struct MatchedDownload {
    /// Source URL Chrome started downloading from.
    pub url: String,
    /// Filename Chrome would have used in the user's downloads folder
    /// (extracted from `Content-Disposition` / URL). Useful for callers
    /// that want to preserve the original name in [`Self::save_to`].
    pub suggested_filename: String,
    /// CDP `guid` — Chrome uses this as the on-disk filename inside
    /// [`Self::download_dir`] (because we configured `allowAndName`).
    pub guid: String,
    /// Shared progress state. Mutated in place by the per-Tab progress
    /// subscriber; cloneable so the same handle can be observed concurrently.
    pub state: SharedDownloadState,
    /// Session this download fired on. Retained so future per-tab APIs
    /// (e.g. `cancel`) can dispatch against the correct target — currently
    /// unused but documented for stability.
    pub session: SessionHandle,
    /// Directory Chrome is writing to (the per-Tab tempdir). The final file
    /// lives at `download_dir.join(guid)` once `state.state == Completed`.
    pub download_dir: PathBuf,
}

impl std::fmt::Debug for MatchedDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchedDownload")
            .field("url", &self.url)
            .field("suggested_filename", &self.suggested_filename)
            .field("guid", &self.guid)
            .field("download_dir", &self.download_dir)
            .field("state", &"<Mutex<DownloadState>>")
            .field("session", &"<SessionHandle>")
            .finish()
    }
}

impl MatchedDownload {
    /// Path to the completed download on disk, or `None` if the transfer
    /// hasn't finished yet.
    ///
    /// Locks [`Self::state`] to read the lifecycle marker; the path is only
    /// returned once `state == Completed`. For canceled or in-progress
    /// downloads this returns `None` — callers waiting on completion should
    /// use [`Self::save_to`] which polls under the hood.
    pub async fn path(&self) -> Option<PathBuf> {
        let s = self.state.lock().await;
        if s.state == DownloadProgressState::Completed {
            Some(self.download_dir.join(&self.guid))
        } else {
            None
        }
    }

    /// Wait for the download to complete, then copy the bytes from the
    /// per-Tab tempdir to `dest`. Errors if the download is canceled or the
    /// outer 30s wait elapses without completion.
    ///
    /// `dest` is interpreted as a full filename — the caller is responsible
    /// for joining a directory + [`Self::suggested_filename`] if they want
    /// to preserve Chrome's name.
    ///
    /// Polls [`Self::state`] every 100ms. A 30s outer cap protects against
    /// downloads that hang indefinitely without Chrome reporting progress.
    pub async fn save_to(self, dest: PathBuf) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let snapshot = *self.state.lock().await;
            match snapshot.state {
                DownloadProgressState::Completed => break,
                DownloadProgressState::Canceled => {
                    return Err(ZendriverError::Navigation(format!(
                        "download {} canceled before save_to completed",
                        self.guid
                    )));
                }
                DownloadProgressState::InProgress => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(ZendriverError::Timeout(Duration::from_secs(30)));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
        let src = self.download_dir.join(&self.guid);
        tokio::fs::copy(&src, &dest)
            .await
            .map_err(ZendriverError::Io)?;
        Ok(())
    }
}

/// Awaitable handle returned by [`crate::Tab::expect_download`]. Resolves
/// with the first matched [`MatchedDownload`] or [`ZendriverError::Timeout`]
/// if no download begins within the configured timeout.
///
/// Implements [`Future`] directly — `.await` works without calling
/// `.matched()`. The `.matched()` accessor exists for parity with the
/// Playwright-style fluent API.
#[derive(Debug)]
pub struct DownloadExpectation {
    rx: oneshot::Receiver<MatchedDownload>,
    timeout: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl DownloadExpectation {
    /// Override the default 30s timeout.
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        // Reset any already-armed sleep — the next poll will rebuild it
        // with the new deadline.
        self.sleep = None;
        self
    }

    /// `await` sugar — `expectation.matched().await` reads more like the
    /// Playwright pattern than `expectation.await`. Functionally identical.
    pub async fn matched(self) -> Result<MatchedDownload> {
        self.await
    }
}

impl Future for DownloadExpectation {
    type Output = Result<MatchedDownload>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the oneshot first — if the subscriber already sent, return
        // without ever arming the sleep timer.
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(d)) => return Poll::Ready(Ok(d)),
            Poll::Ready(Err(_)) => {
                // Sender dropped without sending — subscriber task exited
                // (transport closed). Surface as timeout: same observable
                // shape (no event arrived), avoids inventing a new error.
                return Poll::Ready(Err(ZendriverError::Timeout(self.timeout)));
            }
            Poll::Pending => {}
        }

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

/// CDP `Page.downloadWillBegin` payload. Field names follow the protocol
/// (camelCase) via serde rename.
#[derive(Debug, Deserialize)]
struct DownloadWillBeginEvent {
    guid: String,
    url: String,
    #[serde(rename = "suggestedFilename", default)]
    suggested_filename: String,
}

/// CDP `Page.downloadProgress` payload. Drives the long-running subscriber
/// in [`DownloadCoordinator`].
#[derive(Debug, Deserialize)]
struct DownloadProgressEvent {
    guid: String,
    #[serde(rename = "receivedBytes", default)]
    received_bytes: u64,
    #[serde(rename = "totalBytes", default)]
    total_bytes: u64,
    state: String,
}

/// Spawn a one-shot subscriber for `Page.downloadWillBegin` on `session`,
/// wire the matched download's state cell into the coordinator's routing
/// map, and return a [`DownloadExpectation`].
///
/// Subscription registers synchronously before the returned expectation is
/// constructed so downloads triggered immediately after a click cannot slip
/// past us.
pub(crate) fn register(
    session: &SessionHandle,
    coordinator: Arc<DownloadCoordinator>,
) -> DownloadExpectation {
    let (tx, rx) = oneshot::channel();
    let mut stream = session.subscribe::<DownloadWillBeginEvent>("Page.downloadWillBegin");
    let session_for_match = session.clone();
    tokio::spawn(async move {
        if let Some(ev) = stream.next().await {
            // Register the per-download state cell BEFORE handing off the
            // MatchedDownload. The progress subscriber walks this map on
            // every Page.downloadProgress event; missing the registration
            // would drop progress for races where progress fires before
            // the caller awaits the expectation.
            let state: SharedDownloadState =
                Arc::new(tokio::sync::Mutex::new(DownloadState::default()));
            coordinator
                .states
                .lock()
                .await
                .insert(ev.guid.clone(), state.clone());

            let matched = MatchedDownload {
                url: ev.url,
                suggested_filename: ev.suggested_filename,
                guid: ev.guid,
                state,
                session: session_for_match,
                download_dir: coordinator.download_dir().clone(),
            };
            // Send is fallible only if the receiver was dropped; in that
            // case the caller no longer cares and we just exit.
            let _ = tx.send(matched);
        }
    });
    DownloadExpectation {
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

    /// Drive a `register(...)` end-to-end against a [`MockConnection`]:
    ///
    /// 1. Build a coordinator manually (skips the `Browser.setDownloadBehavior`
    ///    + tempdir + subscriber spin-up — those are exercised separately
    ///    via the `Tab::expect_download` integration path).
    /// 2. Register the expectation, emit a `Page.downloadWillBegin`, assert
    ///    the matched download carries the correct `suggested_filename` /
    ///    `guid` / `url`.
    /// 3. Assert `path()` returns `None` before any `Page.downloadProgress`
    ///    arrives (state stays `InProgress`).
    #[tokio::test]
    async fn expect_download_resolves_on_download_will_begin_and_path_is_none_until_completed() {
        let (mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), "S1");

        // Hand-build a coordinator that mirrors the production shape but
        // doesn't dispatch any CDP setup — keeps the test focused on the
        // event → MatchedDownload routing.
        let tempdir = tempfile::tempdir().unwrap();
        let download_dir = tempdir.path().to_path_buf();
        let coord = Arc::new(DownloadCoordinator {
            _tempdir: tempdir,
            download_dir: download_dir.clone(),
            states: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        });

        let expectation = register(&session, coord);

        mock.emit_event_for_session(
            "Page.downloadWillBegin",
            json!({
                "frameId": "F0",
                "guid": "GUID-123",
                "url": "https://example.com/report.pdf",
                "suggestedFilename": "report.pdf",
            }),
            "S1",
        )
        .await;

        let matched = tokio::time::timeout(Duration::from_secs(2), expectation)
            .await
            .expect("expectation did not resolve within 2s")
            .expect("expectation returned Err");

        assert_eq!(matched.guid, "GUID-123");
        assert_eq!(matched.url, "https://example.com/report.pdf");
        assert_eq!(matched.suggested_filename, "report.pdf");
        assert_eq!(matched.download_dir, download_dir);

        // No Page.downloadProgress yet → state stays InProgress → path()
        // must return None even though download_dir/guid would resolve to
        // a syntactically valid path.
        assert!(
            matched.path().await.is_none(),
            "path() must be None until state == Completed",
        );

        conn.shutdown();
    }
}
