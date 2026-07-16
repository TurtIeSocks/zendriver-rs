//! `IpApiResolver`: derive the exit-IP country via a proxied HTTP probe.

use std::time::Duration;

use async_trait::async_trait;
use zendriver_stealth::geo::{Country, GeoResolver};

/// Resolves the apparent country by querying an IP-geolocation service
/// (default `ip-api.com`) through the browser's proxy. Opt-in via
/// `BrowserBuilder::geo_auto`; endpoint overridable; swap the whole thing out
/// with a custom [`GeoResolver`].
pub struct IpApiResolver {
    endpoint: String,
    proxy: Option<String>,
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
    /// customize; `BrowserBuilder::geo_auto` wires the proxy via the
    /// crate-private [`Self::with_proxy`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            endpoint: "http://ip-api.com/json".into(),
            proxy: None,
            timeout: Duration::from_secs(5),
        }
    }

    /// Override the probe endpoint (default `http://ip-api.com/json`).
    /// Must return a JSON body with a top-level `countryCode` string field.
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

    /// Route the probe through `proxy` (mirrors the browser's own proxy so
    /// the resolved country matches the exit IP Chrome will actually use).
    ///
    /// Not yet called anywhere in-tree — `BrowserBuilder::geo_auto` (a
    /// follow-up task) is the only planned caller, wiring `self.proxy` from
    /// [`crate::browser::BrowserBuilder::proxy`] through here.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn with_proxy(mut self, proxy: Option<String>) -> Self {
        self.proxy = proxy;
        self
    }
}

#[async_trait]
impl GeoResolver for IpApiResolver {
    async fn country(&self) -> Option<Country> {
        let mut builder = reqwest::Client::builder().timeout(self.timeout);
        if let Some(p) = &self.proxy {
            match reqwest::Proxy::all(p) {
                Ok(px) => builder = builder.proxy(px),
                Err(e) => {
                    tracing::warn!(error = %e, "geo probe: bad proxy; skipping");
                    return None;
                }
            }
        }
        let client = builder.build().ok()?;
        let resp = match client.get(&self.endpoint).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "geo probe request failed");
                return None;
            }
        };
        let body: serde_json::Value = resp.json().await.ok()?;
        let cc = body.get("countryCode").and_then(|v| v.as_str())?;
        match Country::try_from(cc) {
            Ok(c) => Some(c),
            Err(_) => {
                tracing::warn!(country = %cc, "geo probe: unrecognized country code");
                None
            }
        }
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
        assert_eq!(r.country().await, Some(Country::try_from("DE").unwrap()));
    }

    #[tokio::test]
    async fn bad_body_yields_none() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("nope"))
            .mount(&server)
            .await;
        assert_eq!(
            IpApiResolver::new().endpoint(server.uri()).country().await,
            None
        );
    }
}
