//! Out-of-process iframe (OOPIF) Frame attach.
//!
//! Extends the [`crate::browser::TabRegistrar`] observer to handle
//! `Target.attachedToTarget` events where `target_info.kind == "iframe"`,
//! constructing a [`crate::frame::Frame`] with the new child session and
//! registering it in the parent tab's frames map.
//!
//! ## Parent-tab discovery
//!
//! When Chrome attaches an OOPIF target the payload identifies the iframe
//! by `target_id` (always present) and — on Chromium 90+ — by
//! `opener_frame_id` (the host iframe's CDP `frameId` inside the parent
//! document). To find the owning [`crate::tab::Tab`] we walk the browser-
//! wide tabs registry and look for the first tab whose frames map already
//! contains an entry under one of these ids:
//!
//! 1. `opener_frame_id` if Chrome supplied one — the canonical match.
//! 2. Otherwise `target_id` — for OOPIFs Chrome typically uses the
//!    `targetId` as the `frameId` of the iframe inside the parent
//!    document (so the parent's `Page.frameAttached` handler has already
//!    registered a same-id entry by the time the OOPIF target attaches).
//!
//! If neither id matches anything in the registry the OOPIF attaches
//! before any parent observed `Page.frameAttached` for the host iframe
//! — a rare race. We log a warning and skip registration; the OOPIF's
//! session is still alive (the actor will release the debugger after the
//! observer returns) but no `Frame` handle is exposed for it. The next
//! `Page.frameAttached` from the parent tab will create a same-origin
//! placeholder entry that callers can use for queries on the parent side,
//! and the OOPIF target can be discovered manually via `Browser::cdp()`
//! if needed.

use std::sync::Arc;

use tracing::{trace, warn};
use zendriver_transport::{SessionHandle, TargetInfo};

use crate::browser::BrowserInner;
use crate::frame::Frame;

/// Construct a [`Frame`] for the attached OOPIF target and register it in
/// the parent tab's frames map. Returns `Some(())` on success, `None` if no
/// parent tab could be located.
///
/// Called by [`crate::browser::TabRegistrar::on_target_attached`] for events
/// with `target_info.kind == "iframe"`. The returned [`Frame`] shares the
/// passed-in [`SessionHandle`] (the OOPIF's distinct child session) — so
/// any subsequent `Runtime.evaluate` / `Page.*` dispatch lands in the
/// out-of-process renderer, not the parent tab's renderer.
pub(crate) async fn register_oopif_frame(
    browser: &Arc<BrowserInner>,
    target_info: &TargetInfo,
    session: SessionHandle,
) -> Option<()> {
    // Candidate frame_ids to look up in each tab's frames map. Prefer the
    // explicit opener_frame_id (Chrome 90+); fall back to the target_id
    // (Chrome uses targetId == frameId for OOPIF hosts in many cases).
    let mut candidates: Vec<&str> = Vec::with_capacity(2);
    if let Some(opener) = target_info.opener_frame_id.as_deref() {
        candidates.push(opener);
    }
    candidates.push(&target_info.target_id);

    let tabs = browser.tabs.read().await;
    for tab in tabs.values() {
        let frames = tab.inner.frames.read().await;
        for cand in &candidates {
            if frames.contains_key(*cand) {
                drop(frames);

                // The OOPIF's own frame_id is ALWAYS its target_id — the
                // unique CDP identifier for this child renderer's frame.
                // The matched `cand` only tells us how we found the parent
                // (either via Chrome's opener_frame_id hint or via the
                // common targetId == frameId convention); it must NOT be
                // reused as the new Frame's id, or two OOPIFs sharing a
                // parent_frame_id would collide and overwrite each other
                // in the frames map. The matched candidate becomes the
                // new frame's `parent_frame_id` linkage.
                //
                // url/name unknown at attach time (a subsequent
                // Page.frameNavigated on the child session will populate
                // them via the lifecycle subscriber spawned by the parent
                // Tab... actually the parent's subscriber listens on the
                // parent's session, not the OOPIF session, so URL stays
                // empty until a future per-OOPIF lifecycle is wired).
                let frame_id = target_info.target_id.clone();
                let parent_frame_id = (*cand).to_string();
                let oopif = Frame::new(
                    frame_id.clone(),
                    Some(parent_frame_id),
                    String::new(),
                    None,
                    session,
                    Arc::downgrade(&tab.inner),
                );
                tab.inner.frames.write().await.insert(frame_id, oopif);
                trace!(
                    target_id = %target_info.target_id,
                    "OOPIF frame registered under parent tab",
                );
                return Some(());
            }
        }
    }
    warn!(
        target_id = %target_info.target_id,
        opener_frame_id = ?target_info.opener_frame_id,
        "OOPIF attached with no matching parent frame; skipping registration",
    );
    None
}

/// Walk every tab in the browser registry and remove any [`Frame`] whose
/// underlying session matches `session_id`. Returns `true` if any frame
/// was removed.
///
/// Called by [`crate::browser::TabRegistrar::on_target_detached`] as the
/// counterpart to [`register_oopif_frame`]: when an OOPIF's child session
/// goes away, drop its placeholder from the hosting tab's frames map.
pub(crate) async fn deregister_oopif_frame(browser: &Arc<BrowserInner>, session_id: &str) -> bool {
    let tabs = browser.tabs.read().await;
    for tab in tabs.values() {
        let to_remove: Vec<String> = {
            let frames = tab.inner.frames.read().await;
            frames
                .iter()
                .filter(|(_, frame)| frame.session().session_id() == session_id)
                .map(|(frame_id, _)| frame_id.clone())
                .collect()
        };
        if !to_remove.is_empty() {
            let mut frames = tab.inner.frames.write().await;
            for frame_id in &to_remove {
                frames.remove(frame_id);
            }
            return true;
        }
    }
    false
}
