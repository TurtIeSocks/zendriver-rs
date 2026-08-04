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

    #[tokio::test]
    async fn resolves_country_from_ipapi_json() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_string(r#"{"countryCode":"DE"}"#),
            )
            .mount(&server)
            .await;
        let r = IpApiResolver::new().endpoint(server.uri());
        assert_eq!(
            r.resolve().await,
            Some(ResolvedGeo {
                country: Country::try_from("DE").unwrap(),
                timezone: None,
            })
        );
    }

    /// A body carrying ip-api's `timezone` field must thread it through as
    /// the EXACT probe timezone, not just the country.
    #[tokio::test]
    async fn resolves_exact_timezone_from_ipapi_json() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(r#"{"countryCode":"US","timezone":"America/Los_Angeles"}"#),
            )
            .mount(&server)
            .await;
        let r = IpApiResolver::new().endpoint(server.uri());
        assert_eq!(
            r.resolve().await,
            Some(ResolvedGeo {
                country: Country::try_from("US").unwrap(),
                timezone: Some("America/Los_Angeles".to_string()),
            })
        );
    }

    /// A body with `countryCode` but no `timezone` field must yield
    /// `timezone: None` (falls back to the country-representative zone
    /// downstream), not an error.
    #[tokio::test]
    async fn missing_timezone_field_yields_none_timezone() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_string(r#"{"countryCode":"US"}"#),
            )
            .mount(&server)
            .await;
        let r = IpApiResolver::new().endpoint(server.uri());
        let resolved = r.resolve().await.unwrap();
        assert_eq!(resolved.country, Country::try_from("US").unwrap());
        assert_eq!(resolved.timezone, None);
    }

    /// Well-formed JSON that simply has no `countryCode` (ip-api answers
    /// `{"status":"fail",...}` for a reserved-range IP) is a probe FAILURE,
    /// not geo data. It yields `None` — and, unlike before, says so in the
    /// log rather than downgrading the persona in silence.
    #[tokio::test]
    async fn missing_country_code_yields_none() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(r#"{"status":"fail","message":"reserved range"}"#),
            )
            .mount(&server)
            .await;
        assert_eq!(
            IpApiResolver::new().endpoint(server.uri()).resolve().await,
            None
        );
    }

    /// A non-2xx answer whose body is not JSON at all (a proxy's HTML error
    /// page is the realistic case) is likewise a logged failure, not data.
    #[tokio::test]
    async fn non_json_error_page_yields_none() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(502)
                    .set_body_string("<html><body>Bad Gateway</body></html>"),
            )
            .mount(&server)
            .await;
        assert_eq!(
            IpApiResolver::new().endpoint(server.uri()).resolve().await,
            None
        );
    }

    #[tokio::test]
    async fn bad_body_yields_none() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("nope"))
            .mount(&server)
            .await;
        assert_eq!(
            IpApiResolver::new().endpoint(server.uri()).resolve().await,
            None
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
        assert_eq!(
            r.resolve().await.map(|g| g.country),
            Some(Country::try_from("DE").unwrap())
        );
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
}
