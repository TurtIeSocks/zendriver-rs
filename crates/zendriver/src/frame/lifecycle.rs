//! Frame lifecycle event subscriber — internal.
//!
//! Spawned per [`crate::Tab`] at construction time. Subscribes to
//! `Page.frameAttached`, `Page.frameDetached`, and `Page.frameNavigated`
//! events on the owning tab's session and maintains the tab's frames
//! registry (read via [`crate::Tab::frames`]).
//!
//! ## Wiring
//!
//! Each [`crate::Tab`] owns one background task. At construction time the
//! task:
//! 1. Subscribes to the three `Page.frame*` event streams BEFORE awaiting
//!    `Page.enable` — avoids the race where Chrome fires events between
//!    the enable reply and our subscription registration.
//! 2. Fires `Page.enable` once on the tab's session so Chrome starts
//!    emitting `Page.frame*` events.
//! 3. Loops over the merged event stream, mutating the registry:
//!    - `Page.frameAttached` — construct a new [`crate::Frame`] sharing
//!      the tab's session (same-origin sub-frame; OOPIF frames arrive
//!      through the [`crate::frame::oopif`] observer path on their own
//!      child session, NOT this stream), insert it under `frameId`, and
//!      **spawn** `sweep_dead_provisional_siblings` to evict any dead
//!      provisional sibling it supersedes. That sweep issues a
//!      `Page.getFrameTree` round-trip, and it is spawned rather than
//!      awaited here on purpose — see the note below.
//!    - `Page.frameNavigated` — update the existing entry's URL in
//!      place (and backfill the frame `name` the attach event could not
//!      carry); insert a fresh [`crate::Frame`] if no entry exists.
//!    - `Page.frameDetached` — remove the entry from the registry.
//!
//! ## Nothing in an arm may await a CDP round-trip
//!
//! A `tokio::select!` arm is not a background job: while an arm's body is
//! pending the loop cannot advance, so no other frame event is processed.
//! That is worse than a delay. The event source underneath is
//! [`zendriver_transport::Connection::subscribe_raw`], a broadcast channel
//! carrying *every* raw CDP event, and a subscriber that lags past its
//! capacity has frames dropped silently. A stalled arm on a busy page
//! therefore loses `Page.frameDetached` events outright, leaving the
//! registry reporting frames that no longer exist — with no error anywhere.
//! Keep the arms to registry mutation; spawn anything that talks to Chrome.
//!
//! The task runs until its [`tokio_util::sync::CancellationToken`] fires —
//! typically when the owning Tab is dropped. Everything it spawns (the
//! startup `Page.enable`, and each sweep from the attach arm) observes the
//! same token, so nothing outlives it waiting out a CDP budget.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use zendriver_transport::SessionHandle;

use crate::frame::{Frame, FrameInner};
use crate::tab::TabInner;

/// Wait budget for the `Page.getFrameTree` probe that confirms a
/// same-parent sibling really is a dead provisional frame.
///
/// This bound does **not** protect the event loop — the sweep runs in its
/// own task, which is what protects the loop. It caps how long a wedged
/// probe keeps that task (and its clone of the registry `Arc`) alive. On
/// expiry the sweep fails open: nothing is evicted.
const PROVISIONAL_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimal projection of `Page.frameAttached` — only the fields we need
/// to construct a [`Frame`]. URL/name are not on this payload; they arrive
/// on a subsequent `Page.frameNavigated`.
#[derive(Debug, Deserialize)]
struct FrameAttachedEvent {
    #[serde(rename = "frameId")]
    frame_id: String,
    #[serde(rename = "parentFrameId")]
    parent_frame_id: Option<String>,
}

/// Minimal projection of `Page.frameDetached` — only `frameId` matters for
/// registry removal.
#[derive(Debug, Deserialize)]
struct FrameDetachedEvent {
    #[serde(rename = "frameId")]
    frame_id: String,
}

/// Minimal projection of `Page.frameNavigated`. Chrome nests the frame
/// metadata under `frame: {...}`.
#[derive(Debug, Deserialize)]
struct FrameNavigatedEvent {
    frame: NavigatedFrameInner,
}

#[derive(Debug, Deserialize)]
struct NavigatedFrameInner {
    id: String,
    #[serde(rename = "parentId", default)]
    parent_id: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    name: Option<String>,
}

/// Drive the lifecycle subscriber until `cancel` fires.
///
/// `frames` is the [`RwLock`]-protected registry shared with the owning
/// [`TabInner`]. `tab_weak` is the [`Weak<TabInner>`] handed to each
/// constructed [`Frame`] so its `tab_for_synthesize` upgrade can return
/// the owning `Tab`.
///
/// Same fire-and-forget posture as
/// [`crate::network_idle::InFlightTracker::run`] for the `Page.enable`
/// call: failure is logged + ignored so the subscriber keeps running on a
/// previously-enabled Page domain.
pub(crate) async fn run(
    session: SessionHandle,
    frames: Arc<RwLock<HashMap<String, Frame>>>,
    tab_weak: Weak<TabInner>,
    cancel: CancellationToken,
) {
    let mut attached = session.subscribe::<FrameAttachedEvent>("Page.frameAttached");
    let mut detached = session.subscribe::<FrameDetachedEvent>("Page.frameDetached");
    let mut navigated = session.subscribe::<FrameNavigatedEvent>("Page.frameNavigated");

    // Fire-and-forget `Page.enable`. Same rationale as
    // `InFlightTracker::run`: the mock harness never replies to this
    // call, and in production the subscribe streams above are already
    // registered, so awaiting the response would only serialize the
    // first arriving event behind the enable round-trip.
    //
    // Cancellation-aware for the same reason the sweep is: an unanswered
    // call sits on the transport's default budget
    // ([`zendriver_transport::DEFAULT_CALL_TIMEOUT`], 180s), and nothing
    // spawned here may outlive the Tab that owns it by that much while
    // holding a session clone.
    let enable_session = session.clone();
    let enable_cancel = cancel.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = enable_cancel.cancelled() => {}
            res = enable_session.call("Page.enable", serde_json::json!({})) => {
                if let Err(e) = res {
                    warn!(error = %e, "frame::lifecycle: Page.enable failed; frame events may be inactive");
                }
            }
        }
    });

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                trace!("frame::lifecycle: cancellation received, exiting");
                return;
            }
            Some(ev) = attached.next() => {
                let frame = Frame::new(
                    ev.frame_id.clone(),
                    ev.parent_frame_id.clone(),
                    String::new(),
                    None,
                    session.clone(),
                    tab_weak.clone(),
                );
                frames.write().await.insert(ev.frame_id.clone(), frame);
                // Chrome occasionally fires `frameAttached` twice for a
                // single iframe — once with a provisional `frameId` and
                // again with the committed one — without an intervening
                // `frameDetached`. Observed on `srcdoc` iframes under
                // `--headless=new`. Evict those dead entries so the
                // registry doesn't accumulate frames that
                // `Page.createIsolatedWorld` would reject with
                // "No frame for given id found".
                //
                // Spawned, never awaited here: distinguishing a dead
                // provisional from a sibling that simply has not navigated
                // yet needs a `Page.getFrameTree` round-trip, and awaiting
                // one in this arm parks every other frame event behind it
                // (see the module header). Nothing waits on the eviction —
                // it is pure housekeeping, and a stale row is recoverable.
                if let Some(parent) = ev.parent_frame_id {
                    let session = session.clone();
                    let frames = frames.clone();
                    let cancel = cancel.clone();
                    let attached_id = ev.frame_id;
                    tokio::spawn(async move {
                        tokio::select! {
                            () = cancel.cancelled() => {}
                            () = sweep_dead_provisional_siblings(
                                &session, &frames, &attached_id, &parent,
                            ) => {}
                        }
                    });
                }
            }
            Some(ev) = navigated.next() => {
                let frame_id = ev.frame.id;
                let new_url = ev.frame.url;
                // `Page.frameAttached` carries no name, so an entry created
                // from it always starts nameless. Treat a non-empty name on
                // `frameNavigated` as the authoritative one; never clear a
                // known name from an event that simply omits the field.
                let new_name = ev.frame.name.filter(|n| !n.is_empty());

                // Fast path, under a READ guard: refreshing a URL mutates
                // the `Frame`'s own `RwLock`, not the registry map, so the
                // ordinary navigation never needs exclusive access to the
                // registry. Only the rename below replaces a map entry.
                let needs_rename = {
                    let map = frames.read().await;
                    match map.get(&frame_id) {
                        Some(existing) => {
                            let name_changed = new_name.is_some()
                                && new_name.as_deref() != existing.inner.name.as_deref();
                            // Write through the shared `Arc` on BOTH paths.
                            // Every `Frame` handle a caller already took
                            // points at THIS inner, and the rename below
                            // swaps the registry row for a fresh one those
                            // handles never see. Writing first means a
                            // pre-rename handle at least observes the
                            // navigation that carries the rename.
                            //
                            // It does not survive the swap. The NEXT
                            // `frameNavigated` resolves the new row and
                            // writes THAT inner, so a handle taken before
                            // the rename freezes at this URL forever. The
                            // repair is one navigation deep, not permanent;
                            // closing it needs interior-mutable
                            // `FrameInner::name`, which is a public break a
                            // maintainer has to sign off (see the ignored
                            // two-navigation test in this module).
                            *existing.inner.url.write().await = new_url.clone();
                            if !name_changed {
                                continue;
                            }
                            true
                        }
                        // No entry: treat the navigation as the implicit
                        // attach (Chrome may emit `frameNavigated` for the
                        // main frame before any subscriber sees an explicit
                        // `frameAttached`).
                        None => false,
                    }
                };

                let mut map = frames.write().await;
                if needs_rename {
                    // Re-look up. The registry is mutated concurrently by
                    // the spawned provisional sweep and by the OOPIF
                    // observer, so the row seen under the read guard may be
                    // gone by now. A name backfill is a refinement, never a
                    // reason to resurrect a row something else deliberately
                    // removed.
                    let Some(existing) = map.get(&frame_id) else { continue };
                    // `FrameInner::name` is immutable (it backs
                    // `Frame::name() -> Option<&str>`, which cannot hand out
                    // a lock guard), so backfilling means replacing the
                    // registry entry. Everything else is carried over —
                    // notably the session, which for an OOPIF is a child
                    // session and NOT this subscriber's.
                    let renamed = Frame::new(
                        frame_id.clone(),
                        existing.inner.parent_frame_id.clone(),
                        new_url,
                        new_name,
                        existing.inner.session.clone(),
                        tab_weak.clone(),
                    );
                    map.insert(frame_id, renamed);
                } else {
                    // Re-look up, for the same reason the rename path does.
                    // The read guard said "no entry", but the OOPIF observer
                    // and the attach arm both insert into this map, and an
                    // OOPIF row carries a CHILD session. Inserting blind
                    // would overwrite one with a `Frame` on the parent
                    // session, sending every later call on that frame to the
                    // wrong target — so a row that appeared in the gap is
                    // refreshed in place instead.
                    //
                    // A name on this event is left to the next navigation:
                    // backfilling it means swapping the row, which is the
                    // thing being avoided here, and the rename path above
                    // does it correctly once the row is visible to a read
                    // guard.
                    let raced = map.get(&frame_id).map(|f| Arc::clone(&f.inner));
                    match raced {
                        Some(inner) => {
                            drop(map);
                            *inner.url.write().await = new_url;
                        }
                        None => {
                            let frame = Frame::new(
                                frame_id.clone(),
                                ev.frame.parent_id,
                                new_url,
                                new_name,
                                session.clone(),
                                tab_weak.clone(),
                            );
                            map.insert(frame_id, frame);
                        }
                    }
                }
            }
            Some(ev) = detached.next() => {
                frames.write().await.remove(&ev.frame_id);
            }
            else => {
                trace!("frame::lifecycle: all event streams closed, exiting");
                return;
            }
        }
    }
}

/// Evict the registry entries that the freshly-attached `attached_id`
/// supersedes: same parent, same session, still URL-less — **and confirmed
/// gone from Chrome's live frame tree**.
///
/// The last clause is what separates a dead provisional frame from a
/// perfectly good sibling that simply has not navigated yet. Both look
/// identical in the event stream (a second `frameAttached` under one
/// parent, no `frameNavigated` on the first), so the registry alone cannot
/// tell them apart; `Page.getFrameTree` can, because Chrome drops a
/// provisional frame from the tree the moment it commits the real one —
/// the same reason `Page.createIsolatedWorld` answers "No frame for given
/// id found" for it.
///
/// ## Why the candidate snapshot is taken before the probe
///
/// The probe answers a question about one instant, and the answer is only
/// sound for rows that already existed at that instant. Taking the
/// candidate snapshot first guarantees the returned tree is at least as
/// recent as the snapshot, so a candidate missing from it is a frame that
/// either never made the tree or died before it was taken — both dead.
/// Collecting candidates *after* the probe would invert that: a frame
/// attached in the gap would be a candidate that is legitimately absent
/// from an older tree, and evicting it would drop a live frame. The
/// re-validation below then re-checks each survivor against the registry
/// as it stands at eviction time, because the round-trip is a real window
/// in which rows can be detached, replaced, or navigated.
///
/// ## Failing open
///
/// Every ambiguity this can see is resolved by keeping the row: no
/// candidates, a probe error or timeout, a row whose identity changed
/// under the probe, a row that navigated while it was outstanding. A stale
/// row in [`crate::Tab::frames`] only costs a lookup that
/// [`crate::Frame::ensure_isolated_world`] already recovers from, whereas
/// evicting a live frame loses a handle the caller may still hold.
///
/// One ambiguity it cannot see: a frame that is genuinely alive but absent
/// from *this* session's `Page.getFrameTree` is evicted anyway. That covers
/// two unverified cases, and neither is helped by the re-validation, since
/// both look exactly like a dead provisional from here.
///
/// The first is a frame Chrome has announced via `Page.frameAttached` but
/// not yet listed in the tree, if such a window exists. Running the sweep
/// off the event loop widens that window slightly, because the candidate
/// snapshot is now taken whenever the spawned task is first polled rather
/// than synchronously in the arm.
///
/// The second is an out-of-process iframe. The session-id filter below
/// only excludes entries already re-homed onto a child session, so a
/// parent-side placeholder for an OOPIF would still be swept if Chrome
/// omits remote children from the parent target's tree. Whether it does is
/// unverified against a real
/// browser; if it turns out to, this needs an OOPIF-aware filter or a
/// grace period before a url-less candidate becomes eligible.
async fn sweep_dead_provisional_siblings(
    session: &SessionHandle,
    frames: &Arc<RwLock<HashMap<String, Frame>>>,
    attached_id: &str,
    parent: &str,
) {
    // Each candidate is carried as (id, `Arc` identity). The identity is
    // what lets the re-validation below tell "the row I sampled" from "some
    // other row that now holds this id".
    let candidates: Vec<(String, Arc<FrameInner>)> = {
        let map = frames.read().await;
        map.iter()
            .filter_map(|(id, f)| {
                if id == attached_id {
                    return None;
                }
                if f.inner.parent_frame_id.as_deref() != Some(parent) {
                    return None;
                }
                // OOPIF entries live on their own child session and are
                // absent from THIS session's frame tree, so the probe below
                // would read as "gone" for every one of them.
                if f.inner.session.session_id() != session.session_id() {
                    return None;
                }
                let url_slot = f.inner.url.try_read().ok()?;
                url_slot
                    .is_empty()
                    .then(|| (id.clone(), Arc::clone(&f.inner)))
            })
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let Some(live) = live_frame_ids(session).await else {
        return;
    };

    let mut map = frames.write().await;
    for (id, sampled) in candidates {
        if live.contains(&id) {
            continue;
        }
        let Some(current) = map.get(&id) else {
            continue;
        };
        // A different `Arc` behind the same id means the row we sampled is
        // already gone and something re-inserted under its id — a detach
        // plus re-attach, or the `frameNavigated` rename swap. Evicting now
        // would delete a frame the probe never asked about. Identity also
        // subsumes re-checking `parent_frame_id` and `session`, which are
        // immutable on `FrameInner`.
        if !Arc::ptr_eq(&current.inner, &sampled) {
            continue;
        }
        // A URL means the frame navigated while the probe was in flight,
        // and only a live frame navigates. `try_read` rather than `read`:
        // this runs under the registry write guard, so it must not block on
        // a slot the navigated arm is writing — and failing open there is
        // the correct answer anyway.
        if !current.inner.url.try_read().is_ok_and(|u| u.is_empty()) {
            continue;
        }
        map.remove(&id);
    }
}

/// Every frame id Chrome currently reports under `Page.getFrameTree`, or
/// `None` if the probe failed or outran [`PROVISIONAL_PROBE_TIMEOUT`].
async fn live_frame_ids(session: &SessionHandle) -> Option<HashSet<String>> {
    let tree = match tokio::time::timeout(
        PROVISIONAL_PROBE_TIMEOUT,
        session.call("Page.getFrameTree", serde_json::json!({})),
    )
    .await
    {
        Ok(Ok(tree)) => tree,
        Ok(Err(e)) => {
            warn!(error = %e, "frame::lifecycle: Page.getFrameTree failed; keeping possibly-stale sibling frames");
            return None;
        }
        Err(_) => {
            warn!(
                "frame::lifecycle: Page.getFrameTree timed out; keeping possibly-stale sibling frames"
            );
            return None;
        }
    };
    let mut ids = HashSet::new();
    crate::frame::tree::walk(&tree["frameTree"], &mut |node| {
        ids.insert(node.id.to_string());
    });
    Some(ids)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::Connection;
    use zendriver_transport::testing::MockConnection;

    type Registry = Arc<RwLock<HashMap<String, Frame>>>;

    /// Session id every test drives events on — the subscriber only sees
    /// events routed to its own session.
    const SESSION: &str = "S1";

    /// Spawn the subscriber on a mock connection and drain its startup
    /// `Page.enable`. Once that command has landed the three `Page.frame*`
    /// subscriptions are registered, so any later `emit_event_for_session`
    /// reaches the loop.
    async fn start() -> (MockConnection, Connection, Registry, CancellationToken) {
        let (mut mock, conn) = MockConnection::pair();
        let session = SessionHandle::new(conn.clone(), SESSION);
        let frames: Registry = Arc::new(RwLock::new(HashMap::new()));
        let cancel = CancellationToken::new();
        tokio::spawn(run(session, frames.clone(), Weak::new(), cancel.clone()));
        let id = tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.enable"))
            .await
            .expect("frame lifecycle did not send Page.enable within 2s");
        mock.reply(id, json!({})).await;
        (mock, conn, frames, cancel)
    }

    async fn attach(mock: &MockConnection, frame_id: &str, parent: &str) {
        mock.emit_event_for_session(
            "Page.frameAttached",
            json!({ "frameId": frame_id, "parentFrameId": parent }),
            SESSION,
        )
        .await;
    }

    /// One `childFrames` entry for a canned `Page.getFrameTree` reply.
    fn tree_child(id: &str, parent: &str) -> serde_json::Value {
        json!({ "frame": { "id": id, "parentId": parent, "url": "" } })
    }

    /// Poll the registry until `pred` holds, then return a snapshot. Panics
    /// on timeout rather than asserting on a half-applied event.
    async fn wait_for(
        frames: &Registry,
        pred: impl Fn(&HashMap<String, Frame>) -> bool,
    ) -> HashMap<String, Frame> {
        wait_for_within(frames, Duration::from_secs(1), pred).await
    }

    /// [`wait_for`] with an explicit budget, for assertions where the
    /// *deadline itself* is the thing under test.
    async fn wait_for_within(
        frames: &Registry,
        budget: Duration,
        pred: impl Fn(&HashMap<String, Frame>) -> bool,
    ) -> HashMap<String, Frame> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            {
                let map = frames.read().await;
                if pred(&map) {
                    return map.clone();
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "registry never reached the expected state within {budget:?}",
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Give a spawned sweep room to finish, for the assertions whose
    /// expected outcome is that it did nothing.
    ///
    /// The sweep runs in its own task, so no registry state proves it has
    /// run: every row it might evict is already in the map before it is
    /// spawned, and the row it leaves alone looks the same before and
    /// after. A negative therefore has no event to wait on. Past the probe
    /// reply the sweep is one lock acquisition from done, so this window is
    /// orders of magnitude more than it needs.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    /// Emit a `Page.frameNavigated` for `frame_id` on the test session.
    async fn navigate(mock: &MockConnection, frame_id: &str, url: &str, name: Option<&str>) {
        mock.emit_event_for_session(
            "Page.frameNavigated",
            json!({
                "frame": { "id": frame_id, "parentId": "P", "url": url, "name": name }
            }),
            SESSION,
        )
        .await;
    }

    /// Wait for the sweep's `Page.getFrameTree` probe and return its command
    /// id. `expect_cmd` has no timeout of its own and silently discards
    /// frames that do not match, so every wait on it is wrapped.
    async fn expect_probe(mock: &mut MockConnection) -> u64 {
        tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.getFrameTree"))
            .await
            .expect("attach with an url-less sibling did not probe Page.getFrameTree")
    }

    /// Two real iframes under one parent, neither navigated yet, are
    /// indistinguishable from the provisional double-attach in the event
    /// stream alone — but Chrome's frame tree still lists both, so the
    /// second attach must NOT evict the first.
    #[tokio::test]
    async fn unnavigated_sibling_present_in_frame_tree_survives_a_second_attach() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "F1", "P").await;
        wait_for(&frames, |m| m.contains_key("F1")).await;
        attach(&mock, "F2", "P").await;

        let id = expect_probe(&mut mock).await;
        mock.reply(
            id,
            json!({
                "frameTree": {
                    "frame": { "id": "P", "url": "https://host.test/" },
                    "childFrames": [tree_child("F1", "P"), tree_child("F2", "P")],
                }
            }),
        )
        .await;

        settle().await;
        let map = frames.read().await.clone();
        assert!(
            map.contains_key("F1"),
            "a sibling Chrome still lists in the frame tree must not be swept",
        );
        assert_eq!(map.len(), 2);

        cancel.cancel();
        conn.shutdown();
    }

    /// The quirk the sweep exists for: Chrome re-attaches one iframe under a
    /// fresh id and drops the provisional from its frame tree. That entry is
    /// dead and must go.
    #[tokio::test]
    async fn provisional_sibling_absent_from_frame_tree_is_evicted() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "PROVISIONAL", "P").await;
        wait_for(&frames, |m| m.contains_key("PROVISIONAL")).await;
        attach(&mock, "COMMITTED", "P").await;

        let id = expect_probe(&mut mock).await;
        mock.reply(
            id,
            json!({
                "frameTree": {
                    "frame": { "id": "P", "url": "https://host.test/" },
                    "childFrames": [tree_child("COMMITTED", "P")],
                }
            }),
        )
        .await;

        let map = wait_for(&frames, |m| !m.contains_key("PROVISIONAL")).await;
        assert!(map.contains_key("COMMITTED"));
        assert_eq!(map.len(), 1);

        cancel.cancel();
        conn.shutdown();
    }

    /// A failed probe must fail OPEN. Dropping a live frame loses a handle;
    /// keeping a dead one only leaves a stale row that
    /// `ensure_isolated_world` recovers from.
    #[tokio::test]
    async fn failed_frame_tree_probe_keeps_the_sibling() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "F1", "P").await;
        wait_for(&frames, |m| m.contains_key("F1")).await;
        attach(&mock, "F2", "P").await;

        let id = expect_probe(&mut mock).await;
        mock.reply_err(id, -32000, "Page domain not enabled").await;

        settle().await;
        let map = frames.read().await.clone();
        assert!(
            map.contains_key("F1"),
            "an unanswerable probe must not authorize an eviction",
        );
        assert!(map.contains_key("F2"));

        cancel.cancel();
        conn.shutdown();
    }

    /// **The regression the spawn exists for.** With a probe deliberately
    /// left unanswered, an unrelated `Page.frameDetached` must still be
    /// applied promptly rather than waiting out
    /// [`PROVISIONAL_PROBE_TIMEOUT`].
    ///
    /// Awaiting the probe inside the `frameAttached` arm parks the whole
    /// loop, and that is worse than a delay: the subscriber sits on a
    /// bounded broadcast that drops frames for a lagging receiver
    /// *silently*, so on a busy page a five-second stall does not delay the
    /// detach, it loses it — leaving `Tab::frames` reporting a frame that no
    /// longer exists, with nothing logged. Hence the 500ms budget: it is an
    /// order of magnitude below the probe timeout, so a pass cannot be the
    /// probe giving up.
    #[tokio::test]
    async fn events_are_processed_while_a_frame_tree_probe_is_outstanding() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "F1", "P").await;
        wait_for(&frames, |m| m.contains_key("F1")).await;
        // The second attach makes url-less F1 a sweep candidate, which is
        // what dispatches the probe. It is never replied to.
        attach(&mock, "F2", "P").await;
        let _wedged = expect_probe(&mut mock).await;

        mock.emit_event_for_session("Page.frameDetached", json!({ "frameId": "F1" }), SESSION)
            .await;

        let map = wait_for_within(&frames, Duration::from_millis(500), |m| {
            !m.contains_key("F1")
        })
        .await;
        assert!(
            map.contains_key("F2"),
            "the loop must still be serving the registry, not just draining it",
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// The probe answers a question about one instant, but the eviction
    /// happens a round-trip later. A candidate that navigated while the
    /// probe was outstanding is provably alive — only a live frame
    /// navigates — so the deferred sweep must re-check the registry at
    /// eviction time instead of trusting its pre-probe snapshot.
    #[tokio::test]
    async fn a_candidate_that_navigates_during_the_probe_is_not_evicted() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "F1", "P").await;
        wait_for(&frames, |m| m.contains_key("F1")).await;
        attach(&mock, "F2", "P").await;
        // Waiting for the probe proves the candidate snapshot (which
        // precedes it) already captured a url-less F1.
        let id = expect_probe(&mut mock).await;

        navigate(&mock, "F1", "https://host.test/one", None).await;
        wait_for(&frames, |m| {
            m.get("F1")
                .is_some_and(|f| f.inner.url.try_read().is_ok_and(|u| !u.is_empty()))
        })
        .await;

        // Answer with the tree as it stood BEFORE that navigation: F1
        // absent. The snapshot says evict; the registry says otherwise, and
        // the registry is the newer of the two.
        mock.reply(
            id,
            json!({
                "frameTree": {
                    "frame": { "id": "P", "url": "https://host.test/" },
                    "childFrames": [tree_child("F2", "P")],
                }
            }),
        )
        .await;

        settle().await;
        assert!(
            frames.read().await.contains_key("F1"),
            "a candidate that navigated under the probe must survive the sweep",
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// The other half of re-validation: a candidate that was detached and
    /// re-attached under the *same* id while the probe was outstanding is a
    /// different, live frame. It is still url-less, so the url re-check
    /// cannot save it — only comparing the row's `Arc` identity against the
    /// one actually sampled can.
    #[tokio::test]
    async fn a_candidate_reattached_under_the_same_id_during_the_probe_is_not_evicted() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "F1", "P").await;
        wait_for(&frames, |m| m.contains_key("F1")).await;
        attach(&mock, "F2", "P").await;
        // Dispatched => the sweep has already sampled the original F1 row.
        let stale_probe = expect_probe(&mut mock).await;

        mock.emit_event_for_session("Page.frameDetached", json!({ "frameId": "F1" }), SESSION)
            .await;
        wait_for(&frames, |m| !m.contains_key("F1")).await;
        let sampled = Arc::clone(&wait_for(&frames, |m| m.contains_key("F2")).await["F2"].inner);
        attach(&mock, "F1", "P").await;
        let live_row = wait_for(&frames, |m| m.contains_key("F1")).await["F1"]
            .inner
            .clone();

        // Answer the FIRST probe with the tree as it stood before the
        // re-attach. Its snapshot named F1; the row under that id is no
        // longer the one it named.
        mock.reply(
            stale_probe,
            json!({
                "frameTree": {
                    "frame": { "id": "P", "url": "https://host.test/" },
                    "childFrames": [tree_child("F2", "P")],
                }
            }),
        )
        .await;

        settle().await;
        let map = frames.read().await;
        assert!(
            map.get("F1")
                .is_some_and(|f| Arc::ptr_eq(&f.inner, &live_row)),
            "the re-attached F1 must survive a sweep that sampled its predecessor",
        );
        assert!(Arc::ptr_eq(&map["F2"].inner, &sampled), "F2 is untouched");
        drop(map);

        cancel.cancel();
        conn.shutdown();
    }

    /// A single child needs no probe at all — no `Page.getFrameTree` is
    /// dispatched, so a page with one iframe never pays for the sweep. (A
    /// second iframe under the same parent does dispatch one; it just no
    /// longer costs the event loop anything.)
    #[tokio::test]
    async fn lone_attach_does_not_probe_the_frame_tree() {
        let (mut mock, conn, frames, cancel) = start().await;

        attach(&mock, "F1", "P").await;
        wait_for(&frames, |m| m.contains_key("F1")).await;

        assert_eq!(
            mock.try_recv_cmd(),
            None,
            "no sibling candidates means no frame-tree probe",
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// `Page.frameAttached` carries no name, so a named iframe is nameless
    /// in the registry until `Page.frameNavigated` supplies one. Dropping it
    /// there leaves `Tab::frame_by_name` unable to find the frame forever.
    #[tokio::test]
    async fn navigated_backfills_the_frame_name_onto_an_attached_entry() {
        let (mock, conn, frames, cancel) = start().await;

        attach(&mock, "FCHILD", "P").await;
        let map = wait_for(&frames, |m| m.contains_key("FCHILD")).await;
        assert_eq!(map["FCHILD"].name(), None, "attach cannot know the name");

        navigate(&mock, "FCHILD", "https://host.test/side", Some("sidebar")).await;

        let map = wait_for(&frames, |m| {
            m.get("FCHILD").is_some_and(|f| f.name().is_some())
        })
        .await;
        let frame = &map["FCHILD"];
        assert_eq!(frame.name(), Some("sidebar"));
        assert_eq!(frame.url().await, "https://host.test/side");
        assert_eq!(
            frame.parent_id(),
            Some("P"),
            "parent must survive the backfill"
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// The backfill only ever adds a name. A later navigation that omits
    /// the field (Chrome does that for the unnamed case) must not erase the
    /// name a previous event established.
    #[tokio::test]
    async fn navigated_without_a_name_keeps_the_known_one() {
        let (mock, conn, frames, cancel) = start().await;

        attach(&mock, "FCHILD", "P").await;
        wait_for(&frames, |m| m.contains_key("FCHILD")).await;
        for (url, name) in [
            ("https://host.test/one", Some("sidebar")),
            ("https://host.test/two", None),
        ] {
            navigate(&mock, "FCHILD", url, name).await;
        }

        // The url is the marker for "the SECOND navigation landed".
        let map = wait_for(&frames, |m| {
            m.get("FCHILD").is_some_and(|f| {
                f.inner
                    .url
                    .try_read()
                    .is_ok_and(|u| *u == "https://host.test/two")
            })
        })
        .await;
        assert_eq!(map["FCHILD"].name(), Some("sidebar"));

        cancel.cancel();
        conn.shutdown();
    }

    /// Chrome emits `frameNavigated` for the main frame before any
    /// subscriber sees a `frameAttached` for it, so a navigation with no
    /// registry row is an implicit attach and has to insert one — off the
    /// event's own `parentId`, since there was no attach to take it from.
    ///
    /// The insert is guarded against a row that appeared while the arm was
    /// upgrading from the read guard to the write guard, and that guard is
    /// what this pins: the ordinary uncontended case must still insert.
    #[tokio::test]
    async fn navigated_for_an_unknown_frame_inserts_it() {
        let (mock, conn, frames, cancel) = start().await;

        navigate(&mock, "MAIN", "https://host.test/", Some("top")).await;

        let map = wait_for(&frames, |m| m.contains_key("MAIN")).await;
        let frame = &map["MAIN"];
        assert_eq!(frame.url().await, "https://host.test/");
        assert_eq!(frame.name(), Some("top"));
        assert_eq!(
            frame.parent_id(),
            Some("P"),
            "the implicit attach takes its parent from the navigation event",
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// A `Frame` is a cheap `Arc` handle and `Tab::frames()` hands out
    /// clones, so holding one across navigations is the intended usage.
    ///
    /// The name backfill replaces the registry row with a fresh `Frame`, so
    /// a handle taken in the window between `frameAttached` and the first
    /// named `frameNavigated` points at an inner nothing would ever touch
    /// again. Writing the URL through that inner *before* the swap repairs
    /// exactly one navigation: the one carrying the rename, asserted here.
    ///
    /// It does **not** make the handle track the frame. Later navigations
    /// resolve the new row and leave this one frozen — see
    /// [`a_held_handle_stops_tracking_after_the_rename_swap`], which is the
    /// same scenario one navigation further along and is `#[ignore]`d
    /// because it cannot pass without the API break below.
    ///
    /// Only `url()` is repaired here. `Frame::name()` on the held handle
    /// still reads `None` forever, because `FrameInner::name` is immutable
    /// and making it interior-mutable turns `Frame::name()` into an `async
    /// fn` — a deliberate public break that is a maintainer's call, not
    /// this fix's.
    #[tokio::test]
    async fn a_handle_taken_before_the_name_backfill_still_tracks_the_url() {
        let (mock, conn, frames, cancel) = start().await;

        attach(&mock, "FCHILD", "P").await;
        let map = wait_for(&frames, |m| m.contains_key("FCHILD")).await;
        let held = map["FCHILD"].clone();
        assert_eq!(held.url().await, "", "nothing has navigated yet");

        navigate(
            &mock,
            "FCHILD",
            "https://host.test/checkout",
            Some("sidebar"),
        )
        .await;
        let map = wait_for(&frames, |m| {
            m.get("FCHILD").is_some_and(|f| f.name().is_some())
        })
        .await;
        assert_eq!(map["FCHILD"].url().await, "https://host.test/checkout");

        assert_eq!(
            held.url().await,
            "https://host.test/checkout",
            "a handle taken before the rename must not be orphaned on url()",
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// The write-through's boundary, and the reason it is a stopgap rather
    /// than a fix: it repairs the navigation that carries the rename and
    /// nothing after it.
    ///
    /// Two navigations, one handle. The first renames the frame and swaps
    /// the registry row; the second resolves that new row and writes its
    /// inner. The held handle points at the old one, so it reads `/cart`
    /// while the registry reads `/checkout` — and
    /// `loop { if f.url().await.contains("/checkout") { break } }`, the
    /// obvious way to wait on an iframe, never terminates on that handle.
    ///
    /// **Ignored, not deleted.** Passing it requires interior-mutable
    /// `FrameInner::name` so the backfill mutates the shared inner instead
    /// of replacing the row, which turns `Frame::name()` into an `async fn`
    /// and breaks a public signature. That call is reserved for a
    /// maintainer (founder's review round 1, §5 item 2 — "`Frame::name()`
    /// and `Frame::id()` interior mutability"). Un-`#[ignore]` this the day
    /// the break is taken; it is the acceptance test for it.
    #[tokio::test]
    #[ignore = "needs interior-mutable FrameInner::name; reserved public API break"]
    async fn a_held_handle_stops_tracking_after_the_rename_swap() {
        let (mock, conn, frames, cancel) = start().await;

        attach(&mock, "FCHILD", "P").await;
        let held = wait_for(&frames, |m| m.contains_key("FCHILD")).await["FCHILD"].clone();

        // Navigation 1 carries the name, so it swaps the registry row.
        navigate(&mock, "FCHILD", "https://host.test/cart", Some("sidebar")).await;
        wait_for(&frames, |m| {
            m.get("FCHILD").is_some_and(|f| f.name().is_some())
        })
        .await;

        // Navigation 2 lands on the row the swap installed.
        navigate(&mock, "FCHILD", "https://host.test/checkout", None).await;
        let map = wait_for(&frames, |m| {
            m.get("FCHILD").is_some_and(|f| {
                f.inner
                    .url
                    .try_read()
                    .is_ok_and(|u| u.ends_with("/checkout"))
            })
        })
        .await;
        assert_eq!(map["FCHILD"].url().await, "https://host.test/checkout");

        assert_eq!(
            held.url().await,
            "https://host.test/checkout",
            "a handle must keep tracking the frame across every navigation, \
             not just the one that renamed it",
        );

        cancel.cancel();
        conn.shutdown();
    }
}
