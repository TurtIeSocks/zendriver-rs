//! Realistic + raw input simulation: mouse paths, keyboard dispatch,
//! per-tab pointer/modifier state.
//!
//! Most user code interacts with the input layer indirectly via
//! [`crate::Element`] action methods. Public types re-exported here:
//!
//! - [`Key`] — single-key dispatch target.
//! - [`SpecialKey`] — named non-character keys (Enter, F1, ...).
//! - [`KeyModifiers`] — composable modifier bitflags.
//! - [`KeySequence`] — mixed text / key / chord builder for `type_keys`.
//! - [`MouseButton`] — mouse button enum.
//!
//! Touch dispatch ([`touch::tap_at`]) is a separate, internal-only module —
//! see [`crate::Tab::tap`] / [`crate::Element::tap`] for the public surface.

use std::sync::Arc;

use rand::SeedableRng;
use tokio::sync::Mutex;
use zendriver_stealth::InputProfile;

use crate::input::pointer_state::MouseButtonSet;

pub mod bezier;
pub mod keyboard;
pub mod mouse;
pub mod pointer_state;
pub(crate) mod touch;

pub use keyboard::{Key, KeyModifiers, KeySequence, SpecialKey};
pub use mouse::MouseButton;

/// Per-Tab input state holder.
///
/// Wraps internal cursor + modifier state and a
/// [`zendriver_stealth::InputProfile`] that controls realism
/// (typing cadence, mouse jitter). One `InputController` lives on each
/// [`crate::Tab`]; element actions ([`crate::Element::click`],
/// [`crate::Element::type_text`], etc.) consult it.
///
/// Most user code does not construct this directly — it's built internally
/// when a [`crate::Tab`] is registered, and accessed via [`crate::Tab::input`].
#[derive(Debug)]
pub struct InputController {
    pub(crate) state: Mutex<InputState>,
    pub(crate) profile: InputProfile,
}

#[derive(Debug)]
pub(crate) struct InputState {
    pub pointer_x: f64,
    pub pointer_y: f64,
    /// Buttons currently held down. Written by every press/release and by
    /// the drag path, and read back as CDP's `buttons` bitmask on every
    /// dispatched mouse event — `MouseButtonSet`'s bit values match that
    /// mask exactly, so [`MouseButtonSet::bits`] is the wire value directly.
    ///
    /// # Why the write paths look the way they do
    ///
    /// This field lives on the per-`Tab` [`InputController`], which outlives
    /// any single gesture, so a `?` between a press and its matching release
    /// would strand the bit set for the rest of the tab's life: every later
    /// `mouseMoved` would report a button held with no preceding `mousedown`.
    /// That impossible state is a worse signal to leak to behavioral
    /// anti-bot scoring than omitting `buttons` altogether — it is exactly
    /// this stream such scoring reads.
    ///
    /// Both gesture paths ([`mouse::click_at`] and [`crate::Tab::mouse_drag`])
    /// therefore run as one fallible section followed by a single cleanup,
    /// rather than using a `Drop` guard. Clearing the bit needs the async
    /// `Mutex`; `Drop` cannot await, and a `try_lock` inside `Drop` would
    /// silently fail under contention — precisely when another task is
    /// mid-dispatch and would go on to read the stale bit. More machinery,
    /// weaker guarantee.
    ///
    /// Residual case neither shape covers: if the caller drops the gesture
    /// future mid-flight (cancellation, an outer `tokio::time::timeout`), no
    /// cleanup runs and the bit stays set. A `Drop` guard would not reliably
    /// close that either, for the `try_lock` reason above.
    pub buttons_held: MouseButtonSet,
    pub modifiers_held: KeyModifiers,
    pub rng: rand::rngs::SmallRng,
}

impl InputController {
    /// Build an [`InputController`] from a
    /// [`zendriver_stealth::InputProfile`].
    ///
    /// The internal RNG is seeded from OS entropy for unpredictable
    /// typing/movement jitter.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::input::InputController;
    /// use zendriver_stealth::InputProfile;
    /// let ic = InputController::new(InputProfile::native());
    /// # let _ = ic;
    /// ```
    #[must_use]
    pub fn new(profile: InputProfile) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InputState {
                pointer_x: 0.0,
                pointer_y: 0.0,
                buttons_held: MouseButtonSet::empty(),
                modifiers_held: KeyModifiers::empty(),
                rng: rand::rngs::SmallRng::from_os_rng(),
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
