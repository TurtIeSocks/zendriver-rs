//! Imperva-bypass errors.

use std::time::Duration;
use zendriver_interception::InterceptionError;
use zendriver_transport::CallError;

/// **Stub** — relocated to `detection.rs` in Task 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpervaSurface {
    /// Modern reese84-based bot management.
    Reese84,
    /// Legacy Incapsula (`___utmvc` / `incap_ses_*`).
    Legacy,
    /// Visual or invisible CAPTCHA challenge.
    Captcha(CaptchaKind),
    /// No Imperva surface detected.
    None,
}

/// **Stub** — relocated to `detection.rs` in Task 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaKind {
    HCaptcha,
    Recaptcha,
    ImpervaNative,
    Unknown,
}

/// Error returned by [`ImpervaBypass`] operations.
///
/// [`ImpervaBypass`]: crate::bypass::ImpervaBypass
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImpervaError {
    /// `wait_for_clearance` exceeded the configured timeout.
    /// `last_surface` is the most recent surface observed by the poll loop.
    #[error("clearance not achieved within {timeout:?}")]
    Timeout {
        timeout: Duration,
        last_surface: Option<ImpervaSurface>,
    },

    /// CAPTCHA detected, but no `on_captcha` solver was registered.
    #[error("CAPTCHA required but no solver registered: {kind:?}")]
    CaptchaRequired { kind: CaptchaKind },

    /// User-supplied CAPTCHA solver returned an error.
    #[error("CAPTCHA solver failed: {0}")]
    CaptchaSolver(Box<dyn std::error::Error + Send + Sync>),

    /// Fetch-domain interception hook failed at startup (only when
    /// [`ImpervaBypass::with_interception`] was set).
    ///
    /// [`ImpervaBypass::with_interception`]: crate::bypass::ImpervaBypass::with_interception
    #[error("interception hook error: {0}")]
    Interception(#[from] InterceptionError),

    /// CDP transport / call error.
    #[error("call failed: {0}")]
    Call(#[from] CallError),

    /// In-page evaluator raised or returned an unexpected payload shape.
    #[error("JS error: {0}")]
    JsError(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_timeout_includes_duration() {
        let e = ImpervaError::Timeout {
            timeout: Duration::from_secs(30),
            last_surface: Some(ImpervaSurface::Reese84),
        };
        assert_eq!(e.to_string(), "clearance not achieved within 30s");
    }

    #[test]
    fn display_captcha_required_includes_kind() {
        let e = ImpervaError::CaptchaRequired {
            kind: CaptchaKind::HCaptcha,
        };
        assert_eq!(
            e.to_string(),
            "CAPTCHA required but no solver registered: HCaptcha"
        );
    }

    #[test]
    fn display_js_error_passthrough() {
        let e = ImpervaError::JsError("bad payload".into());
        assert_eq!(e.to_string(), "JS error: bad payload");
    }
}
