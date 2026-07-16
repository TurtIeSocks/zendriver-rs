//! Proxy-URL parsing shared by per-context proxy configuration.
//!
//! Chrome's `Target.createBrowserContext` `proxyServer` field ignores any
//! userinfo in the URL, so credentials are split out here for the
//! interception auth handler (`Fetch.authRequired`) and the returned
//! `server` string is userinfo-free.

use percent_encoding::percent_decode_str;

use crate::error::ZendriverError;

/// A proxy split into the CDP `proxyServer` string (no credentials) and the
/// optional auth credentials answered separately via `Fetch.authRequired`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedProxy {
    /// `scheme://host:port` with any userinfo stripped.
    pub server: String,
    /// `(user, pass)` when the URL carried userinfo; else `None`.
    pub credentials: Option<(String, String)>,
}

/// Parse `scheme://[user[:pass]@]host:port[/...]` into a [`ParsedProxy`].
///
/// Userinfo is percent-decoded before being returned as credentials — Chrome
/// and upstream proxies expect the literal username/password, not the
/// percent-encoded form `url::Url` reports (e.g. a password containing `@`
/// or `:` round-trips through the URL as `%40`/`%3A`).
///
/// `socks4`/`socks5` URLs with no explicit port default to `1080` (the
/// conventional SOCKS port; [`url::Url::port_or_known_default`] has no
/// built-in default for these schemes).
///
/// # Errors
///
/// Returns [`ZendriverError::Navigation`] if the URL is unparseable or is
/// missing a host or port. Error messages redact any userinfo from `url` so
/// a malformed proxy URL never leaks its password into logs or errors.
pub(crate) fn split_proxy_url(url: &str) -> Result<ParsedProxy, ZendriverError> {
    let u = url::Url::parse(url).map_err(|e| {
        ZendriverError::Navigation(format!("invalid proxy URL {:?}: {e}", redact_userinfo(url)))
    })?;
    let host = u.host_str().filter(|h| !h.is_empty()).ok_or_else(|| {
        ZendriverError::Navigation(format!("proxy URL {:?} missing host", redact_userinfo(url)))
    })?;
    let port = match u.port_or_known_default() {
        Some(port) => port,
        // `url` only knows well-known ports for schemes it recognizes as
        // "special" (http/https/ws/wss/ftp/file); socks4/socks5 fall through
        // to `None` even though SOCKS has a de facto standard port.
        None if matches!(u.scheme(), "socks4" | "socks5") => 1080,
        None => {
            return Err(ZendriverError::Navigation(format!(
                "proxy URL {:?} missing port",
                redact_userinfo(url)
            )));
        }
    };
    let credentials = if u.username().is_empty() {
        None
    } else {
        Some((
            percent_decode_str(u.username())
                .decode_utf8_lossy()
                .into_owned(),
            percent_decode_str(u.password().unwrap_or_default())
                .decode_utf8_lossy()
                .into_owned(),
        ))
    };
    Ok(ParsedProxy {
        server: format!("{}://{}:{}", u.scheme(), host, port),
        credentials,
    })
}

/// Redact userinfo (`user[:pass]@`) from a proxy URL before it is embedded
/// in an error message, so a rejected/malformed proxy URL never leaks its
/// password. Deliberately string surgery rather than `url::Url`-based —
/// this must also work on URLs that failed to parse in the first place.
fn redact_userinfo(raw: &str) -> String {
    let Some(scheme_end) = raw.find("://") else {
        return raw.to_string();
    };
    let after_scheme = scheme_end + 3;
    // The authority (userinfo + host + port) ends at the first `/`, `?`, or
    // `#` — don't let an `@` in the path/query masquerade as userinfo.
    let authority_end = raw[after_scheme..]
        .find(['/', '?', '#'])
        .map_or(raw.len(), |i| after_scheme + i);
    let authority = &raw[after_scheme..authority_end];
    match authority.rfind('@') {
        Some(at) => format!(
            "{}***@{}{}",
            &raw[..after_scheme],
            &authority[at + 1..],
            &raw[authority_end..]
        ),
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_userinfo_from_server() {
        let p = split_proxy_url("http://bob:s3cret@proxy.example:8080").unwrap();
        assert_eq!(p.server, "http://proxy.example:8080");
        assert_eq!(p.credentials, Some(("bob".into(), "s3cret".into())));
    }

    #[test]
    fn no_userinfo_yields_no_credentials() {
        let p = split_proxy_url("http://proxy.example:8080").unwrap();
        assert_eq!(p.server, "http://proxy.example:8080");
        assert_eq!(p.credentials, None);
    }

    #[test]
    fn fills_default_port_from_scheme() {
        let p = split_proxy_url("http://proxy.example").unwrap();
        assert_eq!(p.server, "http://proxy.example:80");
    }

    #[test]
    fn username_without_password_yields_empty_pass() {
        let p = split_proxy_url("http://bob@proxy.example:8080").unwrap();
        assert_eq!(p.credentials, Some(("bob".into(), String::new())));
    }

    #[test]
    fn missing_host_is_an_error() {
        assert!(split_proxy_url("http://").is_err());
    }

    #[test]
    fn unparseable_is_an_error() {
        assert!(split_proxy_url("not a url").is_err());
    }

    #[test]
    fn splits_percent_encoded_userinfo() {
        let p = split_proxy_url("http://bob:p%40ss%3Aword@proxy.example:8080").unwrap();
        assert_eq!(p.server, "http://proxy.example:8080");
        assert_eq!(p.credentials, Some(("bob".into(), "p@ss:word".into())));
    }

    #[test]
    fn socks5_defaults_to_1080() {
        let p = split_proxy_url("socks5://host").unwrap();
        assert_eq!(p.server, "socks5://host:1080");
    }

    #[test]
    fn socks4_defaults_to_1080() {
        let p = split_proxy_url("socks4://host").unwrap();
        assert_eq!(p.server, "socks4://host:1080");
    }

    #[test]
    fn explicit_port_overrides_socks_default() {
        let p = split_proxy_url("socks5://host:9999").unwrap();
        assert_eq!(p.server, "socks5://host:9999");
    }

    #[test]
    fn error_messages_never_contain_the_password() {
        // Missing host, but with userinfo carrying a password.
        let err = split_proxy_url("http://bob:s3cret@").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("s3cret"), "error leaked password: {msg}");

        // Missing port on a non-SOCKS scheme that has no known default.
        let err = split_proxy_url("ftp2://bob:s3cret@host").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("s3cret"), "error leaked password: {msg}");
    }

    #[test]
    fn redact_userinfo_masks_credentials_but_keeps_host() {
        assert_eq!(
            redact_userinfo("http://bob:s3cret@proxy.example:8080/path?q=1"),
            "http://***@proxy.example:8080/path?q=1"
        );
        assert_eq!(
            redact_userinfo("http://proxy.example:8080"),
            "http://proxy.example:8080"
        );
        assert_eq!(redact_userinfo("not a url"), "not a url");
    }
}
