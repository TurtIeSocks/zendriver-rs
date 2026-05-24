//! Public types for the Fetch interception API.
//!
//! - [`RequestStage`] selects which lifecycle point Chrome pauses on.
//! - [`ResourceType`] mirrors Chrome's `Network.ResourceType` enum.
//! - [`AbortReason`] mirrors Chrome's `Network.ErrorReason` enum used by
//!   `Fetch.failRequest`.
//! - [`RequestInfo`] / [`ResponseInfo`] / [`RequestOverrides`] carry the
//!   payloads surfaced to user code via the rule + stream APIs.

use std::collections::HashMap;

/// The lifecycle stage at which Chrome pauses an intercepted request.
///
/// Maps to the `stage` field of CDP's `Fetch.RequestPattern`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum RequestStage {
    /// Pause before the request is sent.
    Request,
    /// Pause after the response headers have been received.
    Response,
}

impl RequestStage {
    /// CDP wire-string for this stage (`"Request"` / `"Response"`).
    #[must_use]
    pub fn as_cdp_str(&self) -> &'static str {
        match self {
            Self::Request => "Request",
            Self::Response => "Response",
        }
    }
}

/// Resource type classification for an intercepted request.
///
/// Mirrors Chrome's [`Network.ResourceType`] enum used by `Fetch.RequestPattern`
/// and the `resourceType` field on `Fetch.requestPaused` events.
///
/// [`Network.ResourceType`]: https://chromedevtools.github.io/devtools-protocol/tot/Network/#type-ResourceType
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum ResourceType {
    Document,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    TextTrack,
    XHR,
    Fetch,
    EventSource,
    WebSocket,
    Manifest,
    SignedExchange,
    Ping,
    CSPViolationReport,
    Preflight,
    Other,
}

impl ResourceType {
    /// CDP wire-string for this resource type, matching the
    /// `Network.ResourceType` enum names exactly (e.g. `"XHR"`, `"Stylesheet"`).
    #[must_use]
    pub fn as_cdp_str(&self) -> &'static str {
        match self {
            Self::Document => "Document",
            Self::Stylesheet => "Stylesheet",
            Self::Image => "Image",
            Self::Media => "Media",
            Self::Font => "Font",
            Self::Script => "Script",
            Self::TextTrack => "TextTrack",
            Self::XHR => "XHR",
            Self::Fetch => "Fetch",
            Self::EventSource => "EventSource",
            Self::WebSocket => "WebSocket",
            Self::Manifest => "Manifest",
            Self::SignedExchange => "SignedExchange",
            Self::Ping => "Ping",
            Self::CSPViolationReport => "CSPViolationReport",
            Self::Preflight => "Preflight",
            Self::Other => "Other",
        }
    }
}

/// Reason supplied to `Fetch.failRequest` when aborting an intercepted request.
///
/// Mirrors Chrome's [`Network.ErrorReason`] enum verbatim.
///
/// [`Network.ErrorReason`]: https://chromedevtools.github.io/devtools-protocol/tot/Network/#type-ErrorReason
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum AbortReason {
    Failed,
    Aborted,
    TimedOut,
    AccessDenied,
    ConnectionClosed,
    ConnectionReset,
    ConnectionRefused,
    ConnectionAborted,
    ConnectionFailed,
    NameNotResolved,
    InternetDisconnected,
    AddressUnreachable,
    BlockedByClient,
    BlockedByResponse,
}

impl AbortReason {
    /// CDP wire-string for this abort reason, matching the
    /// `Network.ErrorReason` enum names exactly.
    #[must_use]
    pub fn as_cdp_str(&self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Aborted => "Aborted",
            Self::TimedOut => "TimedOut",
            Self::AccessDenied => "AccessDenied",
            Self::ConnectionClosed => "ConnectionClosed",
            Self::ConnectionReset => "ConnectionReset",
            Self::ConnectionRefused => "ConnectionRefused",
            Self::ConnectionAborted => "ConnectionAborted",
            Self::ConnectionFailed => "ConnectionFailed",
            Self::NameNotResolved => "NameNotResolved",
            Self::InternetDisconnected => "InternetDisconnected",
            Self::AddressUnreachable => "AddressUnreachable",
            Self::BlockedByClient => "BlockedByClient",
            Self::BlockedByResponse => "BlockedByResponse",
        }
    }
}

/// Information about an intercepted request, surfaced to rule closures and
/// stream consumers.
#[derive(Debug, Clone)]
pub struct RequestInfo {
    /// Full request URL (post-redirect resolution by Chrome).
    pub url: String,
    /// HTTP method (`GET`, `POST`, ...).
    pub method: String,
    /// Request headers as Chrome reported them.
    pub headers: HashMap<String, String>,
    /// Request body, if any (raw bytes; Chrome base64-decodes on our behalf).
    pub post_data: Option<Vec<u8>>,
    /// Chrome's classification of the request's resource type.
    pub resource_type: ResourceType,
}

/// Information about a response paused at the `Response` stage.
#[derive(Debug, Clone)]
pub struct ResponseInfo {
    /// HTTP status code.
    pub status: u16,
    /// HTTP status line text (e.g. `"OK"`, `"Not Found"`).
    pub status_text: String,
    /// Response headers.
    pub headers: HashMap<String, String>,
}

/// Per-field overrides for `Fetch.continueRequest`.
///
/// All fields are optional — `None` means "leave Chrome's original value
/// unchanged". Use [`Default`] to start with an empty override set.
#[derive(Debug, Clone, Default)]
pub struct RequestOverrides {
    /// Replace the request URL.
    pub url: Option<String>,
    /// Replace the HTTP method.
    pub method: Option<String>,
    /// Replace the full header set (CDP semantics: this is *replacement*, not
    /// merge — include every header you want sent).
    pub headers: Option<HashMap<String, String>>,
    /// Replace the request body.
    pub post_data: Option<Vec<u8>>,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Snapshot every enum variant against its CDP wire string. Catches
    /// silent typos that would otherwise only surface in live CDP traffic.
    #[test]
    fn enum_cdp_strings_snapshot() {
        let pairs = serde_json::json!({
            "RequestStage": [
                ["Request", RequestStage::Request.as_cdp_str()],
                ["Response", RequestStage::Response.as_cdp_str()],
            ],
            "ResourceType": [
                ["Document", ResourceType::Document.as_cdp_str()],
                ["Stylesheet", ResourceType::Stylesheet.as_cdp_str()],
                ["Image", ResourceType::Image.as_cdp_str()],
                ["Media", ResourceType::Media.as_cdp_str()],
                ["Font", ResourceType::Font.as_cdp_str()],
                ["Script", ResourceType::Script.as_cdp_str()],
                ["TextTrack", ResourceType::TextTrack.as_cdp_str()],
                ["XHR", ResourceType::XHR.as_cdp_str()],
                ["Fetch", ResourceType::Fetch.as_cdp_str()],
                ["EventSource", ResourceType::EventSource.as_cdp_str()],
                ["WebSocket", ResourceType::WebSocket.as_cdp_str()],
                ["Manifest", ResourceType::Manifest.as_cdp_str()],
                ["SignedExchange", ResourceType::SignedExchange.as_cdp_str()],
                ["Ping", ResourceType::Ping.as_cdp_str()],
                ["CSPViolationReport", ResourceType::CSPViolationReport.as_cdp_str()],
                ["Preflight", ResourceType::Preflight.as_cdp_str()],
                ["Other", ResourceType::Other.as_cdp_str()],
            ],
            "AbortReason": [
                ["Failed", AbortReason::Failed.as_cdp_str()],
                ["Aborted", AbortReason::Aborted.as_cdp_str()],
                ["TimedOut", AbortReason::TimedOut.as_cdp_str()],
                ["AccessDenied", AbortReason::AccessDenied.as_cdp_str()],
                ["ConnectionClosed", AbortReason::ConnectionClosed.as_cdp_str()],
                ["ConnectionReset", AbortReason::ConnectionReset.as_cdp_str()],
                ["ConnectionRefused", AbortReason::ConnectionRefused.as_cdp_str()],
                ["ConnectionAborted", AbortReason::ConnectionAborted.as_cdp_str()],
                ["ConnectionFailed", AbortReason::ConnectionFailed.as_cdp_str()],
                ["NameNotResolved", AbortReason::NameNotResolved.as_cdp_str()],
                ["InternetDisconnected", AbortReason::InternetDisconnected.as_cdp_str()],
                ["AddressUnreachable", AbortReason::AddressUnreachable.as_cdp_str()],
                ["BlockedByClient", AbortReason::BlockedByClient.as_cdp_str()],
                ["BlockedByResponse", AbortReason::BlockedByResponse.as_cdp_str()],
            ],
        });
        insta::assert_yaml_snapshot!("enum_cdp_strings", pairs);
    }
}
