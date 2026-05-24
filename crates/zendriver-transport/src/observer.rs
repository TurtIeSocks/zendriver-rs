//! [`TargetObserver`] trait — fires on each new attached target while the
//! target is paused at the debugger.

use crate::connection::Connection;
use crate::error::CallError;

/// Observer fired on every new [`Target.attachedToTarget`] event before the
/// debugger releases the target.
///
/// The actor walks every registered observer serially (registration order)
/// on each new target. A failing observer returns `Err` and the actor detaches
/// the session via `Target.detachFromTarget`; observers that exceed the
/// observer-timeout are skipped and the actor releases the debugger so Chrome
/// doesn't hang indefinitely.
///
/// `zendriver-stealth::StealthObserver` implements this trait to install
/// patches on every new page target before the page's first script runs.
///
/// [`Target.attachedToTarget`]: https://chromedevtools.github.io/devtools-protocol/tot/Target/#event-attachedToTarget
#[async_trait::async_trait]
pub trait TargetObserver: Send + Sync {
    /// Called once per new target, after attach and before debugger release.
    /// Observer MUST complete and return before the target resumes execution.
    /// Observers run serially in registration order; returning Err leaves the
    /// target paused (the actor logs + force-detaches the session).
    async fn on_target_attached(&self, session: PausedSession<'_>) -> Result<(), ObserverError>;

    /// Called when a target detaches. Default: no-op.
    async fn on_target_detached(&self, _session_id: &str) {}

    /// Stable identifier used in actor diagnostics (`error!` / `warn!` records).
    fn name(&self) -> &'static str;
}

/// Scope passed to [`TargetObserver::on_target_attached`] — a session that's
/// currently paused at the debugger, plus a back-reference to the connection
/// for CDP calls scoped to that session.
#[derive(Debug)]
pub struct PausedSession<'a> {
    /// CDP `sessionId` for the newly attached target.
    pub session_id: &'a str,
    /// Decoded `targetInfo` payload (target id, kind, url, ...).
    pub target_info: &'a TargetInfo,
    pub(crate) conn: &'a Connection,
}

impl<'a> PausedSession<'a> {
    /// Send a CDP command scoped to this paused session's `sessionId`.
    /// Convenience over reaching for [`PausedSession::connection`] manually.
    pub async fn call(
        &self,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, CallError> {
        self.conn
            .call_raw(method, params, Some(self.session_id.to_string()))
            .await
    }

    /// The underlying [`Connection`]. Observers that need to spawn
    /// additional [`crate::SessionHandle`]s (e.g. zendriver's
    /// `TabRegistrar`) clone this to bind a fresh handle for the newly
    /// attached `sessionId`.
    #[must_use]
    pub fn connection(&self) -> &'a Connection {
        self.conn
    }
}

/// Errors an observer may return to indicate it failed to set up its slice of
/// the new target.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ObserverError {
    /// A CDP call dispatched from inside the observer failed.
    #[error("call failed: {0}")]
    Call(#[from] CallError),

    /// The observer exceeded its per-target timeout. The actor surfaces this
    /// when constructing diagnostic output; observers don't construct it
    /// themselves.
    #[error("observer timed out after {0:?}")]
    Timeout(std::time::Duration),

    /// The observer panicked. Carries the downcast panic payload.
    #[error("observer panicked: {0}")]
    Panicked(String),

    /// Catch-all for observer-defined failures that don't fit the typed
    /// variants above.
    #[error("{0}")]
    Other(String),
}

/// Decoded `targetInfo` payload from `Target.attachedToTarget` / `targetCreated`.
///
/// Mirrors CDP's [`Target.TargetInfo`] but only deserializes the fields used
/// downstream by observers + zendriver core.
///
/// [`Target.TargetInfo`]: https://chromedevtools.github.io/devtools-protocol/tot/Target/#type-TargetInfo
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TargetInfo {
    /// CDP target id (stable across `attach` / `detach` cycles).
    #[serde(rename = "targetId")]
    pub target_id: String,
    /// Target kind (`"page"`, `"iframe"`, `"worker"`, ...). The stealth
    /// observer keys off this to skip workers + iframes.
    #[serde(rename = "type")]
    pub kind: String,
    /// Initial URL the target is at — typically `about:blank` at attach time.
    pub url: String,
    /// Document title, when present.
    #[serde(default)]
    pub title: Option<String>,
    /// Whether a debugger is currently attached.
    #[serde(default)]
    pub attached: bool,
    /// Browser-context id this target belongs to (incognito / profile split).
    #[serde(default, rename = "browserContextId")]
    pub browser_context_id: Option<String>,
    /// `frameId` of the iframe element that hosts this target, when present.
    /// Chrome populates this for `kind == "iframe"` OOPIF targets (Chromium
    /// 90+); used by [`crate::TargetObserver`] implementations to attach the
    /// OOPIF's child session to its hosting frame in the parent tab's frame
    /// tree. Not present for `kind == "page"` and may be absent on older
    /// Chromium versions even for iframe targets, in which case attach
    /// observers fall back to matching `target_id` against the frame tree.
    #[serde(default, rename = "openerFrameId")]
    pub opener_frame_id: Option<String>,
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
