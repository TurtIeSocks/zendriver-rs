//! Cloudflare-bypass errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CloudflareError {
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
    fn display_js_error_passthrough() {
        let e = CloudflareError::JsError("bad payload".into());
        assert_eq!(e.to_string(), "JS error: bad payload");
    }
}
