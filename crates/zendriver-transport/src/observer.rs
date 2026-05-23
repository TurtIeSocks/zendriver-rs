//! TargetObserver trait — fires on each new attached target while the
//! target is paused at the debugger.

use crate::connection::Connection;
use crate::error::CallError;

#[async_trait::async_trait]
pub trait TargetObserver: Send + Sync {
    /// Called once per new target, after attach and before debugger release.
    /// Observer MUST complete and return before the target resumes execution.
    /// Observers run serially in registration order; returning Err leaves the
    /// target paused (the actor logs + force-detaches the session).
    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError>;

    /// Called when a target detaches. Default: no-op.
    async fn on_target_detached(&self, _session_id: &str) {}

    fn name(&self) -> &'static str;
}

pub struct PausedSession<'a> {
    pub session_id: &'a str,
    pub target_info: &'a TargetInfo,
    pub(crate) conn: &'a Connection,
}

impl<'a> PausedSession<'a> {
    pub async fn call(
        &self,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, CallError> {
        self.conn
            .call_raw(method, params, Some(self.session_id.to_string()))
            .await
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ObserverError {
    #[error("call failed: {0}")]
    Call(#[from] CallError),

    #[error("observer timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("observer panicked: {0}")]
    Panicked(String),

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TargetInfo {
    #[serde(rename = "targetId")]
    pub target_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub attached: bool,
    #[serde(default, rename = "browserContextId")]
    pub browser_context_id: Option<String>,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_observer_error_timeout_includes_duration() {
        let e = ObserverError::Timeout(std::time::Duration::from_secs(5));
        assert_eq!(e.to_string(), "observer timed out after 5s");
    }

    #[test]
    fn display_observer_error_panicked_includes_message() {
        let e = ObserverError::Panicked("oh no".into());
        assert_eq!(e.to_string(), "observer panicked: oh no");
    }

    #[test]
    fn target_info_deserializes_chrome_payload() {
        let json = r#"{"targetId":"T1","type":"page","url":"about:blank","attached":true}"#;
        let info: TargetInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.target_id, "T1");
        assert_eq!(info.kind, "page");
        assert_eq!(info.url, "about:blank");
        assert!(info.attached);
    }
}
