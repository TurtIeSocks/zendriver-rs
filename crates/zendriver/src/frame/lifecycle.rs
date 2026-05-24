//! Frame lifecycle event subscriber.
//!
//! Stub for T15: subscribes to `Page.frameAttached`, `Page.frameDetached`,
//! and `Page.frameNavigated` on a tab's session, maintains the tab's frames
//! registry, and updates [`crate::frame::FrameInner::url`] on navigation.
