//! Error hierarchy for the `zendriver` crate.
//!
//! Every fallible API in zendriver returns [`Result<T>`], which is an alias
//! for `std::result::Result<T, ZendriverError>`. [`ZendriverError`] is a
//! non-exhaustive enum covering CDP transport failures, navigation /
//! element / cookie / storage operation errors, and (when the relevant
//! cargo feature is enabled) wrappers around the sub-crate error types.

use std::path::PathBuf;
use std::time::Duration;

use zendriver_transport::CallError;

/// Top-level error type returned by every fallible API in this crate.
///
/// `#[non_exhaustive]` — new variants may be added in minor releases.
/// Pattern-match defensively (use a `_` arm).
///
/// # Examples
///
/// ```
/// # use zendriver::ZendriverError;
/// # use std::time::Duration;
/// let e = ZendriverError::Timeout(Duration::from_secs(5));
/// assert_eq!(e.to_string(), "timed out after 5s");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ZendriverError {
    /// Chrome process / launch failure.
    #[error("browser process failed: {0}")]
    Browser(#[from] BrowserError),

    /// Lower-level transport (WebSocket) failure.
    #[error("transport: {0}")]
    Transport(Box<zendriver_transport::TransportError>),

    /// Chrome returned a CDP RPC error (a method call returned `error.code` /
    /// `error.message`).
    #[error("CDP RPC error [{code}] {message}")]
    Cdp {
        /// CDP RPC error code (typically negative; see
        /// [JSON-RPC spec](https://www.jsonrpc.org/specification#error_object)).
        code: i32,
        /// Human-readable error message from Chrome.
        message: String,
        /// Optional `data` field from the RPC error payload.
        data: Option<serde_json::Value>,
    },

    /// A query selector did not match within the timeout.
    #[error("element not found: {selector}")]
    ElementNotFound {
        /// Description of the selector that failed to match (e.g.
        /// `"css(button.primary)"`, `"text_exact(Submit)"`).
        selector: String,
    },

    /// Generic operation timeout.
    #[error("timed out after {0:?}")]
    Timeout(Duration),

    /// Page navigation failed (DNS, connection refused, page crashed, etc.).
    #[error("navigation failed: {0}")]
    Navigation(String),

    /// A JS expression raised an exception during evaluation.
    #[error("javascript exception: {0}")]
    JsException(String),

    /// An element handle is stale and the auto-refresh path failed.
    #[error("element is stale: refresh failed or origin not refreshable")]
    ElementStale,

    /// An element handle obtained from raw JS evaluation cannot be refreshed
    /// (no selector to replay).
    #[error("element not refreshable (was returned from a JS evaluation)")]
    NotRefreshable,

    /// An element did not become actionable (visible + enabled + stable +
    /// hit-tested) within the gate timeout.
    #[error("element not actionable within {0:?}: {1}")]
    NotActionable(std::time::Duration, String),

    /// Frame lookup by id / url / name failed.
    #[error("frame not found: {0}")]
    FrameNotFound(String),

    /// Tab lookup by target_id / session_id failed.
    #[error("tab not found: {0}")]
    TabNotFound(String),

    /// A cookie operation failed (CDP refusal, malformed payload, etc.).
    #[error("cookie operation failed: {0}")]
    Cookie(String),

    /// A DOM storage operation failed (origin mismatch, CDP refusal, etc.).
    #[error("storage operation failed: {0}")]
    Storage(String),

    /// Session history navigation failed (no back/forward entry).
    #[error("history navigation failed: {0}")]
    HistoryNavigation(String),

    /// JSON serialization / deserialization error at the CDP boundary.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    /// I/O error (file read/write, etc.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Stealth fingerprint resolution failed.
    #[error("stealth: {0}")]
    Stealth(Box<zendriver_stealth::StealthError>),

    /// Request-interception sub-crate error. Gated by feature `interception`.
    #[cfg(feature = "interception")]
    #[error("interception: {0}")]
    Interception(Box<zendriver_interception::InterceptionError>),

    /// Cloudflare bypass sub-crate error. Gated by feature `cloudflare`.
    #[cfg(feature = "cloudflare")]
    #[error("cloudflare: {0}")]
    Cloudflare(Box<zendriver_cloudflare::CloudflareError>),

    /// Imperva bypass sub-crate error. Gated by feature `imperva`.
    #[cfg(feature = "imperva")]
    #[error("imperva: {0}")]
    Imperva(Box<zendriver_imperva::ImpervaError>),

    /// Chrome-for-Testing fetcher error. Gated by feature `fetcher`.
    #[cfg(feature = "fetcher")]
    #[error("fetcher: {0}")]
    Fetcher(Box<zendriver_fetcher::FetcherError>),
}

impl From<zendriver_transport::TransportError> for ZendriverError {
    fn from(e: zendriver_transport::TransportError) -> Self {
        Self::Transport(Box::new(e))
    }
}

impl From<zendriver_stealth::StealthError> for ZendriverError {
    fn from(e: zendriver_stealth::StealthError) -> Self {
        Self::Stealth(Box::new(e))
    }
}

#[cfg(feature = "interception")]
impl From<zendriver_interception::InterceptionError> for ZendriverError {
    fn from(e: zendriver_interception::InterceptionError) -> Self {
        Self::Interception(Box::new(e))
    }
}

#[cfg(feature = "cloudflare")]
impl From<zendriver_cloudflare::CloudflareError> for ZendriverError {
    fn from(e: zendriver_cloudflare::CloudflareError) -> Self {
        Self::Cloudflare(Box::new(e))
    }
}

#[cfg(feature = "imperva")]
impl From<zendriver_imperva::ImpervaError> for ZendriverError {
    fn from(e: zendriver_imperva::ImpervaError) -> Self {
        Self::Imperva(Box::new(e))
    }
}

#[cfg(feature = "fetcher")]
impl From<zendriver_fetcher::FetcherError> for ZendriverError {
    fn from(e: zendriver_fetcher::FetcherError) -> Self {
        Self::Fetcher(Box::new(e))
    }
}

impl From<CallError> for ZendriverError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Transport(t) => ZendriverError::Transport(Box::new(t)),
            CallError::Rpc(code, message, data) => {
                // Special-case: Chrome returns -32000 "Cannot find context in
                // which to perform call" when the page navigated out from
                // under us. That's semantically a navigation failure, not a
                // raw protocol error.
                if code == -32000 && message.contains("Cannot find context") {
                    ZendriverError::Navigation(message)
                } else {
                    ZendriverError::Cdp {
                        code,
                        message,
                        data,
                    }
                }
            }
            // `CallError` is `#[non_exhaustive]`; if a new variant lands and
            // higher layers need to handle it specially, this fallback keeps
            // information by wrapping the Display in a transport-io error.
            other => ZendriverError::Io(std::io::Error::other(other.to_string())),
        }
    }
}

/// Convenience alias for `Result<T, ZendriverError>`.
///
/// All fallible APIs in this crate return this type.
pub type Result<T, E = ZendriverError> = std::result::Result<T, E>;

/// Errors specific to Chrome process discovery / spawn / WebSocket attach.
///
/// Surfaced inside [`ZendriverError::Browser`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserError {
    /// No Chrome / Chromium binary found on PATH or in conventional install
    /// locations. `searched` lists every path that was probed.
    #[error("chrome executable not found; searched: {searched:?}")]
    ExecutableNotFound {
        /// Every candidate path the discovery routine probed.
        searched: Vec<PathBuf>,
    },

    /// `Command::spawn` returned an OS-level failure.
    #[error("chrome failed to start: {0}")]
    SpawnFailed(#[source] std::io::Error),

    /// Chrome exited before printing its `DevTools listening on` line —
    /// typically a profile lock, missing GPU sandbox, or invalid flag.
    #[error("chrome exited before WS endpoint became available (status: {0:?})")]
    EarlyExit(std::process::ExitStatus),

    /// Timeout waiting for the `DevTools listening on` line.
    #[error("timed out waiting for chrome WS endpoint")]
    WsTimeout,

    /// Stderr contained an unparseable DevTools URL line.
    #[error("could not parse devtools endpoint from chrome stderr")]
    DevtoolsParse,

    /// `tempfile` cleanup of the `user_data_dir` failed.
    #[error("failed to clean user_data_dir: {0}")]
    Cleanup(#[source] std::io::Error),

    /// A configured extension could not be resolved — the path is missing, is
    /// neither a directory nor a `.crx`, or a `.crx` failed to unzip.
    #[error("failed to load extension {path:?}: {reason}")]
    ExtensionLoad {
        /// The configured extension path that failed.
        path: PathBuf,
        /// Human-readable cause (missing path, bad archive, IO error, …).
        reason: String,
    },
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_for_element_not_found_includes_selector() {
        let e = ZendriverError::ElementNotFound {
            selector: "button.foo".into(),
        };
        assert_eq!(e.to_string(), "element not found: button.foo");
    }

    #[test]
    fn display_for_timeout_includes_duration() {
        let e = ZendriverError::Timeout(Duration::from_secs(5));
        assert_eq!(e.to_string(), "timed out after 5s");
    }

    #[test]
    fn display_for_cdp_includes_code_and_message() {
        let e = ZendriverError::Cdp {
            code: -32602,
            message: "Invalid params".into(),
            data: None,
        };
        assert_eq!(e.to_string(), "CDP RPC error [-32602] Invalid params");
    }

    #[test]
    fn display_for_executable_not_found_includes_paths() {
        let e = ZendriverError::Browser(BrowserError::ExecutableNotFound {
            searched: vec![PathBuf::from("/usr/bin/google-chrome")],
        });
        assert!(e.to_string().contains("/usr/bin/google-chrome"));
    }

    #[test]
    fn from_transport_error_works() {
        let te = zendriver_transport::TransportError::Shutdown;
        let ze: ZendriverError = te.into();
        assert!(matches!(ze, ZendriverError::Transport(_)));
        assert!(ze.to_string().contains("connection shut down"));
    }

    #[test]
    fn from_call_error_rpc_minus_32602_maps_to_cdp_variant() {
        let ce = CallError::Rpc(-32602, "Invalid params".into(), None);
        let ze: ZendriverError = ce.into();
        match ze {
            ZendriverError::Cdp {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32602);
                assert_eq!(message, "Invalid params");
                assert!(data.is_none());
            }
            other => panic!("expected Cdp, got {other:?}"),
        }
    }

    #[test]
    fn from_call_error_cannot_find_context_maps_to_navigation() {
        let ce = CallError::Rpc(-32000, "Cannot find context with specified id".into(), None);
        let ze: ZendriverError = ce.into();
        match ze {
            ZendriverError::Navigation(m) => assert!(m.contains("Cannot find context")),
            other => panic!("expected Navigation, got {other:?}"),
        }
    }

    #[test]
    fn from_call_error_transport_maps_to_transport() {
        let ce = CallError::Transport(zendriver_transport::TransportError::Shutdown);
        let ze: ZendriverError = ce.into();
        assert!(matches!(ze, ZendriverError::Transport(_)));
    }

    #[test]
    fn from_stealth_error_works() {
        let se = zendriver_stealth::StealthError::ChromeVersionDetect("test".into());
        let ze: ZendriverError = se.into();
        assert!(matches!(ze, ZendriverError::Stealth(_)));
        assert!(ze.to_string().contains("test"));
    }

    #[test]
    fn display_element_stale() {
        let e = ZendriverError::ElementStale;
        assert_eq!(
            e.to_string(),
            "element is stale: refresh failed or origin not refreshable"
        );
    }

    #[test]
    fn display_not_refreshable() {
        let e = ZendriverError::NotRefreshable;
        assert_eq!(
            e.to_string(),
            "element not refreshable (was returned from a JS evaluation)"
        );
    }

    #[test]
    fn display_not_actionable_includes_duration_and_reason() {
        let e = ZendriverError::NotActionable(
            Duration::from_secs(5),
            "not visible: display: none".into(),
        );
        assert_eq!(
            e.to_string(),
            "element not actionable within 5s: not visible: display: none"
        );
    }

    #[test]
    fn display_frame_not_found() {
        let e = ZendriverError::FrameNotFound("F1".into());
        assert_eq!(e.to_string(), "frame not found: F1");
    }

    #[test]
    fn display_tab_not_found() {
        let e = ZendriverError::TabNotFound("S2".into());
        assert_eq!(e.to_string(), "tab not found: S2");
    }

    #[test]
    fn display_cookie() {
        let e = ZendriverError::Cookie("bad domain".into());
        assert_eq!(e.to_string(), "cookie operation failed: bad domain");
    }

    #[test]
    fn display_storage() {
        let e = ZendriverError::Storage("origin mismatch".into());
        assert_eq!(e.to_string(), "storage operation failed: origin mismatch");
    }

    #[test]
    fn display_history_navigation() {
        let e = ZendriverError::HistoryNavigation("no back history".into());
        assert_eq!(e.to_string(), "history navigation failed: no back history");
    }

    #[test]
    fn error_displays_snapshot() {
        let cases = vec![
            (
                "element_not_found",
                ZendriverError::ElementNotFound {
                    selector: "button.foo".into(),
                }
                .to_string(),
            ),
            (
                "timeout_5s",
                ZendriverError::Timeout(Duration::from_secs(5)).to_string(),
            ),
            (
                "cdp_invalid_params",
                ZendriverError::Cdp {
                    code: -32602,
                    message: "Invalid params".into(),
                    data: None,
                }
                .to_string(),
            ),
            (
                "navigation",
                ZendriverError::Navigation("ERR_NAME_NOT_RESOLVED".into()).to_string(),
            ),
            (
                "js_exception",
                ZendriverError::JsException("Error: boom".into()).to_string(),
            ),
        ];
        insta::assert_yaml_snapshot!("error_displays", cases);
    }
}
