//! Out-of-process iframe (OOPIF) Frame attach.
//!
//! Stub for T16: extends the `TabRegistrar` observer to handle
//! `Target.attachedToTarget` events where `target_info.kind == "iframe"`,
//! constructing a [`crate::frame::Frame`] with the new child session and
//! registering it in the parent tab's frames map.
