//! Input realism tunables — typing typo rate, mouse jitter, etc.

#[derive(Debug, Clone)]
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
}
