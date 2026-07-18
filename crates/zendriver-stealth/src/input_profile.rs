//! Input realism tunables — typing typo rate, mouse jitter, etc.

/// Per-profile tunables controlling how realistic synthesized keyboard and
/// mouse input should look.
///
/// Two presets ship by default — [`InputProfile::native`] is fast and
/// deterministic, [`InputProfile::spoofed`] mimics human-paced typing and
/// jittery mouse motion. Use the field setters on a struct literal if you
/// need a custom mix.
///
/// ```
/// use zendriver_stealth::InputProfile;
/// let fast = InputProfile::native();
/// let slow = InputProfile::spoofed();
/// assert_eq!(fast.typo_rate, 0.0);
/// assert!(slow.typo_rate > 0.0);
/// assert!(fast.mouse_speed_px_per_ms > slow.mouse_speed_px_per_ms);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct InputProfile {
    /// Probability per character of injecting a typo + backspace. 0.0–1.0.
    pub typo_rate: f32,
    /// Range of "thinking pause" duration injected between words (ms).
    pub thinking_pause_ms_range: (u32, u32),
    /// Per-character typing delay range (ms).
    pub per_char_delay_ms_range: (u32, u32),
    /// Mouse cursor speed in pixels per millisecond when moving.
    pub mouse_speed_px_per_ms: f64,
    /// Jitter amplitude (px) applied to Bezier control points.
    pub jitter_amplitude_px: f64,
    /// Probability the mouse overshoots its target before settling.
    pub overshoot_rate: f32,
}

impl InputProfile {
    /// Fast + deterministic. Used by StealthProfile::native and Off.
    #[must_use]
    pub fn native() -> Self {
        Self {
            typo_rate: 0.0,
            thinking_pause_ms_range: (0, 0),
            per_char_delay_ms_range: (0, 0),
            mouse_speed_px_per_ms: 10.0,
            jitter_amplitude_px: 0.0,
            overshoot_rate: 0.0,
        }
    }

    /// chaser-oxide-derived realistic defaults. Used by StealthProfile::spoofed.
    #[must_use]
    pub fn spoofed() -> Self {
        Self {
            typo_rate: 0.03,
            thinking_pause_ms_range: (200, 400),
            per_char_delay_ms_range: (50, 150),
            mouse_speed_px_per_ms: 1.5,
            jitter_amplitude_px: 2.0,
            overshoot_rate: 0.20,
        }
    }

    /// Non-mechanical, humanized-timing preset for input realism, selected
    /// **independently** of any [`StealthProfile`](crate::StealthProfile).
    ///
    /// Shares [`InputProfile::spoofed`]'s tunables (human-paced typing,
    /// jittery mouse motion) but is meant to be opted into explicitly — e.g.
    /// via `BrowserBuilder::input_profile(InputProfile::coherent())` — rather
    /// than being derived from the active stealth profile. By default (no
    /// explicit `.input_profile(..)` call), input timing already follows the
    /// active `StealthProfile` — spoofed stealth gets humanized timing,
    /// native/off stealth gets zero-overhead timing — exactly as it always
    /// has. `coherent()` is for callers who want to *decouple* the two: keep
    /// humanized timing while stealth is off (or otherwise pin non-mechanical
    /// timing independently of whatever the stealth setting is doing). It is
    /// never applied implicitly — only an explicit `.input_profile(..)` call
    /// selects it.
    ///
    /// ```
    /// use zendriver_stealth::InputProfile;
    /// let coherent = InputProfile::coherent();
    /// assert_eq!(coherent, InputProfile::spoofed());
    /// assert_ne!(coherent, InputProfile::native());
    /// ```
    #[must_use]
    pub fn coherent() -> Self {
        Self::spoofed()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn native_has_zero_realism_overhead() {
        let p = InputProfile::native();
        assert_eq!(p.typo_rate, 0.0);
        assert_eq!(p.thinking_pause_ms_range, (0, 0));
        assert_eq!(p.per_char_delay_ms_range, (0, 0));
        assert_eq!(p.jitter_amplitude_px, 0.0);
        assert_eq!(p.overshoot_rate, 0.0);
    }

    #[test]
    fn spoofed_has_nonzero_realism() {
        let p = InputProfile::spoofed();
        assert!(p.typo_rate > 0.0);
        assert!(p.per_char_delay_ms_range.0 > 0);
        assert!(p.jitter_amplitude_px > 0.0);
        assert!(p.overshoot_rate > 0.0);
    }

    #[test]
    fn native_mouse_speed_much_faster_than_spoofed() {
        assert!(
            InputProfile::native().mouse_speed_px_per_ms
                > InputProfile::spoofed().mouse_speed_px_per_ms
        );
    }

    #[test]
    fn coherent_is_non_mechanical_and_matches_spoofed_realism() {
        let coherent = InputProfile::coherent();
        assert_eq!(
            coherent,
            InputProfile::spoofed(),
            "coherent() shares spoofed()'s humanized tunables"
        );
        assert_ne!(
            coherent,
            InputProfile::native(),
            "coherent() must be non-mechanical (not equal to the zero-overhead native preset)"
        );
        assert!(coherent.typo_rate > 0.0);
        assert!(coherent.per_char_delay_ms_range.0 > 0);
    }

    #[test]
    fn input_profile_partial_eq_is_structural() {
        assert_eq!(InputProfile::native(), InputProfile::native());
        assert_eq!(InputProfile::spoofed(), InputProfile::spoofed());
        assert_ne!(InputProfile::native(), InputProfile::spoofed());
    }
}
