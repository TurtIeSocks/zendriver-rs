//! Error hierarchy for the `zendriver` crate.

use std::path::PathBuf;
use std::time::Duration;

use zendriver_transport::CallError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ZendriverError {
    #[error("browser process failed: {0}")]
    Browser(#[from] BrowserError),

    #[error("transport: {0}")]
    Transport(#[from] zendriver_transport::TransportError),

    #[error("CDP RPC error [{code}] {message}")]
    Cdp {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },

    #[error("element not found: {selector}")]
    ElementNotFound { selector: String },

    #[error("timed out after {0:?}")]
    Timeout(Duration),

    #[error("navigation failed: {0}")]
    Navigation(String),

    #[error("javascript exception: {0}")]
    JsException(String),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("stealth: {0}")]
    Stealth(#[from] zendriver_stealth::StealthError),
}

impl From<CallError> for ZendriverError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Transport(t) => ZendriverError::Transport(t),
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

pub type Result<T, E = ZendriverError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserError {
    #[error("chrome executable not found; searched: {searched:?}")]
    ExecutableNotFound { searched: Vec<PathBuf> },

    #[error("chrome failed to start: {0}")]
    SpawnFailed(#[source] std::io::Error),

    #[error("chrome exited before WS endpoint became available (status: {0:?})")]
    EarlyExit(std::process::ExitStatus),

    #[error("timed out waiting for chrome WS endpoint")]
    WsTimeout,

    #[error("could not parse devtools endpoint from chrome stderr")]
    DevtoolsParse,

    #[error("failed to clean user_data_dir: {0}")]
    Cleanup(#[source] std::io::Error),
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
