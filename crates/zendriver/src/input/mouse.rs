//! Populated in Phase 3.
//!
//! Temporary `MouseButton` stub so `input::mod.rs` re-export compiles before
//! Task 6 lands the real type + dispatch helpers.

/// Stub — Task 6 replaces with full variant set + `cdp_str()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
}
