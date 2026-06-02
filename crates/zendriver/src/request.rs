//! Browser-context HTTP: `tab.request()` runs `fetch` in the page (cookies +
//! CORS inherited) with an opt-in `Network.loadNetworkResource` bypass.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

// ──────────────────────────────────────────────────────────────────────────────
// HTTP method
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Patch,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Patch => "PATCH",
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// RequestBuilder
// ──────────────────────────────────────────────────────────────────────────────

/// A builder for browser-context HTTP requests.
///
/// Obtain via [`Tab::request`]. Chain method/header/body setters, then call
/// [`send`][Self::send] (in-page `fetch`) or enable [`bypass_cors`][Self::bypass_cors]
/// for the privileged `Network.loadNetworkResource` path.
pub struct RequestBuilder<'a> {
    tab: &'a Tab,
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    bypass: bool,
}

impl<'a> RequestBuilder<'a> {
    pub(crate) fn new(tab: &'a Tab) -> Self {
        Self {
            tab,
            method: Method::Get,
            url: String::new(),
            headers: vec![],
            body: None,
            bypass: false,
        }
    }

    /// Set URL and method to GET.
    pub fn get(mut self, url: impl Into<String>) -> Self {
        self.method = Method::Get;
        self.url = url.into();
        self
    }

    /// Set URL and method to POST.
    pub fn post(mut self, url: impl Into<String>) -> Self {
        self.method = Method::Post;
        self.url = url.into();
        self
    }

    /// Set URL and method to PUT.
    pub fn put(mut self, url: impl Into<String>) -> Self {
        self.method = Method::Put;
        self.url = url.into();
        self
    }

    /// Set URL and method to DELETE.
    pub fn delete(mut self, url: impl Into<String>) -> Self {
        self.method = Method::Delete;
        self.url = url.into();
        self
    }

    /// Set URL and method to HEAD.
    pub fn head(mut self, url: impl Into<String>) -> Self {
        self.method = Method::Head;
        self.url = url.into();
        self
    }

    /// Set URL and method to PATCH.
    pub fn patch(mut self, url: impl Into<String>) -> Self {
        self.method = Method::Patch;
        self.url = url.into();
        self
    }

    /// Append a request header.
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    /// Set raw bytes as the request body.
    pub fn body(mut self, b: impl Into<Vec<u8>>) -> Self {
        self.body = Some(b.into());
        self
    }

    /// Serialize `v` as JSON, set it as the body, and add `Content-Type: application/json`.
    pub fn json<T: Serialize>(mut self, v: &T) -> Result<Self> {
        let s = serde_json::to_vec(v).map_err(ZendriverError::from)?;
        self.headers
            .push(("Content-Type".into(), "application/json".into()));
        self.body = Some(s);
        Ok(self)
    }

    /// Use the privileged `Network.loadNetworkResource` path instead of
    /// in-page `fetch`. Inherits session cookies and bypasses page-level CORS.
    ///
    /// **GET only in v1.** For non-GET requests use the default `send()` path.
    pub fn bypass_cors(mut self) -> Self {
        self.bypass = true;
        self
    }

    // ──────────────────────────────────────────────────────────────────────────
    // JS generation (delegates to free fn for unit-testability)
    // ──────────────────────────────────────────────────────────────────────────

    fn fetch_js(&self) -> String {
        build_fetch_js(
            self.method.as_str(),
            &self.url,
            &self.headers,
            self.body.as_deref(),
        )
    }

    // ──────────────────────────────────────────────────────────────────────────
    // send
    // ──────────────────────────────────────────────────────────────────────────

    /// Execute the request. Uses in-page `fetch` by default; delegates to
    /// `Network.loadNetworkResource` when [`bypass_cors`][Self::bypass_cors]
    /// was called.
    pub async fn send(self) -> Result<Response> {
        if self.bypass {
            return self.send_bypass().await;
        }
        let js = self.fetch_js();
        let fr: FetchResult = self
            .tab
            .evaluate_main(&js)
            .await
            .map_err(|e| ZendriverError::Request(format!("fetch failed: {e}")))?;
        let body = BASE64
            .decode(&fr.body_b64)
            .map_err(|e| ZendriverError::Request(format!("body decode: {e}")))?;
        Ok(Response {
            status: fr.status,
            headers: fr.headers,
            body,
        })
    }

    /// Bypass-CORS path via `Network.loadNetworkResource`.
    async fn send_bypass(self) -> Result<Response> {
        // v1: GET only — loadNetworkResource is oriented around GET; non-GET
        // callers should use the default in-page fetch path instead.
        if self.method != Method::Get {
            return Err(ZendriverError::Request(
                "bypass_cors supports GET only; use the default fetch path for other methods"
                    .into(),
            ));
        }

        // `loadNetworkResource` requires a `frameId` for frame (page) targets.
        // Fetch the main frame ID lazily; failures here are surfaced as a
        // Request error rather than silently omitting the parameter (which
        // causes a `-32602` CDP error on Chrome >= 97).
        let frame_id = self.tab.main_frame().await.map_err(|e| {
            ZendriverError::Request(format!("get main frame for loadNetworkResource: {e}"))
        })?;
        let frame_id = frame_id.id().to_owned();

        let res = self
            .tab
            .session()
            .call(
                "Network.loadNetworkResource",
                json!({
                    "frameId": frame_id,
                    "url": self.url,
                    "options": { "disableCache": false, "includeCredentials": true },
                }),
            )
            .await
            .map_err(|e| ZendriverError::Request(format!("loadNetworkResource: {e}")))?;

        let resource = &res["resource"];
        if !resource["success"].as_bool().unwrap_or(false) {
            return Err(ZendriverError::Request(format!(
                "loadNetworkResource failed: {}",
                resource["netErrorName"].as_str().unwrap_or("unknown")
            )));
        }

        let status = resource["httpStatusCode"].as_u64().unwrap_or(0) as u16;

        // Map response headers if Chrome included them (object → HashMap).
        // `resource["headers"]` may be absent in older builds; leave empty then.
        let headers: HashMap<String, String> = resource["headers"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();

        // Body: when a stream handle is present, read via IO.read; else empty.
        let body = if let Some(stream) = resource["stream"].as_str() {
            read_io_stream(self.tab, stream).await?
        } else {
            Vec::new()
        };

        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Free JS-generation helper (unit-testable without a Tab)
// ──────────────────────────────────────────────────────────────────────────────

/// Build the in-page `fetch` invocation string.
///
/// All values are embedded via `serde_json::json!` to prevent injection.
/// The body is transmitted as a base64 string and reconstructed as a
/// `Uint8Array` via `atob` inside the JS snippet.
pub(crate) fn build_fetch_js(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> String {
    let headers_map: HashMap<&str, &str> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let headers_json = json!(headers_map);
    let body_json = match body {
        Some(b) => json!(BASE64.encode(b)),
        None => json!(null),
    };
    format!(
        r#"(async () => {{
  const body = {body};
  const init = {{ method: {method}, headers: {headers} }};
  if (body !== null) {{ const bin = atob(body); const u = new Uint8Array(bin.length);
    for (let i=0;i<bin.length;i++) u[i]=bin.charCodeAt(i); init.body = u; }}
  const r = await fetch({url}, init);
  const buf = new Uint8Array(await r.arrayBuffer());
  let s=""; for (const x of buf) s += String.fromCharCode(x);
  const h = {{}}; r.headers.forEach((v,k)=>h[k]=v);
  return {{ status: r.status, headers: h, body_b64: btoa(s) }};
}})()"#,
        method = json!(method),
        headers = headers_json,
        url = json!(url),
        body = body_json,
    )
}

// ──────────────────────────────────────────────────────────────────────────────
// IO stream reader (bypass path)
// ──────────────────────────────────────────────────────────────────────────────

async fn read_io_stream(tab: &Tab, handle: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let r = tab
            .session()
            .call("IO.read", json!({ "handle": handle, "size": 65536 }))
            .await
            .map_err(|e| ZendriverError::Request(format!("IO.read: {e}")))?;
        let data = r["data"].as_str().unwrap_or_default();
        if r["base64Encoded"].as_bool().unwrap_or(false) {
            out.extend(
                BASE64
                    .decode(data)
                    .map_err(|e| ZendriverError::Request(format!("io b64: {e}")))?,
            );
        } else {
            out.extend_from_slice(data.as_bytes());
        }
        if r["eof"].as_bool().unwrap_or(true) {
            break;
        }
    }
    let _ = tab
        .session()
        .call("IO.close", json!({ "handle": handle }))
        .await;
    Ok(out)
}

// ──────────────────────────────────────────────────────────────────────────────
// Deserialization of the in-page fetch result
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
struct FetchResult {
    status: u16,
    headers: HashMap<String, String>,
    body_b64: String,
}

// ──────────────────────────────────────────────────────────────────────────────
// Response
// ──────────────────────────────────────────────────────────────────────────────

/// Response returned by [`RequestBuilder::send`].
pub struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Response {
    /// HTTP status code.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Response headers.
    #[must_use]
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    /// Raw response body bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// Decode the body as UTF-8 (lossy).
    pub fn text(&self) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.body).into_owned())
    }

    /// Deserialize the body as JSON into `T`.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(ZendriverError::from)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn fetch_js_contains_method_url_headers() {
        let js = build_fetch_js(
            "POST",
            "https://x/a",
            &[("X".into(), "y".into())],
            Some(b"hi"),
        );
        assert!(js.contains(r#""POST""#), "method missing: {js}");
        assert!(js.contains("https://x/a"), "url missing: {js}");
        // serde_json serialises {"X":"y"} — accept either quoting style
        assert!(
            js.contains(r#""X":"y""#) || js.contains(r#""X": "y""#),
            "header missing: {js}"
        );
        assert!(js.contains("btoa"), "btoa missing: {js}");
    }

    #[test]
    fn fetch_js_no_body_passes_null() {
        let js = build_fetch_js("GET", "https://x/b", &[], None);
        assert!(js.contains("null"), "null body marker missing: {js}");
    }

    #[test]
    fn response_json_round_trips() {
        let r = Response {
            status: 200,
            headers: HashMap::new(),
            body: br#"{"a":1}"#.to_vec(),
        };
        #[derive(serde::Deserialize)]
        struct A {
            a: i32,
        }
        assert_eq!(r.json::<A>().unwrap().a, 1);
    }

    #[test]
    fn response_text_round_trips() {
        let r = Response {
            status: 200,
            headers: HashMap::new(),
            body: b"hello".to_vec(),
        };
        assert_eq!(r.text().unwrap(), "hello");
    }

    #[test]
    fn response_status_and_bytes() {
        let r = Response {
            status: 404,
            headers: HashMap::new(),
            body: b"not found".to_vec(),
        };
        assert_eq!(r.status(), 404);
        assert_eq!(r.bytes(), b"not found");
    }
}
