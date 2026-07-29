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
