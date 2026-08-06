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
//!      child session, NOT this stream) and insert it under `frameId`.
//!    - `Page.frameNavigated` — update the existing entry's URL in
//!      place (and backfill the frame `name` the attach event could not
//!      carry); insert a fresh [`crate::Frame`] if no entry exists.
//!    - `Page.frameDetached` — remove the entry from the registry.
//!
//! The task runs until its [`tokio_util::sync::CancellationToken`] fires —
//! typically when the owning Tab is dropped.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use zendriver_transport::SessionHandle;

use crate::frame::Frame;
use crate::tab::TabInner;

/// Wait budget for the `Page.getFrameTree` probe that confirms a
/// same-parent sibling really is a dead provisional frame. Bounded so a
/// wedged probe can never stall the event loop that keeps the registry
/// current; on expiry the sweep fails open (nothing is evicted).
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
    let enable_session = session.clone();
    tokio::spawn(async move {
        if let Err(e) = enable_session
            .call("Page.enable", serde_json::json!({}))
            .await
        {
            warn!(error = %e, "frame::lifecycle: Page.enable failed; frame events may be inactive");
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
                // Chrome occasionally fires `frameAttached` twice for a
                // single iframe — once with a provisional `frameId` and
                // again with the committed one — without an intervening
                // `frameDetached`. Observed on `srcdoc` iframes under
                // `--headless=new`. Evict those dead entries so the
                // registry doesn't accumulate frames that
                // `Page.createIsolatedWorld` would reject with
                // "No frame for given id found".
                let dead = dead_provisional_siblings(&session, &frames, &ev).await;
                let mut map = frames.write().await;
                for id in dead {
                    map.remove(&id);
                }
                map.insert(ev.frame_id, frame);
            }
            Some(ev) = navigated.next() => {
                let frame_id = ev.frame.id;
                let new_url = ev.frame.url;
                // `Page.frameAttached` carries no name, so an entry created
                // from it always starts nameless. Treat a non-empty name on
                // `frameNavigated` as the authoritative one; never clear a
                // known name from an event that simply omits the field.
                let new_name = ev.frame.name.filter(|n| !n.is_empty());
                // Update in place if known; otherwise treat the navigation
                // as the implicit attach (Chrome may emit `frameNavigated`
                // for the main frame before any subscriber sees an explicit
                // `frameAttached`).
                let mut map = frames.write().await;
                if let Some(existing) = map.get(&frame_id) {
                    let name_changed = new_name.is_some()
                        && new_name.as_deref() != existing.inner.name.as_deref();
                    if !name_changed {
                        *existing.inner.url.write().await = new_url;
                        continue;
                    }
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
                    continue;
                }
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

/// Registry entries that the freshly-attached `ev` supersedes: same parent,
/// same session, still URL-less — **and confirmed gone from Chrome's live
/// frame tree**.
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
/// Fails open in every ambiguous case (no parent, no candidates, probe
/// error or timeout): keeping a possibly-dead entry only costs a stale row
/// in [`crate::Tab::frames`], which
/// [`crate::Frame::ensure_isolated_world`] already recovers from, whereas
/// evicting a live frame loses a handle the caller may hold.
async fn dead_provisional_siblings(
    session: &SessionHandle,
    frames: &Arc<RwLock<HashMap<String, Frame>>>,
    ev: &FrameAttachedEvent,
) -> Vec<String> {
    let Some(parent) = ev.parent_frame_id.as_deref() else {
        return Vec::new();
    };
    let candidates: Vec<String> = {
        let map = frames.read().await;
        map.iter()
            .filter_map(|(id, f)| {
                if id == &ev.frame_id {
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
                url_slot.is_empty().then(|| id.clone())
            })
            .collect()
    };
    if candidates.is_empty() {
        return Vec::new();
    }
    let Some(live) = live_frame_ids(session).await else {
        return Vec::new();
    };
    candidates
        .into_iter()
        .filter(|id| !live.contains(id))
        .collect()
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
    fn collect(node: &serde_json::Value, out: &mut HashSet<String>) {
        if let Some(id) = node["frame"]["id"].as_str() {
            out.insert(id.to_string());
        }
        if let Some(children) = node["childFrames"].as_array() {
            for c in children {
                collect(c, out);
            }
        }
    }
    let mut ids = HashSet::new();
    collect(&tree["frameTree"], &mut ids);
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
        for _ in 0..100 {
            {
                let map = frames.read().await;
                if pred(&map) {
                    return map.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("registry never reached the expected state within 1s");
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

        let id = tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.getFrameTree"))
            .await
            .expect("attach with an url-less sibling did not probe Page.getFrameTree");
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

        let map = wait_for(&frames, |m| m.contains_key("F2")).await;
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

        let id = tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.getFrameTree"))
            .await
            .expect("attach with an url-less sibling did not probe Page.getFrameTree");
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

        let id = tokio::time::timeout(Duration::from_secs(2), mock.expect_cmd("Page.getFrameTree"))
            .await
            .expect("attach with an url-less sibling did not probe Page.getFrameTree");
        mock.reply_err(id, -32000, "Page domain not enabled").await;

        let map = wait_for(&frames, |m| m.contains_key("F2")).await;
        assert!(
            map.contains_key("F1"),
            "an unanswerable probe must not authorize an eviction",
        );

        cancel.cancel();
        conn.shutdown();
    }

    /// A single child needs no probe at all — no `Page.getFrameTree` is
    /// dispatched, so the common case keeps its zero-round-trip cost.
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

        mock.emit_event_for_session(
            "Page.frameNavigated",
            json!({
                "frame": {
                    "id": "FCHILD",
                    "parentId": "P",
                    "url": "https://host.test/side",
                    "name": "sidebar",
                }
            }),
            SESSION,
        )
        .await;

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
            ("https://host.test/one", json!("sidebar")),
            ("https://host.test/two", json!(null)),
        ] {
            mock.emit_event_for_session(
                "Page.frameNavigated",
                json!({
                    "frame": { "id": "FCHILD", "parentId": "P", "url": url, "name": name }
                }),
                SESSION,
            )
            .await;
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
}
