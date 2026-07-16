//! Proxy-URL parsing shared by per-context proxy configuration.
//!
//! Chrome's `Target.createBrowserContext` `proxyServer` field ignores any
//! userinfo in the URL, so credentials are split out here for the
//! interception auth handler (`Fetch.authRequired`) and the returned
//! `server` string is userinfo-free.

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
/// # Errors
///
/// Returns [`ZendriverError::Navigation`] if the URL is unparseable or is
/// missing a host or port.
pub(crate) fn split_proxy_url(url: &str) -> Result<ParsedProxy, ZendriverError> {
    let u = url::Url::parse(url)
        .map_err(|e| ZendriverError::Navigation(format!("invalid proxy URL {url:?}: {e}")))?;
    let host = u
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| ZendriverError::Navigation(format!("proxy URL {url:?} missing host")))?;
    let port = u
        .port_or_known_default()
        .ok_or_else(|| ZendriverError::Navigation(format!("proxy URL {url:?} missing port")))?;
    let credentials = if u.username().is_empty() {
        None
    } else {
        Some((
            u.username().to_string(),
            u.password().unwrap_or_default().to_string(),
        ))
    };
    Ok(ParsedProxy {
        server: format!("{}://{}:{}", u.scheme(), host, port),
        credentials,
    })
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
}
