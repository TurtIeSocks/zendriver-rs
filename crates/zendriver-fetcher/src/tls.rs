//! Process-wide rustls crypto provider, and the one HTTP entry point that guarantees it is set.
//!
//! reqwest 0.12 chose a provider for you as a side effect of the feature name: `rustls-tls`
//! expanded to `__rustls-ring`. reqwest 0.13 stopped guessing. Its `rustls` feature bundles
//! aws-lc-rs, whose `-sys` C/asm crate breaks musl cross-compilation, so this workspace uses
//! `rustls-no-provider` and names the provider itself.
//!
//! The failure mode that shape creates is a **runtime panic** — rustls aborts with "no
//! process-level CryptoProvider available" the first time it builds a TLS config — not a compile
//! error. A missed call site therefore ships. That is why every request in this crate goes through
//! [`get`] rather than calling `reqwest::get` directly: there is one door, and it installs the
//! provider before opening.

use std::sync::Once;

/// Install the ring crypto provider as the process default, once.
///
/// Safe to call from anywhere, any number of times, and from more than one crate: the first caller
/// wins and later ones are a no-op. `install_default` returns `Err` when a provider is already
/// installed, which is exactly the "someone else got here first" case and is not an error for us.
pub fn install_default_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// `reqwest::get`, with the crypto provider guaranteed installed first.
pub(crate) async fn get(url: &str) -> Result<reqwest::Response, reqwest::Error> {
    install_default_crypto_provider();
    reqwest::get(url).await
}

/// User-Agent sent to `api.github.com`.
///
/// Not cosmetic: GitHub's API answers **403** to any request without a
/// User-Agent, which reads exactly like a rate-limit rejection and sends you
/// hunting for a token you do not need.
const GITHUB_USER_AGENT: &str = concat!("zendriver-fetcher/", env!("CARGO_PKG_VERSION"));

/// GET a JSON document from GitHub's REST API.
///
/// Sends the required User-Agent, pins the `2022-11-28` API version, and
/// attaches `GITHUB_TOKEN` as a bearer token when the environment has one.
/// A rejection that looks like rate limiting becomes
/// [`FetcherError::GitHubRateLimited`](crate::FetcherError::GitHubRateLimited),
/// whose message names the 60/hour unauthenticated ceiling and the variable
/// that lifts it — a bare `403` would leave the caller guessing.
pub(crate) async fn get_github(url: &str) -> Result<String, crate::error::FetcherError> {
    install_default_crypto_provider();

    let mut req = reqwest::Client::builder()
        .user_agent(GITHUB_USER_AGENT)
        .build()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.trim().is_empty()
    {
        req = req.bearer_auth(token.trim());
    }

    let resp = req.send().await?;

    if is_rate_limited(resp.status(), resp.headers()) {
        return Err(crate::error::FetcherError::GitHubRateLimited {
            reset_hint: reset_hint(resp.headers()),
        });
    }

    Ok(resp.error_for_status()?.text().await?)
}

/// True when a GitHub response is a rate-limit rejection.
///
/// GitHub signals the primary limit with `403` + `x-ratelimit-remaining: 0`,
/// and secondary/abuse limits with `429`. A `403` with budget left over is
/// something else (a bad token, say) and is left to `error_for_status`.
fn is_rate_limited(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) -> bool {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return true;
    }
    if status != reqwest::StatusCode::FORBIDDEN {
        return false;
    }
    headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|remaining| remaining == 0)
}

/// `", resets at <unix timestamp>"` when GitHub told us when the window rolls
/// over, otherwise an empty string.
fn reset_hint(headers: &reqwest::header::HeaderMap) -> String {
    headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .map(|ts| format!(", resets at {}", ts.trim()))
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The guard is idempotent — a second call must not panic or poison the `Once`.
    #[test]
    fn install_is_idempotent() {
        install_default_crypto_provider();
        install_default_crypto_provider();
        install_default_crypto_provider();
    }

    /// A TLS config must be constructible after installing. This is the assertion that would have
    /// caught a missing provider: without one, `builder()` panics rather than failing to compile.
    #[test]
    fn tls_config_builds_after_install() {
        install_default_crypto_provider();
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn forbidden_with_no_budget_left_is_rate_limiting() {
        assert!(is_rate_limited(
            reqwest::StatusCode::FORBIDDEN,
            &headers(&[("x-ratelimit-remaining", "0")]),
        ));
    }

    /// A 403 that still has budget is a different problem (bad token, blocked
    /// resource) and must not be reported as rate limiting.
    #[test]
    fn forbidden_with_budget_left_is_not_rate_limiting() {
        assert!(!is_rate_limited(
            reqwest::StatusCode::FORBIDDEN,
            &headers(&[("x-ratelimit-remaining", "41")]),
        ));
        assert!(!is_rate_limited(
            reqwest::StatusCode::FORBIDDEN,
            &headers(&[]),
        ));
    }

    #[test]
    fn too_many_requests_is_rate_limiting_regardless_of_headers() {
        assert!(is_rate_limited(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers(&[]),
        ));
    }

    #[test]
    fn success_is_never_rate_limiting() {
        assert!(!is_rate_limited(
            reqwest::StatusCode::OK,
            &headers(&[("x-ratelimit-remaining", "0")]),
        ));
    }

    #[test]
    fn reset_hint_is_empty_without_the_header() {
        assert_eq!(reset_hint(&headers(&[])), "");
        assert_eq!(
            reset_hint(&headers(&[("x-ratelimit-reset", "1786023295")])),
            ", resets at 1786023295"
        );
    }

    /// End-to-end proof over real HTTPS, through the same door the crate's own requests use.
    ///
    /// Ignored by default because it needs the network; the nightly ignored-tests job runs it.
    /// The offline tests above prove the provider installs, but only a real handshake proves the
    /// request path actually goes through it — a wiremock server speaks plain HTTP and would pass
    /// even with no provider installed at all.
    #[tokio::test]
    #[ignore = "network"]
    async fn https_request_succeeds_through_the_guarded_door() {
        let resp =
            get("https://googlechromelabs.github.io/chrome-for-testing/known-good-versions.json")
                .await
                .expect("HTTPS must succeed with the ring provider installed");
        assert!(resp.status().is_success(), "status: {}", resp.status());
    }
}
