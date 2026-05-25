//! Fetch-domain fast-path for Imperva clearance detection.
//!
//! Opt-in via [`ImpervaBypass::with_interception`]. Subscribes to
//! Fetch responses matching `Reese.js` or `_Incapsula_Resource` URL
//! patterns; signals the waiter via a oneshot when a 2xx is observed.
//! Polling continues in parallel — first signal wins.
//!
//! [`ImpervaBypass::with_interception`]: crate::bypass::ImpervaBypass::with_interception

use futures::StreamExt;
use tokio::sync::oneshot;
use zendriver_interception::InterceptBuilder;
use zendriver_transport::SessionHandle;

/// Spawn a background task that signals on first 2xx Imperva-sensor
/// response and returns the receiver half of a oneshot.
///
/// Infallible: the `InterceptBuilder` chain used here (`new` → `pattern` →
/// `at_response` → `subscribe`) is pure sync setup with no `Result`-returning
/// step. The actual `Fetch.enable` CDP round-trip is fire-and-forget inside
/// `subscribe()`; transport errors there are surfaced as a warn log + an
/// empty stream, not as a `spawn_signal` failure. Treating this as `-> _`
/// rather than `Result<_, ImpervaError>` avoids the
/// `clippy::result_large_err` warning that `ImpervaError`'s 136-byte
/// `CallError` variant would otherwise trip for a tiny `Ok` payload.
///
/// Caller must keep the returned [`InterceptionGuard`] alive until they
/// are done with the receiver — dropping it aborts the background task
/// (the stream is owned by the task, so the CDP subscription tears
/// down on the next poll).
pub(crate) fn spawn_signal(session: &SessionHandle) -> (oneshot::Receiver<()>, InterceptionGuard) {
    let (tx, rx) = oneshot::channel();

    // `subscribe()` is sync and returns `impl Stream<Item = PausedRequest>`.
    // `pattern()` returns `Self` (not a `Result`), so no `?` needed on chain.
    let stream = InterceptBuilder::new(session)
        .pattern("*Reese.js*")
        .at_response()
        .pattern("*_Incapsula_Resource*")
        .at_response()
        .subscribe();

    let handle = tokio::spawn(async move {
        let mut stream = Box::pin(stream);
        let mut tx = Some(tx);
        while let Some(paused) = stream.next().await {
            let is_2xx = paused
                .response
                .as_ref()
                .map(|r| (200..300).contains(&r.status))
                .unwrap_or(false);
            // Always release the pause so the page keeps loading.
            let _ = paused.continue_().await;
            if is_2xx {
                if let Some(t) = tx.take() {
                    let _ = t.send(());
                }
                break;
            }
        }
    });

    (
        rx,
        InterceptionGuard {
            handle: Some(handle),
        },
    )
}

/// Guard for the background interception task. Aborts on drop.
pub(crate) struct InterceptionGuard {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for InterceptionGuard {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}
