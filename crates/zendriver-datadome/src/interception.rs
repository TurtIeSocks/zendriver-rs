//! Fetch-domain fast-path for DataDome clearance detection.
//!
//! Opt-in via [`DataDomeBypass::with_interception`]. Subscribes to
//! Fetch responses matching `captcha-delivery.com` or `datadome` URL
//! patterns; signals the waiter via a oneshot when a 2xx is observed.
//! Polling continues in parallel — first signal wins.
//!
//! [`DataDomeBypass::with_interception`]: crate::bypass::DataDomeBypass::with_interception

use futures::StreamExt;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use zendriver_interception::InterceptBuilder;
use zendriver_transport::SessionHandle;

/// Spawn a background task that signals on first 2xx DataDome-sensor
/// response and returns the receiver half of a oneshot.
///
/// Infallible: the `InterceptBuilder` chain used here (`new` → `pattern` →
/// `at_response` → `subscribe`) is pure sync setup with no `Result`-returning
/// step. The actual `Fetch.enable` CDP round-trip is fire-and-forget inside
/// `subscribe()`; transport errors there are surfaced as a warn log + an
/// empty stream, not as a `spawn_signal` failure. Treating this as `-> _`
/// rather than `Result<_, DataDomeError>` avoids the
/// `clippy::result_large_err` warning that `DataDomeError`'s 136-byte
/// `CallError` variant would otherwise trip for a tiny `Ok` payload.
///
/// Caller must keep the returned [`InterceptionGuard`] alive until they are
/// done with the receiver. Dropping the guard cancels the spawned task
/// cooperatively via a `CancellationToken` checked at every loop boundary,
/// then aborts it as a backstop. Cooperative cancel is preferred over a
/// bare `abort()` because abort is asynchronous: the task may run one more
/// `paused.continue_().await` after the abort signal lands — harmless, but
/// the token lets the loop exit cleanly between events instead.
pub(crate) fn spawn_signal(session: &SessionHandle) -> (oneshot::Receiver<()>, InterceptionGuard) {
    let (tx, rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();

    // `subscribe()` is sync and returns `impl Stream<Item = PausedRequest>`.
    // `pattern()` returns `Self` (not a `Result`), so no `?` needed on chain.
    let stream = InterceptBuilder::new(session)
        .pattern("*captcha-delivery.com*")
        .at_response()
        .pattern("*datadome*")
        .at_response()
        .subscribe();

    let handle = tokio::spawn(async move {
        let mut stream = Box::pin(stream);
        let mut tx = Some(tx);
        loop {
            tokio::select! {
                biased;
                () = task_cancel.cancelled() => break,
                next = stream.next() => {
                    let Some(paused) = next else { break };
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
            }
        }
    });

    (
        rx,
        InterceptionGuard {
            cancel,
            handle: Some(handle),
        },
    )
}

/// Guard for the background interception task. On drop, signals
/// cooperative cancellation first (clean exit between events) and then
/// aborts as a backstop in case the task is parked on a non-cancellable
/// future.
pub(crate) struct InterceptionGuard {
    cancel: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for InterceptionGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}
