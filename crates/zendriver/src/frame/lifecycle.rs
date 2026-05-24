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
//!      place; insert a fresh [`crate::Frame`] if no entry exists.
//!    - `Page.frameDetached` — remove the entry from the registry.
//!
//! The task runs until its [`tokio_util::sync::CancellationToken`] fires —
//! typically when the owning Tab is dropped.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use zendriver_transport::SessionHandle;

use crate::frame::Frame;
use crate::tab::TabInner;

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
                    ev.parent_frame_id,
                    String::new(),
                    None,
                    session.clone(),
                    tab_weak.clone(),
                );
                frames.write().await.insert(ev.frame_id, frame);
            }
            Some(ev) = navigated.next() => {
                let frame_id = ev.frame.id.clone();
                let new_url = ev.frame.url.clone();
                // Update in place if known; otherwise treat the navigation
                // as the implicit attach (Chrome may emit `frameNavigated`
                // for the main frame before any subscriber sees an explicit
                // `frameAttached`).
                {
                    let map = frames.read().await;
                    if let Some(existing) = map.get(&frame_id) {
                        let mut url_slot = existing.inner.url.write().await;
                        *url_slot = new_url;
                        continue;
                    }
                }
                let frame = Frame::new(
                    frame_id.clone(),
                    ev.frame.parent_id,
                    ev.frame.url,
                    ev.frame.name,
                    session.clone(),
                    tab_weak.clone(),
                );
                frames.write().await.insert(frame_id, frame);
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
