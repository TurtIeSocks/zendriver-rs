//! Interception-layer errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InterceptionError {
    #[error("call failed: {0}")]
    Call(#[from] zendriver_transport::CallError),

    #[error("invalid url pattern: {0}")]
    InvalidPattern(String),

    #[error("interception already started")]
    AlreadyStarted,

    #[error("interception not started")]
    NotStarted,

    #[error("subscription channel closed")]
    SubscriptionClosed,

    #[error("invalid response from CDP: {0}")]
    InvalidResponse(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_invalid_pattern() {
        let e = InterceptionError::InvalidPattern("**bad".into());
        assert_eq!(e.to_string(), "invalid url pattern: **bad");
    }
}
