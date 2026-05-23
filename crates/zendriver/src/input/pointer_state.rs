//! Pointer state bitflags + helpers.

use bitflags::bitflags;

bitflags! {
    /// Mouse buttons currently held down. Tracked by InputController so
    /// drag/release sequences work.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct MouseButtonSet: u8 {
        const LEFT    = 0b00001;
        const RIGHT   = 0b00010;
        const MIDDLE  = 0b00100;
        const BACK    = 0b01000;
        const FORWARD = 0b10000;
    }
}
