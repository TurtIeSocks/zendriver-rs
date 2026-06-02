//! Stealth-layer errors.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StealthError {
    #[error("failed to apply patch '{patch}'")]
    PatchFailed {
        patch: &'static str,
        #[source]
        source: zendriver_transport::CallError,
    },

    #[error("could not detect chrome version: {0}")]
    ChromeVersionDetect(String),

    #[error("could not read system info: {0}")]
    SystemInfo(String),

    #[error("invalid fingerprint override: {0}")]
    InvalidOverride(String),

    /// A live-browser probe (e.g. [`crate::Persona::from_browser`]) failed.
    ///
    /// Carries the upstream error's message as a `String` rather than the
    /// concrete type: the probe runs through the [`crate::JsProbe`] seam so
    /// that `zendriver-stealth` need not depend on the `zendriver` core crate
    /// (which would be a dependency cycle). The caller's `JsProbe` impl maps
    /// its own error into this variant.
    #[error("live-browser probe failed: {0}")]
    Probe(String),
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_chrome_version_detect_includes_message() {
        let e = StealthError::ChromeVersionDetect("exit 1".into());
        assert_eq!(e.to_string(), "could not detect chrome version: exit 1");
    }

    #[test]
    fn display_system_info_includes_message() {
        let e = StealthError::SystemInfo("permission denied".into());
        assert_eq!(
            e.to_string(),
            "could not read system info: permission denied"
        );
    }

    #[test]
    fn display_invalid_override_includes_message() {
        let e = StealthError::InvalidOverride("memory_gb must be > 0".into());
        assert_eq!(
            e.to_string(),
            "invalid fingerprint override: memory_gb must be > 0"
        );
    }
}
