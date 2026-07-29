//! Process-wide rustls crypto provider.
//!
//! reqwest 0.13's `rustls-no-provider` installs no provider, so one must be named before any
//! `Client` is built. Note "before any Client is built", not "before any HTTPS request": reqwest
//! constructs its TLS config when the client is created, so even a plain-HTTP request through a
//! fresh client panics without a provider.
//!
//! This crate keeps its own copy because it depends on neither `zendriver` nor
//! `zendriver-fetcher`. All copies are `Once`-guarded and the first caller wins.

use std::sync::Once;

/// Install the ring crypto provider as the process default, once.
pub fn install_default_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent() {
        install_default_crypto_provider();
        install_default_crypto_provider();
    }

    /// Building a TLS config panics without a provider, so this is the regression guard the
    /// compiler cannot give us.
    #[test]
    fn tls_config_builds_after_install() {
        install_default_crypto_provider();
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }
}
