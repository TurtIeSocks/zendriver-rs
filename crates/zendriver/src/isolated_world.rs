//! Per-frame isolated-world execution-context cache.
//!
//! Shared between [`crate::tab::Tab`] (main-frame eval) and
//! [`crate::frame::Frame`] (per-frame eval). Holds the
//! `executionContextId` returned by `Page.createIsolatedWorld` (cached
//! after first use; invalidated when Chrome reports
//! "Cannot find context with specified id", typically after a navigation
//! destroys the previous context).
//!
//! Pre-P4 this lived as a private type inside `tab.rs`. P4 promotes it to
//! a shared module so [`crate::frame::Frame`] can carry its own per-frame
//! cache without duplicating the struct definition. The visibility stays
//! `pub(crate)` — neither the cache nor its contents are part of the public
//! API.

/// Cache of the `executionContextId` for one frame's `zendriver-eval`
/// isolated world.
///
/// Starts `None`. [`crate::tab::Tab::ensure_isolated_world`] populates it
/// with a `Page.getFrameTree` + `Page.createIsolatedWorld` pair;
/// [`crate::frame::Frame::ensure_isolated_world`] already knows its own
/// `frameId` and skips the first of those. Later calls short-circuit on the
/// cached value.
///
/// When [`crate::tab::Tab::evaluate`] or [`crate::frame::Frame::evaluate`]
/// catches Chrome's `-32000 Cannot find context with specified id` error
/// (see [`is_stale_context`]), it sets `context_id = None` and retries, so
/// the next `ensure_isolated_world` call runs that discovery again from
/// scratch. The frame id is deliberately not cached alongside it: reusing
/// one across the navigation that invalidated the context is how a stale
/// `frameId` reaches `Page.createIsolatedWorld`, and the round-trip it
/// would save is one per navigation.
#[derive(Default, Debug)]
pub(crate) struct IsolatedWorldCache {
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
