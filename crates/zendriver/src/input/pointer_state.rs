//! Pointer state bitflags — internal mouse-button tracking.

use bitflags::bitflags;

bitflags! {
    /// Mouse buttons currently held down.
    ///
    /// Tracked by the per-tab [`crate::input::InputController`] so
    /// drag/release sequences work correctly across multiple element
    /// actions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct MouseButtonSet: u8 {
        /// Primary button held.
        const LEFT    = 0b00001;
        /// Secondary button held.
        const RIGHT   = 0b00010;
        /// Middle button held.
        const MIDDLE  = 0b00100;
        /// Back thumb button held.
        const BACK    = 0b01000;
        /// Forward thumb button held.
        const FORWARD = 0b10000;
    }
}
