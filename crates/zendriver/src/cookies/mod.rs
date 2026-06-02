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
//!     http_only: true,
//!     secure: true,
//!     same_site: Some(zendriver::SameSite::Lax),
//!     ..Default::default()
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

/// SameSite policy as defined by RFC 6265bis.
///
/// Mirrors the CDP `Network.CookieSameSite` enum. Serializes as
/// `"Strict"` / `"Lax"` / `"None"` to match CDP exactly (and the
/// standard's capitalization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    /// First-party only — never sent on cross-site requests.
    Strict,
    /// Sent on cross-site top-level navigations only (the default
    /// modern-browser behavior for unspecified SameSite).
    Lax,
    /// Always sent, including in third-party contexts. Requires `Secure`.
    None,
}

/// Cookie priority hint as defined by the (non-standard) `Priority`
/// attribute Chrome honors during eviction.
///
/// Mirrors the CDP `Network.CookiePriority` enum. Serializes as
/// `"Low"` / `"Medium"` / `"High"` to match CDP exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CookiePriority {
    /// Lowest retention priority — evicted first under storage pressure.
    Low,
    /// Default priority when the attribute is unspecified.
    Medium,
    /// Highest retention priority — evicted last.
    High,
}

/// Scheme of the request that set the cookie.
///
/// Mirrors the CDP `Network.CookieSourceScheme` enum. Serializes as
/// `"Unset"` / `"NonSecure"` / `"Secure"` to match CDP exactly. CDP uses
/// this together with `secure` to implement the "schemeful same-site"
/// model; `Unset` is the default for cookies set without a known scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CookieSourceScheme {
    /// Scheme unknown / not recorded.
    Unset,
    /// Cookie was set over a non-secure (`http`) origin.
    NonSecure,
    /// Cookie was set over a secure (`https`) origin.
    Secure,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain the cookie applies to. Leading dot (e.g. `.example.com`)
    /// matches the domain and all subdomains.
    pub domain: String,
    /// URL path the cookie applies to (typically `"/"`).
    pub path: String,
    /// Expiration timestamp in seconds since Unix epoch. `None` for a
    /// session cookie (deleted when the browser closes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    /// `true` if the cookie has the `HttpOnly` flag (not accessible via
    /// JavaScript).
    #[serde(default)]
    pub http_only: bool,
    /// `true` if the cookie is only sent over HTTPS.
    #[serde(default)]
    pub secure: bool,
    /// SameSite policy, if specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<SameSite>,
    /// Origin URL for [`CookieJar::set`] convenience — CDP infers
    /// `domain` + `path` + `secure` from it. Always `None` on cookies
    /// returned by reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Eviction-priority hint (`Network.CookiePriority`). `None` lets
    /// Chrome apply its default (`Medium`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<CookiePriority>,
    /// First-Party Sets membership flag (`sameParty`). Rarely set by
    /// hand; populated by CDP on reads when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_party: Option<bool>,
    /// Scheme of the origin that set the cookie
    /// (`Network.CookieSourceScheme`). Drives schemeful same-site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_scheme: Option<CookieSourceScheme>,
    /// Source port of the origin that set the cookie. `-1` in CDP means
    /// "unspecified"; modeled as a plain `Option<i32>` here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_port: Option<i32>,
    /// CHIPS partition key — the top-level site the partitioned cookie is
    /// scoped to (e.g. `"https://example.com"`).
    ///
    /// Modeled as the top-level-site string for the common case. Newer CDP
    /// channels expose `partitionKey` as a structured object
    /// (`{ topLevelSite, hasCrossSiteAncestor }`); that variant is
    /// deferred — set the string form for now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<CookiePriority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    same_party: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_scheme: Option<CookieSourceScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    partition_key: Option<String>,
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
            priority: c.priority,
            same_party: c.same_party,
            source_scheme: c.source_scheme,
            source_port: c.source_port,
            partition_key: c.partition_key,
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
            priority: c.priority,
            same_party: c.same_party,
            source_scheme: c.source_scheme,
            source_port: c.source_port,
            partition_key: c.partition_key,
        }
    }
}

/// Cheap-to-clone handle to the browser's cookie store.
///
/// Wraps a [`Connection`] in an [`Arc`]; cloning is reference-bump cheap.
///
/// All methods send commands at browser scope (no `sessionId`) — Chrome's
/// cookie store is shared across all tabs in the browser, so per-tab
/// scoping is meaningless for cookies. Construct via
/// [`crate::Browser::cookies`] or [`crate::Tab::cookies`] — both produce
/// jars bound to the same underlying store.
#[derive(Clone, Debug)]
pub struct CookieJar {
    inner: Arc<CookieJarInner>,
}

#[derive(Debug)]
struct CookieJarInner {
    conn: Connection,
}

impl CookieJar {
    /// Construct a jar around a [`Connection`].
    ///
    /// Typically called by [`crate::Browser::cookies`] /
    /// [`crate::Tab::cookies`] rather than user code.
    #[must_use]
    pub fn new(conn: Connection) -> Self {
        Self {
            inner: Arc::new(CookieJarInner { conn }),
        }
    }

    /// Return every cookie in the browser's store.
    ///
    /// Maps to `Storage.getCookies`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let jar = browser.cookies();
    /// for c in jar.all().await? {
    ///     println!("{}={} ({})", c.name, c.value, c.domain);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn all(&self) -> Result<Vec<Cookie>> {
        let resp = self
            .inner
            .conn
            .call_raw("Storage.getCookies", json!({}), None)
            .await?;
        parse_cookies(&resp)
    }

    /// Return cookies that would be sent for `url`.
    ///
    /// Maps to `Network.getCookies` with `urls: [url]`. Accepting a parsed
    /// [`url::Url`] surfaces malformed-URL errors at construction time
    /// instead of as a confusing empty result.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let jar = browser.cookies();
    /// let url = url::Url::parse("https://example.com/").unwrap();
    /// let cookies = jar.for_url(&url).await?;
    /// # let _ = cookies;
    /// # Ok(()) }
    /// ```
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
    /// Maps to `Storage.setCookies` with a one-element batch. The singular
    /// `Network.setCookie` is removed from newer Chrome / Chromium builds
    /// (the CDP method is reported as not found), so the bulk endpoint is
    /// the portable choice and is what the parent zendriver-python project
    /// uses too.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Cookie`] if CDP rejects the cookie.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::{Cookie, SameSite};
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.cookies().set(Cookie {
    ///     name: "sid".into(),
    ///     value: "abc".into(),
    ///     domain: ".example.com".into(),
    ///     path: "/".into(),
    ///     http_only: true,
    ///     secure: true,
    ///     same_site: Some(SameSite::Lax),
    ///     ..Default::default()
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn set(&self, cookie: Cookie) -> Result<()> {
        let cdp: CdpCookie = cookie.into();
        let cdp_json = serde_json::to_value(&cdp).map_err(ZendriverError::Serde)?;
        self.inner
            .conn
            .call_raw("Storage.setCookies", json!({ "cookies": [cdp_json] }), None)
            .await?;
        Ok(())
    }

    /// Set many cookies in a single CDP call.
    ///
    /// Maps to `Storage.setCookies`. Faster than looping over [`Self::set`]
    /// for bulk loads (one round-trip instead of N).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::Cookie;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// let cookies = vec![
    ///     Cookie { name: "a".into(), value: "1".into(), domain: ".x.com".into(),
    ///              path: "/".into(), ..Default::default() },
    /// ];
    /// browser.cookies().set_many(cookies).await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_many(&self, cookies: Vec<Cookie>) -> Result<()> {
        let cdp: Vec<CdpCookie> = cookies.into_iter().map(CdpCookie::from).collect();
        self.inner
            .conn
            .call_raw("Storage.setCookies", json!({ "cookies": cdp }), None)
            .await?;
        Ok(())
    }

    /// Delete cookies matching `name` and optional `domain` / `path`.
    ///
    /// Maps to `Network.deleteCookies`. CDP requires `name`; `domain` and
    /// `path` are optional narrowers (when omitted, all cookies with the
    /// given name across all domains/paths are deleted).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.cookies().delete("sid", Some(".example.com"), Some("/")).await?;
    /// # Ok(()) }
    /// ```
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

    /// Clear the entire browser cookie store.
    ///
    /// Maps to `Storage.clearCookies` (the modern CDP method; newer
    /// Chrome / Chromium builds dropped `Storage.clearCookies`
    /// from the wire). No params, no response payload.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// browser.cookies().clear().await?;
    /// # Ok(()) }
    /// ```
    pub async fn clear(&self) -> Result<()> {
        self.inner
            .conn
            .call_raw("Storage.clearCookies", json!({}), None)
            .await?;
        Ok(())
    }
}

/// Shared parser for `Storage.getCookies` and `Network.getCookies` —
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

    /// [`CookieJar::all`] dispatches `Storage.getCookies` and parses the
    /// `cookies` array (with camelCase fields) into snake_case [`Cookie`]s.
    #[tokio::test]
    async fn all_parses_get_all_cookies_response() {
        let (mut mock, conn) = MockConnection::pair();
        let jar = CookieJar::new(conn.clone());

        let call = tokio::spawn({
            let j = jar.clone();
            async move { j.all().await }
        });

        let id = mock.expect_cmd("Storage.getCookies").await;
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

    /// [`CookieJar::set`] dispatches `Storage.setCookies` with a one-element
    /// `cookies` array, using CDP's camelCase field names. Singular
    /// `Network.setCookie` was removed in newer Chrome builds; the bulk
    /// endpoint is portable and matches zendriver-python's behaviour.
    #[tokio::test]
    async fn set_dispatches_network_set_cookies_with_camel_case_payload() {
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
                    ..Default::default()
                })
                .await
            }
        });

        let id = mock.expect_cmd("Storage.setCookies").await;
        let params = &mock.last_sent()["params"];
        let arr = params["cookies"]
            .as_array()
            .expect("setCookies payload must carry a cookies array");
        assert_eq!(arr.len(), 1);
        let c = &arr[0];
        assert_eq!(c["name"], "sid");
        assert_eq!(c["value"], "xyz");
        assert_eq!(c["domain"], ".example.com");
        assert_eq!(c["path"], "/");
        assert_eq!(c["httpOnly"], true);
        assert_eq!(c["secure"], true);
        assert_eq!(c["sameSite"], "Strict");
        // Snake-case names must NOT appear on the wire.
        assert!(c.get("http_only").is_none());
        assert!(c.get("same_site").is_none());

        mock.reply(id, json!({})).await;
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

    /// A [`Cookie`] carrying the CHIPS / priority extension fields must
    /// surface them on the `Storage.setCookies` wire payload with CDP's
    /// camelCase names + enum string forms.
    #[tokio::test]
    async fn set_cookie_with_priority_and_partition_key_on_wire() {
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
                    priority: Some(CookiePriority::High),
                    same_party: Some(true),
                    source_scheme: Some(CookieSourceScheme::Secure),
                    source_port: Some(443),
                    partition_key: Some("https://top".into()),
                    ..Default::default()
                })
                .await
            }
        });

        let id = mock.expect_cmd("Storage.setCookies").await;
        let params = &mock.last_sent()["params"];
        let c = &params["cookies"]
            .as_array()
            .expect("setCookies payload must carry a cookies array")[0];
        assert_eq!(c["priority"], "High");
        assert_eq!(c["sameParty"], true);
        assert_eq!(c["sourceScheme"], "Secure");
        assert_eq!(c["sourcePort"], 443);
        assert_eq!(c["partitionKey"], "https://top");
        // Snake-case names must NOT leak onto the wire.
        assert!(c.get("same_party").is_none());
        assert!(c.get("source_scheme").is_none());
        assert!(c.get("partition_key").is_none());

        mock.reply(id, json!({})).await;
        call.await.unwrap().unwrap();

        conn.shutdown();
    }

    /// Serializing a [`Cookie`] with the new fields to JSON and back must
    /// preserve every value losslessly (snake_case on disk).
    #[test]
    fn cookie_json_roundtrip_preserves_new_fields() {
        let cookie = Cookie {
            name: "sid".into(),
            value: "xyz".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            priority: Some(CookiePriority::Low),
            same_party: Some(false),
            source_scheme: Some(CookieSourceScheme::NonSecure),
            source_port: Some(80),
            partition_key: Some("https://top.example".into()),
            ..Default::default()
        };

        let json = serde_json::to_string(&cookie).unwrap();
        // snake_case on disk for the public shape.
        assert!(json.contains("\"source_scheme\""));
        assert!(json.contains("\"partition_key\""));

        let back: Cookie = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cookie);
        assert_eq!(back.priority, Some(CookiePriority::Low));
        assert_eq!(back.same_party, Some(false));
        assert_eq!(back.source_scheme, Some(CookieSourceScheme::NonSecure));
        assert_eq!(back.source_port, Some(80));
        assert_eq!(back.partition_key.as_deref(), Some("https://top.example"));
    }

    /// Deserializing a CDP cookie that omits the new fields must leave them
    /// `None` (serde `default`) and must not panic — this is the
    /// crash-immunity contract that protected rs from the Chrome-146
    /// `sameParty` regression.
    #[test]
    fn cookie_read_missing_new_fields_is_none() {
        let cdp: CdpCookie = serde_json::from_value(json!({
            "name": "sid",
            "value": "xyz",
            "domain": ".example.com",
            "path": "/",
            "httpOnly": true,
            "secure": true,
        }))
        .expect("a CDP cookie lacking the new fields must still parse");
        let cookie: Cookie = cdp.into();

        assert_eq!(cookie.priority, None);
        assert_eq!(cookie.same_party, None);
        assert_eq!(cookie.source_scheme, None);
        assert_eq!(cookie.source_port, None);
        assert_eq!(cookie.partition_key, None);
    }
}
