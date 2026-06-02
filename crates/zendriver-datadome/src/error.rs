//! DataDome-bypass errors.

use zendriver_interception::InterceptionError;
use zendriver_transport::CallError;

/// Error returned by [`DataDomeBypass`] operations. Faults only — flow
/// terminals (cleared / blocked / timed-out / already-clear) are
/// [`ClearanceOutcome`] variants, not errors.
///
/// [`DataDomeBypass`]: crate::bypass::DataDomeBypass
/// [`ClearanceOutcome`]: crate::bypass::ClearanceOutcome
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DataDomeError {
    /// CAPTCHA surface detected, but no `on_captcha` solver was registered.
    #[error("CAPTCHA surface detected but no solver registered")]
    CaptchaRequired,

    /// User-supplied CAPTCHA solver returned an error.
    #[error("CAPTCHA solver failed: {0}")]
    CaptchaSolver(Box<dyn std::error::Error + Send + Sync>),

    /// Fetch-domain interception hook failed at startup.
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
    fn display_captcha_required() {
        assert_eq!(
            DataDomeError::CaptchaRequired.to_string(),
            "CAPTCHA surface detected but no solver registered"
        );
    }

    #[test]
    fn display_js_error_passthrough() {
        assert_eq!(
            DataDomeError::JsError("bad payload".into()).to_string(),
            "JS error: bad payload"
        );
    }
}
