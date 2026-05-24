//! Browser-wide cookie store backed by CDP `Network.*Cookies*` methods.
//!
//! [`CookieJar`] wraps a [`Connection`] and exposes ergonomic CRUD over
//! Chrome's cookie store. The jar is cheap to clone — internally an `Arc`
//! over a thin inner struct — so it can be passed around freely and is
//! suitable as both [`crate::Browser::cookies`] and [`crate::Tab::cookies`]
//! (both bind to the same browser-scope connection, since Chrome's cookie
//! store is browser-wide).
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! let jar = browser.cookies();
//! jar.set(zendriver::Cookie {
//!     name: "sid".into(),
//!     value: "abc123".into(),
//!     domain: ".example.com".into(),
//!     path: "/".into(),
//!     expires: None,
//!     http_only: true,
//!     secure: true,
//!     same_site: Some(zendriver::SameSite::Lax),
//!     url: None,
//! }).await?;
//! # Ok(()) }
//! ```
//!
//! ## Serialization — snake_case on disk, camelCase on the wire
//!
//! The public [`Cookie`] struct uses idiomatic snake_case for JSON output
//! (so users' on-disk JSON looks Rust-natural), while a private internal
//! mirror handles the CDP camelCase rename. Lossless conversion in both
//! directions; users see clean snake_case JSON, CDP sees camelCase, neither
//! side has to know the other exists.

pub mod persistence;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use zendriver_transport::Connection;

use crate::error::{Result, ZendriverError};

/// SameSite policy as defined by RFC 6265bis + the CDP `Network.CookieSameSite`
/// enum. Serializes as `"Strict"` / `"Lax"` / `"None"` to match CDP exactly
/// (and the standard's capitalization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// A single HTTP cookie.
///
/// Field shape matches the public Rust/JSON contract (snake_case). An
/// internal mirror handles the CDP camelCase rename — see the module-level
/// docs.
///
/// - `expires` is a Unix-epoch timestamp in **seconds** (with sub-second
///   precision), matching CDP's `Network.TimeSinceEpoch`. `None` means
///   "session cookie" (deleted when the browser closes).
/// - `url` is a constructor-time convenience for [`CookieJar::set`]: when
///   present, CDP infers `domain` + `path` + `secure` from it. CDP never
///   emits this field on reads, so it's always `None` on cookies returned
///   by [`CookieJar::all`] / [`CookieJar::for_url`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<SameSite>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Internal mirror of [`Cookie`] with CDP's camelCase rename. Used only at
/// the transport boundary — never escapes this module. Round-trips losslessly
/// through `From<Cookie>` / `From<CdpCookie>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires: Option<f64>,
    #[serde(default)]
    http_only: bool,
    #[serde(default)]
    secure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    same_site: Option<SameSite>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl From<Cookie> for CdpCookie {
    fn from(c: Cookie) -> Self {
        Self {
            name: c.name,
            value: c.value,
            domain: c.domain,
            path: c.path,
            expires: c.expires,
            http_only: c.http_only,
            secure: c.secure,
            same_site: c.same_site,
            url: c.url,
        }
    }
}

impl From<CdpCookie> for Cookie {
    fn from(c: CdpCookie) -> Self {
        Self {
            name: c.name,
            value: c.value,
            domain: c.domain,
            path: c.path,
            expires: c.expires,
            http_only: c.http_only,
            secure: c.secure,
            same_site: c.same_site,
            url: c.url,
        }
    }
}

/// Cheap-to-clone handle to the browser's cookie store. Wraps a
/// [`Connection`] in an [`Arc`]; cloning is reference-bump cheap.
///
/// All methods send commands at browser scope (no `sessionId`) — Chrome's
/// cookie store is shared across all tabs in the browser, so per-tab
/// scoping is meaningless for cookies. The owning Tab/Browser handle just
/// passes its `Connection` in.
#[derive(Clone, Debug)]
pub struct CookieJar {
    inner: Arc<CookieJarInner>,
}

#[derive(Debug)]
struct CookieJarInner {
    conn: Connection,
}

impl CookieJar {
    /// Construct a jar around a [`Connection`]. Typically called by
    /// `Browser::cookies()` / `Tab::cookies()` (T10) rather than user code.
    #[must_use]
    pub fn new(conn: Connection) -> Self {
        Self {
            inner: Arc::new(CookieJarInner { conn }),
        }
    }

    /// Return every cookie in the browser's store.
    ///
    /// Maps to `Network.getAllCookies`. The response field is `cookies: Cookie[]`.
    pub async fn all(&self) -> Result<Vec<Cookie>> {
        let resp = self
            .inner
            .conn
            .call_raw("Network.getAllCookies", json!({}), None)
            .await?;
        parse_cookies(&resp)
    }

    /// Return cookies that would be sent for `url`.
    ///
    /// Maps to `Network.getCookies` with `urls: [url]`. Same response shape
    /// as [`Self::all`]. Accepting [`url::Url`] (not `&str`) gates the call
    /// at the type level: CDP rejects malformed URLs silently (returning an
    /// empty cookie list), so making the caller present a parsed `Url`
    /// surfaces parse errors at construction time instead of as a confusing
    /// empty result.
    pub async fn for_url(&self, url: &url::Url) -> Result<Vec<Cookie>> {
        let resp = self
            .inner
            .conn
            .call_raw(
                "Network.getCookies",
                json!({ "urls": [url.as_str()] }),
                None,
            )
            .await?;
        parse_cookies(&resp)
    }

    /// Set a single cookie.
    ///
    /// Maps to `Network.setCookie` — the cookie fields are flattened into
    /// the top-level params object (CDP convention). Returns
    /// [`ZendriverError::Cookie`] if CDP reports `success: false`.
    pub async fn set(&self, cookie: Cookie) -> Result<()> {
        let cdp: CdpCookie = cookie.into();
        let params = serde_json::to_value(&cdp).map_err(ZendriverError::Serde)?;
        let resp = self
            .inner
            .conn
            .call_raw("Network.setCookie", params, None)
            .await?;
        // CDP returns `{ "success": bool }`. Treat absent as success
        // (defensive — older Chrome builds may omit on the happy path).
        if resp
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|ok| !ok)
        {
            return Err(ZendriverError::Cookie(format!(
                "Network.setCookie reported success=false for {cdp:?}"
            )));
        }
        Ok(())
    }

    /// Set many cookies in a single CDP call.
    ///
    /// Maps to `Network.setCookies` — params shape is `{ cookies: Cookie[] }`.
    /// Faster than looping over [`Self::set`] for bulk loads (one round-trip
    /// instead of N).
    pub async fn set_many(&self, cookies: Vec<Cookie>) -> Result<()> {
        let cdp: Vec<CdpCookie> = cookies.into_iter().map(CdpCookie::from).collect();
        self.inner
            .conn
            .call_raw("Network.setCookies", json!({ "cookies": cdp }), None)
            .await?;
        Ok(())
    }

    /// Delete cookies matching `name` and optional `domain` / `path` filters.
    ///
    /// Maps to `Network.deleteCookies`. CDP requires `name`; `domain` and
    /// `path` are optional narrowers (when omitted, all cookies with the
    /// given name across all domains/paths are deleted).
    pub async fn delete(&self, name: &str, domain: Option<&str>, path: Option<&str>) -> Result<()> {
        let mut params = json!({ "name": name });
        if let Some(d) = domain {
            params["domain"] = json!(d);
        }
        if let Some(p) = path {
            params["path"] = json!(p);
        }
        self.inner
            .conn
            .call_raw("Network.deleteCookies", params, None)
            .await?;
        Ok(())
    }

    /// Clear the entire browser cookie store. Maps to
    /// `Network.clearBrowserCookies` — no params, no response payload.
    pub async fn clear(&self) -> Result<()> {
        self.inner
            .conn
            .call_raw("Network.clearBrowserCookies", json!({}), None)
            .await?;
        Ok(())
    }
}

/// Shared parser for `Network.getAllCookies` and `Network.getCookies` —
/// both responses use `{ cookies: CdpCookie[] }`.
#[allow(clippy::result_large_err)] // ZendriverError variance is the project-wide return type
fn parse_cookies(resp: &serde_json::Value) -> Result<Vec<Cookie>> {
    let arr = resp
        .get("cookies")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ZendriverError::Cookie("response missing `cookies` array".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let cdp: CdpCookie = serde_json::from_value(v.clone()).map_err(ZendriverError::Serde)?;
        out.push(cdp.into());
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;

    /// [`CookieJar::all`] dispatches `Network.getAllCookies` and parses the
    /// `cookies` array (with camelCase fields) into snake_case [`Cookie`]s.
    #[tokio::test]
    async fn all_parses_get_all_cookies_response() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());

        let call = tokio::spawn({
            let j = jar.clone();
            async move { j.all().await }
        });

        let id = mock.expect_cmd("Network.getAllCookies").await;
        mock.reply(
            id,
            json!({
                "cookies": [
                    {
                        "name": "sid",
                        "value": "abc",
                        "domain": ".example.com",
                        "path": "/",
                        "expires": 1_700_000_000.5,
                        "httpOnly": true,
                        "secure": true,
                        "sameSite": "Lax",
                    },
                    {
                        "name": "theme",
                        "value": "dark",
                        "domain": "example.com",
                        "path": "/",
                        "httpOnly": false,
                        "secure": false,
                    },
                ]
            }),
        )
        .await;

        let cookies = call.await.unwrap().unwrap();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "abc");
        assert_eq!(cookies[0].domain, ".example.com");
        assert_eq!(cookies[0].path, "/");
        assert!((cookies[0].expires.unwrap() - 1_700_000_000.5).abs() < 1e-6);
        assert!(cookies[0].http_only);
        assert!(cookies[0].secure);
        assert_eq!(cookies[0].same_site, Some(SameSite::Lax));
        assert_eq!(cookies[1].name, "theme");
        assert!(!cookies[1].http_only);
        assert_eq!(cookies[1].expires, None);
        assert_eq!(cookies[1].same_site, None);

        conn.shutdown();
    }

    /// [`CookieJar::set`] dispatches `Network.setCookie` with the cookie
    /// flattened into params, using CDP's camelCase field names.
    #[tokio::test]
    async fn set_dispatches_network_set_cookie_with_camel_case_payload() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());

        let call = tokio::spawn({
            let j = jar.clone();
            async move {
                j.set(Cookie {
                    name: "sid".into(),
                    value: "xyz".into(),
                    domain: ".example.com".into(),
                    path: "/".into(),
                    expires: None,
                    http_only: true,
                    secure: true,
                    same_site: Some(SameSite::Strict),
                    url: None,
                })
                .await
            }
        });

        let id = mock.expect_cmd("Network.setCookie").await;
        let params = &mock.last_sent()["params"];
        assert_eq!(params["name"], "sid");
        assert_eq!(params["value"], "xyz");
        assert_eq!(params["domain"], ".example.com");
        assert_eq!(params["path"], "/");
        assert_eq!(params["httpOnly"], true);
        assert_eq!(params["secure"], true);
        assert_eq!(params["sameSite"], "Strict");
        // Snake-case names must NOT appear on the wire.
        assert!(params.get("http_only").is_none());
        assert!(params.get("same_site").is_none());

        mock.reply(id, json!({ "success": true })).await;
        call.await.unwrap().unwrap();

        conn.shutdown();
    }

    /// [`CookieJar::delete`] dispatches `Network.deleteCookies` with `name`
    /// always present and `domain` / `path` included only when supplied.
    #[tokio::test]
    async fn delete_dispatches_network_delete_cookies_with_filters() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());

        // Variant A: name only.
        let call_a = tokio::spawn({
            let j = jar.clone();
            async move { j.delete("sid", None, None).await }
        });
        let id_a = mock.expect_cmd("Network.deleteCookies").await;
        let params_a = &mock.last_sent()["params"];
        assert_eq!(params_a["name"], "sid");
        assert!(params_a.get("domain").is_none());
        assert!(params_a.get("path").is_none());
        mock.reply(id_a, json!({})).await;
        call_a.await.unwrap().unwrap();

        // Variant B: name + domain + path.
        let call_b = tokio::spawn({
            let j = jar.clone();
            async move { j.delete("sid", Some(".example.com"), Some("/api")).await }
        });
        let id_b = mock.expect_cmd("Network.deleteCookies").await;
        let params_b = &mock.last_sent()["params"];
        assert_eq!(params_b["name"], "sid");
        assert_eq!(params_b["domain"], ".example.com");
        assert_eq!(params_b["path"], "/api");
        mock.reply(id_b, json!({})).await;
        call_b.await.unwrap().unwrap();

        conn.shutdown();
    }
}
