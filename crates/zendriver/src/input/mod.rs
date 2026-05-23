//! Realistic + raw input simulation: mouse paths, keyboard dispatch,
//! per-browser pointer/modifier state.

pub mod bezier;
pub mod keyboard;
pub mod mouse;
pub mod pointer_state;

pub use keyboard::{Key, KeyModifiers, SpecialKey};
pub use mouse::MouseButton;
