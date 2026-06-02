//! Interception-layer errors.

/// Errors surfaced by the interception API.
///
/// The [`Self::Call`] variant is boxed so the enum stays small
/// (`size_of::<usize>()` for the heavy arm). Without boxing, every
/// `Result<T, InterceptionError>` would carry the full
/// `zendriver_transport::CallError` payload — large enough to trip
/// clippy's `result_large_err` lint at every fallible call site.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InterceptionError {
    #[error("call failed: {0}")]
    Call(Box<zendriver_transport::CallError>),

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

    #[error("wrong interception stage: this action is only valid at the Response stage")]
    WrongStage,
}

impl From<zendriver_transport::CallError> for InterceptionError {
    fn from(e: zendriver_transport::CallError) -> Self {
        Self::Call(Box::new(e))
    }
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
