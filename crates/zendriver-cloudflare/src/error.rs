//! Cloudflare-bypass errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CloudflareError {
    #[error("no Turnstile challenge detected")]
    NoChallenge,

    #[error("clearance timed out")]
    ClearanceTimeout,

    #[error("call failed: {0}")]
    Call(#[from] zendriver_transport::CallError),

    #[error("JS error: {0}")]
    JsError(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_no_challenge() {
        let e = CloudflareError::NoChallenge;
        assert_eq!(e.to_string(), "no Turnstile challenge detected");
    }

    #[test]
    fn display_clearance_timeout() {
        let e = CloudflareError::ClearanceTimeout;
        assert_eq!(e.to_string(), "clearance timed out");
    }
}
