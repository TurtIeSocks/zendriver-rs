//! Realistic + raw input simulation: mouse paths, keyboard dispatch,
//! per-browser pointer/modifier state.

use std::sync::Arc;

use rand::SeedableRng;
use tokio::sync::Mutex;
use zendriver_stealth::InputProfile;

use crate::input::pointer_state::MouseButtonSet;

pub mod bezier;
pub mod keyboard;
pub mod mouse;
pub mod pointer_state;

pub use keyboard::{Key, KeyModifiers, SpecialKey};
pub use mouse::MouseButton;

/// Per-Browser input state holder. Wraps `InputState` + an `InputProfile`.
///
/// One `InputController` lives on each `Browser` and is shared via
/// `Arc<InputController>` so each `Tab`/`Element` action can coordinate
/// pointer position + modifier state across the same tab tree.
pub struct InputController {
    // Fields are exercised by tests and consumed by later P3 tasks
    // (mouse dispatch, keyboard dispatch, actionability waits).
    #[allow(dead_code)]
    pub(crate) state: Mutex<InputState>,
    #[allow(dead_code)]
    pub(crate) profile: InputProfile,
}

#[allow(dead_code)]
pub(crate) struct InputState {
    pub pointer_x: f64,
    pub pointer_y: f64,
    pub buttons_held: MouseButtonSet,
    pub modifiers_held: KeyModifiers,
    pub rng: rand::rngs::SmallRng,
}

impl InputController {
    /// Build an `InputController` from an `InputProfile`. The internal RNG
    /// is seeded from OS entropy for unpredictable typing/movement jitter.
    #[must_use]
    pub fn new(profile: InputProfile) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InputState {
                pointer_x: 0.0,
                pointer_y: 0.0,
                buttons_held: MouseButtonSet::empty(),
                modifiers_held: KeyModifiers::empty(),
                rng: rand::rngs::SmallRng::from_entropy(),
            }),
            profile,
        })
    }

    /// Test-only constructor with a seeded RNG for deterministic Bezier paths
    /// and typing patterns.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn new_with_seed(profile: InputProfile, seed: u64) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InputState {
                pointer_x: 0.0,
                pointer_y: 0.0,
                buttons_held: MouseButtonSet::empty(),
                modifiers_held: KeyModifiers::empty(),
                rng: rand::rngs::SmallRng::seed_from_u64(seed),
            }),
            profile,
        })
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_initializes_zeroed_pointer_and_empty_buttons() {
        let ic = InputController::new(InputProfile::native());
        let s = ic.state.lock().await;
        assert_eq!(s.pointer_x, 0.0);
        assert_eq!(s.pointer_y, 0.0);
        assert!(s.buttons_held.is_empty());
        assert!(s.modifiers_held.is_empty());
    }

    #[tokio::test]
    async fn new_with_seed_is_deterministic() {
        // Two controllers built with the same seed must produce the same
        // first RNG output. We compare raw u64 draws to avoid coupling this
        // test to Bezier's specific jitter pattern.
        let a = InputController::new_with_seed(InputProfile::native(), 42);
        let b = InputController::new_with_seed(InputProfile::native(), 42);
        let av = {
            let mut s = a.state.lock().await;
            rand::RngCore::next_u64(&mut s.rng)
        };
        let bv = {
            let mut s = b.state.lock().await;
            rand::RngCore::next_u64(&mut s.rng)
        };
        assert_eq!(av, bv);
    }

    #[test]
    fn profile_is_stored_verbatim() {
        let ic = InputController::new(InputProfile::spoofed());
        assert!(ic.profile.typo_rate > 0.0);
        let ic2 = InputController::new(InputProfile::native());
        assert_eq!(ic2.profile.typo_rate, 0.0);
    }
}
