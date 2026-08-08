//! `IpApiResolver`: derive the exit-IP country (and exact timezone, when
//! available) via a proxied HTTP probe.

use std::time::Duration;

use async_trait::async_trait;
use zendriver_stealth::geo::{Country, GeoResolver, ResolvedGeo};

/// Resolves the apparent country — and, when present, the exact IANA
/// timezone — by querying an IP-geolocation service (default
/// `http://ip-api.com/json` — **plaintext**; the proxy operator can tamper
/// with the response in transit, so override [`Self::endpoint`] to an HTTPS
/// service if response integrity matters for your threat model) through the
/// browser's proxy. Opt-in via `BrowserBuilder::geo_auto`; endpoint
/// overridable; swap the whole thing out with a custom [`GeoResolver`].
pub struct IpApiResolver {
    endpoint: String,
    proxy: Option<String>,
    proxy_auth: Option<(String, String)>,
    timeout: Duration,
}

impl Default for IpApiResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl IpApiResolver {
    /// A resolver hitting `http://ip-api.com/json` directly (no proxy), with
    /// a 5-second timeout. Chain [`Self::endpoint`] / [`Self::timeout`] to
    /// customize; `BrowserBuilder::geo_auto` wires the proxy (and its
    /// credentials, if any) via the crate-private [`Self::with_proxy`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            endpoint: "http://ip-api.com/json".into(),
            proxy: None,
            proxy_auth: None,
            timeout: Duration::from_secs(5),
        }
    }

    /// Override the probe endpoint (default `http://ip-api.com/json`, which
    /// is plaintext HTTP — a malicious or compromised proxy operator can
    /// observe or tamper with the response since it's routed through
    /// `with_proxy`; override to an HTTPS endpoint if you need integrity).
    /// Must return a JSON body with a top-level `countryCode` string field;
    /// an optional top-level `timezone` string field (ip-api's exact IANA
    /// zone for the exit IP) is used when present, falling back to the
    /// country-representative zone otherwise.
    #[must_use]
    pub fn endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = url.into();
        self
    }

    /// Override the request timeout (default 5s).
    #[must_use]
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Route the probe through `server` (mirrors the browser's own proxy so
    /// the resolved country matches the exit IP Chrome will actually use),
    /// authenticating with `auth` (`user`, `pass`) when the proxy requires
    /// it — passed to reqwest via [`reqwest::Proxy::basic_auth`], never
    /// embedded in the proxy URL string, so it can't leak into an error
    /// `Display`.
    ///
    /// Called by `BrowserBuilder::geo_auto`, wiring `self.proxy` from
    /// [`crate::browser::BrowserBuilder::proxy`] through here. Public so
    /// callers building a custom-endpoint resolver by hand (e.g.
    /// `zendriver-mcp`'s `geo_endpoint` option) can mirror the same proxy
    /// the browser itself uses, the same way `geo_auto`'s bundled default
    /// does.
    #[must_use]
    pub fn with_proxy(mut self, server: Option<String>, auth: Option<(String, String)>) -> Self {
        self.proxy = server;
        self.proxy_auth = auth;
        self
    }
}

#[async_trait]
impl GeoResolver for IpApiResolver {
    async fn resolve(&self) -> Option<ResolvedGeo> {
        // reqwest 0.13 installs no rustls crypto provider of its own (see the workspace manifest),
        // and the omission surfaces as a runtime panic rather than a build error. Install before
        // the client exists.
        crate::tls::install_default_crypto_provider();
        let mut builder = reqwest::Client::builder().timeout(self.timeout);
        if let Some(p) = &self.proxy {
            match reqwest::Proxy::all(p) {
                Ok(mut px) => {
                    if let Some((user, pass)) = &self.proxy_auth {
                        px = px.basic_auth(user, pass);
                    }
                    builder = builder.proxy(px);
                }
                Err(e) => {
                    // `reqwest::Error`'s `Display` only ever includes the
                    // failed proxy/target URL text, never proxy credentials
                    // (those are sent via `basic_auth`, not embedded in the
                    // URL) — safe to log as-is.
                    tracing::warn!(error = %e, "geo probe: bad proxy; skipping");
                    return None;
                }
            }
        }
        // Every `None` below is a *failure*, not "this IP has no country" —
        // the caller downgrades the persona either way, so each exit says
        // which one it was.
        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "geo probe: HTTP client build failed; skipping");
                return None;
            }
        };
        let resp = match client.get(&self.endpoint).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "geo probe request failed");
                return None;
            }
        };
        let status = resp.status();
        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    status = %status,
                    endpoint = %self.endpoint,
                    "geo probe: response body was not JSON"
                );
                return None;
            }
        };
        let Some(cc) = body.get("countryCode").and_then(|v| v.as_str()) else {
            tracing::warn!(
                status = %status,
                endpoint = %self.endpoint,
                "geo probe: response has no string `countryCode` field"
            );
            return None;
        };
        let country = match Country::try_from(cc) {
            Ok(c) => c,
            Err(_) => {
                tracing::warn!(country = %cc, "geo probe: unrecognized country code");
                return None;
            }
        };
        let timezone = body
            .get("timezone")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Some(ResolvedGeo { country, timezone })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_capture::{CapturedLog, sole_warning, warnings, with_captured_logs};

    /// Stand a mock endpoint up answering `status` with `body`, run a
    /// default [`IpApiResolver`] against it, and return the resolution plus
    /// everything logged while it ran.
    ///
    /// Every probe test needs the same four lines of wiremock scaffolding;
    /// this is them, once.
    async fn probe(status: u16, body: &str) -> (Option<ResolvedGeo>, Vec<CapturedLog>) {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let resolver = IpApiResolver::new().endpoint(server.uri());
        with_captured_logs(async move { resolver.resolve().await }).await
    }

    fn country(cc: &str) -> Country {
        Country::try_from(cc).expect("test country code must be valid")
    }

    #[tokio::test]
    async fn resolves_country_from_ipapi_json() {
        let (geo, logs) = probe(200, r#"{"countryCode":"DE"}"#).await;
        assert_eq!(
            geo,
            Some(ResolvedGeo {
                country: country("DE"),
                timezone: None,
            })
        );
        assert!(
            warnings(&logs).is_empty(),
            "a probe that succeeded must warn about nothing: {:?}",
            warnings(&logs),
        );
    }

    /// A body carrying ip-api's `timezone` field must thread it through as
    /// the EXACT probe timezone, not just the country.
    #[tokio::test]
    async fn resolves_exact_timezone_from_ipapi_json() {
        let (geo, _) = probe(
            200,
            r#"{"countryCode":"US","timezone":"America/Los_Angeles"}"#,
        )
        .await;
        assert_eq!(
            geo,
            Some(ResolvedGeo {
                country: country("US"),
                timezone: Some("America/Los_Angeles".to_string()),
            })
        );
    }

    /// A body with `countryCode` but no `timezone` field must yield
    /// `timezone: None` (falls back to the country-representative zone
    /// downstream), not an error.
    #[tokio::test]
    async fn missing_timezone_field_yields_none_timezone() {
        let (geo, logs) = probe(200, r#"{"countryCode":"US"}"#).await;
        let resolved = geo.expect("a country with no timezone is still a resolution");
        assert_eq!(resolved.country, country("US"));
        assert_eq!(resolved.timezone, None);
        assert!(
            warnings(&logs).is_empty(),
            "a missing timezone is not a failure: {:?}",
            warnings(&logs),
        );
    }

    /// Well-formed JSON that simply has no `countryCode` (ip-api answers
    /// `{"status":"fail",...}` for a reserved-range IP) is a probe FAILURE,
    /// not geo data.
    ///
    /// It yielded `None` before this change too — the change is that it now
    /// says so instead of downgrading the persona in silence, so the
    /// warning is the only thing here worth asserting.
    #[tokio::test]
    async fn missing_country_code_warns_and_yields_none() {
        let (geo, logs) = probe(200, r#"{"status":"fail","message":"reserved range"}"#).await;
        assert_eq!(geo, None);
        let warning = sole_warning(&logs);
        assert!(
            warning.contains("no string `countryCode` field"),
            "unexpected warning: {warning}",
        );
    }

    /// A body that is not JSON at all is one failure with one cause,
    /// whether it arrives as a 200 or as a proxy's 502 HTML error page:
    /// same `resp.json()` code path, same warning. Both cases live here
    /// rather than in two tests that differ only in bytes neither asserts
    /// on.
    #[tokio::test]
    async fn a_non_json_body_warns_and_yields_none() {
        for (status, body) in [
            (200, "nope"),
            (502, "<html><body>Bad Gateway</body></html>"),
        ] {
            let (geo, logs) = probe(status, body).await;
            assert_eq!(geo, None, "status {status}");
            let warning = sole_warning(&logs);
            assert!(
                warning.contains("response body was not JSON"),
                "status {status}: unexpected warning: {warning}",
            );
        }
    }

    /// A `countryCode` that is not a two-letter code is a failure too, and
    /// a distinct one — the body parsed, the field was there, the value was
    /// junk. It must not be reported as a missing field.
    #[tokio::test]
    async fn an_unparseable_country_code_warns_and_yields_none() {
        let (geo, logs) = probe(200, r#"{"countryCode":"XYZ"}"#).await;
        assert_eq!(geo, None);
        let warning = sole_warning(&logs);
        assert!(
            warning.contains("unrecognized country code"),
            "unexpected warning: {warning}",
        );
    }

    /// C1: `with_proxy`'s `auth` must actually reach the underlying
    /// `reqwest::Proxy` (via `basic_auth`), not just be stored and ignored.
    /// A real authenticated-proxy wiremock (mocking the CONNECT/tunnel
    /// handshake a real forward proxy performs) is impractical with
    /// `wiremock` (it's an HTTP server, not a proxy), so this instead stands
    /// a plain wiremock server in for the proxy and drives a plain-HTTP
    /// target through it — reqwest relays plain-HTTP-through-HTTP-proxy
    /// requests as absolute-form requests directly to the proxy's socket
    /// with a `Proxy-Authorization` header when `basic_auth` was set, so the
    /// mock server IS the thing that receives (and can assert on) that
    /// header. If `with_proxy`'s `auth` were dropped (the C1 bug), the mock
    /// (which requires the header) would never match and `resolve()` would
    /// return `None`.
    #[tokio::test]
    async fn threads_proxy_credentials_into_reqwest_proxy() {
        let proxy = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::header_exists("Proxy-Authorization"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_string(r#"{"countryCode":"DE"}"#),
            )
            .mount(&proxy)
            .await;

        let r = IpApiResolver::new()
            .endpoint("http://geo-probe.invalid/json")
            .with_proxy(Some(proxy.uri()), Some(("bob".into(), "s3cret".into())));
        assert_eq!(r.resolve().await.map(|g| g.country), Some(country("DE")));
    }

    /// Without credentials, the mock (which requires `Proxy-Authorization`)
    /// must NOT match — a control proving the above test isn't a false
    /// positive from some other matcher laxity.
    #[tokio::test]
    async fn no_credentials_means_no_proxy_auth_header() {
        let proxy = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::header_exists("Proxy-Authorization"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_string(r#"{"countryCode":"DE"}"#),
            )
            .mount(&proxy)
            .await;

        let r = IpApiResolver::new()
            .endpoint("http://geo-probe.invalid/json")
            .with_proxy(Some(proxy.uri()), None);
        // No `Proxy-Authorization` header sent -> mock doesn't match -> 404
        // from wiremock -> `.json()` fails -> `resolve()` yields `None`.
        assert_eq!(r.resolve().await, None);
    }

    /// A proxy URL reqwest cannot parse is a failure before any request is
    /// made, and the warning must not carry the credentials that would have
    /// gone with it.
    #[tokio::test]
    async fn a_bad_proxy_url_warns_without_leaking_credentials() {
        let resolver = IpApiResolver::new()
            .endpoint("http://geo-probe.invalid/json")
            .with_proxy(
                Some("not a url".into()),
                Some(("bob".into(), "s3cret".into())),
            );
        let (geo, logs) = with_captured_logs(async move { resolver.resolve().await }).await;
        assert_eq!(geo, None);
        let warning = sole_warning(&logs);
        assert!(
            warning.contains("bad proxy"),
            "unexpected warning: {warning}"
        );
        assert!(
            !warning.contains("s3cret") && !warning.contains("bob"),
            "proxy credentials must never reach the log: {warning}",
        );
    }
}
