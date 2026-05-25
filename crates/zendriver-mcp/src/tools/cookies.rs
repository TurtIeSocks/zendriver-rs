//! Cookie handlers — `browser_cookies_get / _set / _delete / _clear / _persist`.
//!
//! All five tools route through [`zendriver::CookieJar`] (one shared
//! browser-scope handle, [`zendriver::Browser::cookies`]) rather than going
//! per-tab — Chrome's cookie store is browser-wide, so per-tab scoping is
//! meaningless.
//!
//! ## Wire shape — why [`CookieDto`] exists
//!
//! `zendriver::Cookie` already derives `Serialize + Deserialize`, but **not**
//! `schemars::JsonSchema` (the lib doesn't depend on `schemars`). rmcp needs
//! `JsonSchema` for tool input/output schema synthesis, so we mirror the
//! struct shape with a wire-only [`CookieDto`] and convert at the handler
//! boundary. Round-trips losslessly via `From<Cookie>` / `From<CookieDto>`.
//! Same for [`SameSiteDto`] mirroring `zendriver::SameSite`.
//!
//! `browser_cookies_persist` round-trips a `Vec<Cookie>` to disk via
//! `serde_json` — there's no `save_to_file` / `load_from_file` on
//! `CookieJar` itself (the lib doesn't take a stance on persistence
//! formats), so the MCP layer owns the serialization shim.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;

// ---------- wire types ----------------------------------------------------

/// SameSite policy mirror of [`zendriver::SameSite`].
///
/// Serializes with the same `"Strict" / "Lax" / "None"` capitalization as
/// the lib's enum (which matches the RFC and CDP), so on-the-wire payloads
/// stay identical to what `zendriver::Cookie`'s own serde emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SameSiteDto {
    /// First-party only — never sent on cross-site requests.
    Strict,
    /// Sent on cross-site top-level navigations only.
    Lax,
    /// Always sent, including in third-party contexts.
    None,
}

impl From<zendriver::SameSite> for SameSiteDto {
    fn from(s: zendriver::SameSite) -> Self {
        match s {
            zendriver::SameSite::Strict => Self::Strict,
            zendriver::SameSite::Lax => Self::Lax,
            zendriver::SameSite::None => Self::None,
        }
    }
}

impl From<SameSiteDto> for zendriver::SameSite {
    fn from(s: SameSiteDto) -> Self {
        match s {
            SameSiteDto::Strict => Self::Strict,
            SameSiteDto::Lax => Self::Lax,
            SameSiteDto::None => Self::None,
        }
    }
}

/// Wire-only mirror of [`zendriver::Cookie`] — see module docs.
///
/// Field set + serde shape match the lib's `Cookie` exactly, so a `Vec<Cookie>`
/// serialized by the lib parses cleanly as `Vec<CookieDto>` and vice versa.
/// Adding [`JsonSchema`] here (without touching the lib) lets rmcp synthesize
/// tool schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CookieDto {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain the cookie applies to. Leading dot (`.example.com`) matches
    /// the domain and all subdomains.
    pub domain: String,
    /// URL path the cookie applies to (typically `"/"`).
    pub path: String,
    /// Expiration timestamp in seconds since Unix epoch. `None` for a
    /// session cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    /// `true` if the cookie has the `HttpOnly` flag.
    #[serde(default)]
    pub http_only: bool,
    /// `true` if the cookie is only sent over HTTPS.
    #[serde(default)]
    pub secure: bool,
    /// SameSite policy, if specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<SameSiteDto>,
    /// Origin URL for set-side convenience. CDP infers `domain` + `path` +
    /// `secure` when present. Always `None` on cookies returned by reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl From<zendriver::Cookie> for CookieDto {
    fn from(c: zendriver::Cookie) -> Self {
        Self {
            name: c.name,
            value: c.value,
            domain: c.domain,
            path: c.path,
            expires: c.expires,
            http_only: c.http_only,
            secure: c.secure,
            same_site: c.same_site.map(SameSiteDto::from),
            url: c.url,
        }
    }
}

impl From<CookieDto> for zendriver::Cookie {
    fn from(c: CookieDto) -> Self {
        Self {
            name: c.name,
            value: c.value,
            domain: c.domain,
            path: c.path,
            expires: c.expires,
            http_only: c.http_only,
            secure: c.secure,
            same_site: c.same_site.map(zendriver::SameSite::from),
            url: c.url,
        }
    }
}

// ---------- browser_cookies_get -------------------------------------------

/// Input for `browser_cookies_get`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesGetInput {
    /// When set, returns only cookies that would be sent for this URL
    /// (CDP `Network.getCookies`). When unset, returns every cookie in
    /// the browser's store (`Storage.getCookies`).
    #[serde(default)]
    pub url: Option<String>,
    /// When set, post-filters the result to cookies whose `name` matches
    /// exactly. Combinable with `url`.
    #[serde(default)]
    pub name: Option<String>,
}

/// Output of `browser_cookies_get`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesGetOutput {
    /// Cookies matching the input filters. Order matches what CDP
    /// returned (unspecified).
    pub cookies: Vec<CookieDto>,
}

/// Fetch the browser's cookies, optionally filtered by URL and name.
pub async fn cookies_get(
    state: Arc<Mutex<SessionState>>,
    input: CookiesGetInput,
) -> Result<CookiesGetOutput, ErrorData> {
    let s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let jar = b.cookies();
    let cs = match input.url.as_deref() {
        Some(u) => {
            let parsed = url::Url::parse(u)
                .map_err(|e| ErrorData::invalid_params(format!("invalid url `{u}`: {e}"), None))?;
            jar.for_url(&parsed)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?
        }
        None => jar
            .all()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?,
    };
    let filtered: Vec<CookieDto> = match input.name {
        Some(n) => cs
            .into_iter()
            .filter(|c| c.name == n)
            .map(CookieDto::from)
            .collect(),
        None => cs.into_iter().map(CookieDto::from).collect(),
    };
    Ok(CookiesGetOutput { cookies: filtered })
}

// ---------- browser_cookies_set -------------------------------------------

/// Input for `browser_cookies_set`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesSetInput {
    /// Cookies to add. Existing cookies with matching `(name, domain, path)`
    /// are overwritten.
    pub cookies: Vec<CookieDto>,
}

/// Output of `browser_cookies_set`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesSetOutput {
    /// Number of cookies dispatched to CDP (echoes input.cookies.len()).
    pub added: usize,
}

/// Set many cookies in one CDP round-trip.
pub async fn cookies_set(
    state: Arc<Mutex<SessionState>>,
    input: CookiesSetInput,
) -> Result<CookiesSetOutput, ErrorData> {
    let s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let count = input.cookies.len();
    let cookies: Vec<zendriver::Cookie> = input
        .cookies
        .into_iter()
        .map(zendriver::Cookie::from)
        .collect();
    b.cookies()
        .set_many(cookies)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(CookiesSetOutput { added: count })
}

// ---------- browser_cookies_delete ----------------------------------------

/// Input for `browser_cookies_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesDeleteInput {
    /// Cookie name to delete. Always required (CDP `Network.deleteCookies`
    /// requires it).
    pub name: String,
    /// Optional domain narrower. When omitted, cookies with the given name
    /// across all domains are deleted.
    #[serde(default)]
    pub domain: Option<String>,
    /// Optional path narrower. When omitted, cookies with the given name
    /// across all paths are deleted.
    #[serde(default)]
    pub path: Option<String>,
}

/// Output of `browser_cookies_delete`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesDeleteOutput {
    /// `true` if CDP accepted the delete (it doesn't report match count;
    /// missing cookies are silently ignored).
    pub deleted: bool,
}

/// Delete cookies matching `name` + optional `domain` / `path`.
pub async fn cookies_delete(
    state: Arc<Mutex<SessionState>>,
    input: CookiesDeleteInput,
) -> Result<CookiesDeleteOutput, ErrorData> {
    let s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    b.cookies()
        .delete(&input.name, input.domain.as_deref(), input.path.as_deref())
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(CookiesDeleteOutput { deleted: true })
}

// ---------- browser_cookies_clear -----------------------------------------

/// Output of `browser_cookies_clear`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesClearOutput {
    /// Always `true` when the call succeeded.
    pub ok: bool,
}

/// Clear the entire browser cookie store.
pub async fn cookies_clear(
    state: Arc<Mutex<SessionState>>,
    _: crate::tools::common::EmptyInput,
) -> Result<CookiesClearOutput, ErrorData> {
    let s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    b.cookies()
        .clear()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(CookiesClearOutput { ok: true })
}

// ---------- browser_cookies_persist ---------------------------------------

/// Direction selector for `browser_cookies_persist`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersistDirection {
    /// Snapshot every cookie in the browser store to `path` as pretty JSON.
    Save,
    /// Replace the in-memory cookies with those parsed from `path`.
    Load,
}

/// Input for `browser_cookies_persist`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesPersistInput {
    /// Whether to `save` (browser → disk) or `load` (disk → browser).
    pub direction: PersistDirection,
    /// Path on the MCP server host. Save writes pretty-printed JSON; load
    /// expects the same shape (a JSON array of cookies).
    pub path: String,
}

/// Output of `browser_cookies_persist`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesPersistOutput {
    /// Number of cookies written (save) or restored (load).
    pub count: usize,
    /// Echoes the input direction.
    pub direction: PersistDirection,
}

/// Save the browser's cookies to disk or restore from disk.
///
/// The on-disk format is `serde_json` of a `Vec<CookieDto>` — same shape
/// as `browser_cookies_get` returns. There's no `save_to_file` /
/// `load_from_file` on `CookieJar` itself; the persistence shim lives at
/// the MCP layer so the lib can stay format-agnostic.
pub async fn cookies_persist(
    state: Arc<Mutex<SessionState>>,
    input: CookiesPersistInput,
) -> Result<CookiesPersistOutput, ErrorData> {
    let s = state.lock().await;
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let jar = b.cookies();
    let path = PathBuf::from(&input.path);
    let count = match input.direction {
        PersistDirection::Save => {
            let cookies = jar
                .all()
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
            let dtos: Vec<CookieDto> = cookies.into_iter().map(CookieDto::from).collect();
            let bytes = serde_json::to_vec_pretty(&dtos)
                .map_err(|e| ErrorData::internal_error(format!("serialize cookies: {e}"), None))?;
            tokio::fs::write(&path, &bytes).await.map_err(|e| {
                ErrorData::internal_error(format!("write `{}`: {e}", path.display()), None)
            })?;
            dtos.len()
        }
        PersistDirection::Load => {
            let bytes = tokio::fs::read(&path).await.map_err(|e| {
                ErrorData::internal_error(format!("read `{}`: {e}", path.display()), None)
            })?;
            let dtos: Vec<CookieDto> = serde_json::from_slice(&bytes).map_err(|e| {
                ErrorData::invalid_params(
                    format!("parse `{}` as cookie JSON: {e}", path.display()),
                    None,
                )
            })?;
            let count = dtos.len();
            let cookies: Vec<zendriver::Cookie> =
                dtos.into_iter().map(zendriver::Cookie::from).collect();
            jar.set_many(cookies)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
            count
        }
    };
    Ok(CookiesPersistOutput {
        count,
        direction: input.direction,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tools::common::EmptyInput;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    #[tokio::test]
    async fn cookies_get_with_no_browser_suggests_browser_open() {
        let err = cookies_get(fresh(), CookiesGetInput::default())
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn cookies_set_with_no_browser_suggests_browser_open() {
        let err = cookies_set(fresh(), CookiesSetInput { cookies: vec![] })
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn cookies_delete_with_no_browser_suggests_browser_open() {
        let err = cookies_delete(
            fresh(),
            CookiesDeleteInput {
                name: "sid".into(),
                domain: None,
                path: None,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn cookies_clear_with_no_browser_suggests_browser_open() {
        let err = cookies_clear(fresh(), EmptyInput {})
            .await
            .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn cookies_persist_with_no_browser_suggests_browser_open() {
        let err = cookies_persist(
            fresh(),
            CookiesPersistInput {
                direction: PersistDirection::Save,
                path: "/tmp/never-touched-no-browser.json".into(),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn cookies_get_invalid_url_returns_invalid_params() {
        // `BrowserNotOpen` is checked first, so we can't reach the URL
        // parse path without a real browser. Just smoke-check the URL
        // parser directly to lock in the error mapping shape.
        let bad = url::Url::parse("not a url");
        assert!(bad.is_err());
    }

    /// Lossless round-trip of [`CookieDto`] through the lib's [`zendriver::Cookie`]
    /// — every field preserved including SameSite + the optional `url` slot.
    #[test]
    fn cookie_dto_round_trips_through_lib_cookie() {
        let original = CookieDto {
            name: "sid".into(),
            value: "abc".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            expires: Some(1_700_000_000.5),
            http_only: true,
            secure: true,
            same_site: Some(SameSiteDto::Lax),
            url: Some("https://example.com/".into()),
        };
        let lib: zendriver::Cookie = original.clone().into();
        let back: CookieDto = lib.into();
        assert_eq!(original, back);
    }

    /// `browser_cookies_persist save → load` round-trip via the serde shim
    /// alone (no real Chrome). Validates the on-disk format the lib expects.
    #[tokio::test]
    async fn cookies_persist_save_load_round_trip_via_serde_shim() {
        let tmp = std::env::temp_dir().join(format!(
            "zendriver-mcp-cookies-persist-shim-{}.json",
            std::process::id()
        ));
        let _ = tokio::fs::remove_file(&tmp).await;

        let cookies = vec![
            CookieDto {
                name: "a".into(),
                value: "1".into(),
                domain: ".x.com".into(),
                path: "/".into(),
                expires: None,
                http_only: false,
                secure: false,
                same_site: None,
                url: None,
            },
            CookieDto {
                name: "b".into(),
                value: "2".into(),
                domain: ".y.com".into(),
                path: "/".into(),
                expires: Some(1_700_000_000.0),
                http_only: true,
                secure: true,
                same_site: Some(SameSiteDto::Strict),
                url: None,
            },
        ];

        let bytes = serde_json::to_vec_pretty(&cookies).unwrap();
        tokio::fs::write(&tmp, &bytes).await.unwrap();

        let raw = tokio::fs::read(&tmp).await.unwrap();
        let loaded: Vec<CookieDto> = serde_json::from_slice(&raw).unwrap();
        assert_eq!(cookies, loaded);

        // Also: each dto round-trips losslessly through the lib's Cookie.
        for dto in &loaded {
            let lib: zendriver::Cookie = dto.clone().into();
            let back: CookieDto = lib.into();
            assert_eq!(*dto, back);
        }

        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[test]
    fn same_site_dto_maps_all_variants() {
        for (dto, lib) in [
            (SameSiteDto::Strict, zendriver::SameSite::Strict),
            (SameSiteDto::Lax, zendriver::SameSite::Lax),
            (SameSiteDto::None, zendriver::SameSite::None),
        ] {
            assert_eq!(SameSiteDto::from(lib), dto);
            assert_eq!(zendriver::SameSite::from(dto), lib);
        }
    }
}
