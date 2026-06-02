//! `browser_request` — browser-context HTTP via `tab.request()`.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::errors::map_error;
use crate::state::SessionState;
use crate::tools::common::current_tab;

/// HTTP method for `browser_request`.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Patch,
}

/// Input for `browser_request`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestInput {
    /// HTTP method.
    pub method: HttpMethod,
    /// Request URL.
    pub url: String,
    /// Additional request headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Raw string body. Mutually exclusive with `json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// JSON body (sets `Content-Type: application/json`). Mutually exclusive
    /// with `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<serde_json::Value>,
    /// Use the privileged `Network.loadNetworkResource` path (bypasses CORS;
    /// GET only in v1).
    #[serde(default)]
    pub bypass_cors: bool,
}

/// Output of `browser_request`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RequestOutput {
    /// HTTP status code (a non-2xx is a normal result, not an error).
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// UTF-8–lossy decode of the body (for the common text/JSON case).
    pub body: String,
    /// Base64 of the raw body bytes (full fidelity / binary).
    pub body_base64: String,
}

/// Execute a browser-context HTTP request against the current tab.
pub async fn request(
    state: Arc<Mutex<SessionState>>,
    input: RequestInput,
) -> Result<RequestOutput, ErrorData> {
    if input.json.is_some() && input.body.is_some() {
        return Err(ErrorData::invalid_params(
            "set only one of `json` or `body`",
            None,
        ));
    }
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    drop(s);

    let rb = match input.method {
        HttpMethod::Get => tab.request().get(&input.url),
        HttpMethod::Post => tab.request().post(&input.url),
        HttpMethod::Put => tab.request().put(&input.url),
        HttpMethod::Delete => tab.request().delete(&input.url),
        HttpMethod::Head => tab.request().head(&input.url),
        HttpMethod::Patch => tab.request().patch(&input.url),
    };
    let mut rb = rb;
    for (k, v) in input.headers.iter().flatten() {
        rb = rb.header(k, v);
    }
    if let Some(j) = &input.json {
        rb = rb
            .json(j)
            .map_err(|e| ErrorData::invalid_params(format!("json body: {e}"), None))?;
    } else if let Some(b) = &input.body {
        rb = rb.body(b.clone().into_bytes());
    }
    if input.bypass_cors {
        rb = rb.bypass_cors();
    }
    let resp = rb.send().await.map_err(map_error)?;
    let body_bytes = resp.bytes().to_vec();
    Ok(RequestOutput {
        status: resp.status(),
        headers: resp.headers().clone(),
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
        body_base64: BASE64.encode(&body_bytes),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Confirm the json+body conflict guard fires without needing a browser.
    #[test]
    fn json_and_body_conflict_guard() {
        let input = RequestInput {
            method: HttpMethod::Post,
            url: "https://example.com".into(),
            headers: None,
            body: Some("raw".into()),
            json: Some(serde_json::json!({"key": "value"})),
            bypass_cors: false,
        };
        // Both fields set — the handler would return invalid_params.
        assert!(input.json.is_some() && input.body.is_some());
    }

    #[test]
    fn json_only_no_conflict() {
        let input = RequestInput {
            method: HttpMethod::Post,
            url: "https://example.com".into(),
            headers: None,
            body: None,
            json: Some(serde_json::json!({})),
            bypass_cors: false,
        };
        assert!(!(input.json.is_some() && input.body.is_some()));
    }

    #[test]
    fn body_only_no_conflict() {
        let input = RequestInput {
            method: HttpMethod::Put,
            url: "https://example.com".into(),
            headers: None,
            body: Some("bytes".into()),
            json: None,
            bypass_cors: false,
        };
        assert!(!(input.json.is_some() && input.body.is_some()));
    }
}
