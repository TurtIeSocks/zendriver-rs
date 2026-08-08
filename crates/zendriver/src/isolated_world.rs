//! Per-frame isolated-world execution-context cache.
//!
//! Shared between [`crate::tab::Tab`] (main-frame eval) and
//! [`crate::frame::Frame`] (per-frame eval). Holds the discovered main frame
//! id (cached after the first `Page.getFrameTree` round-trip) and the
//! `executionContextId` returned by `Page.createIsolatedWorld` (cached
//! after first use; invalidated when Chrome reports
//! "Cannot find context with specified id", typically after a navigation
//! destroys the previous context).
//!
//! Pre-P4 this lived as a private type inside `tab.rs`. P4 promotes it to
//! a shared module so [`crate::frame::Frame`] can carry its own per-frame
//! cache without duplicating the struct definition. The visibility stays
//! `pub(crate)` — neither the cache nor its fields are part of the public
//! API.

/// Cache of the discovered main-frame id + the executionContextId for that
/// frame's `zendriver-eval` isolated world.
///
/// Both fields start `None`; the first call to
/// [`crate::tab::Tab::ensure_isolated_world`] populates them in one
/// round-trip pair (`Page.getFrameTree` then `Page.createIsolatedWorld`).
/// Subsequent calls short-circuit on the cached `context_id`.
///
/// When [`crate::tab::Tab::evaluate`] or [`crate::frame::Frame::evaluate`]
/// catches Chrome's `-32000 Cannot find context with specified id` error
/// (see [`is_stale_context`]), it sets `context_id = None` (leaving
/// `main_frame_id` intact since the frame is still around) and retries —
/// the next `ensure_isolated_world` call re-runs `Page.createIsolatedWorld`
/// only, skipping the `Page.getFrameTree` round-trip.
#[derive(Default, Debug)]
pub(crate) struct IsolatedWorldCache {
    pub(crate) main_frame_id: Option<String>,
    pub(crate) context_id: Option<i64>,
}

/// Whether `e` is Chrome telling us the `executionContextId` we cached no
/// longer exists — the signal to drop `context_id` and retry once.
///
/// Chrome reports this as `-32000 Cannot find context with specified id`,
/// but [`crate::error::ZendriverError`]'s `From<CallError>` maps `-32000`
/// onto [`crate::error::ZendriverError::Navigation`], so the match is on
/// that variant and not on `Cdp`.
///
/// Single source of truth on purpose: [`crate::tab::Tab::evaluate`] and
/// [`crate::frame::Frame::evaluate`] both need it, and a magic string
/// duplicated across two files is exactly the thing that drifts. (It did:
/// `Frame::evaluate` had no retry at all until this was hoisted, so a
/// navigated iframe's handle failed permanently while the tab-level call
/// recovered transparently.)
pub(crate) fn is_stale_context(e: &crate::error::ZendriverError) -> bool {
    matches!(e, crate::error::ZendriverError::Navigation(m) if m.contains("Cannot find context"))
}
