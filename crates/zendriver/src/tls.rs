//! Process-wide rustls crypto provider.
//!
//! reqwest 0.12 chose a provider for you as a side effect of the feature name: `rustls-tls`
//! expanded to `__rustls-ring`. reqwest 0.13 stopped guessing. Its `rustls` feature bundles
//! aws-lc-rs, whose `-sys` C/asm crate breaks musl cross-compilation, so this workspace uses
//! `rustls-no-provider` and names the provider itself.
//!
//! Omitting the install is a **runtime panic** ("no process-level CryptoProvider available") the
//! first time rustls builds a TLS config, not a compile error, so it must be called before any
//! HTTPS request rather than relied on incidentally.
//!
//! `zendriver-fetcher` carries its own copy: it does not depend on this crate, and `zendriver`
//! depends on it only behind the optional `fetcher` feature. Both are `Once`-guarded and the first
//! caller wins, so whichever runs first is correct.

use std::sync::Once;

/// Install the ring crypto provider as the process default, once.
///
/// Public so an embedding application can install eagerly at startup rather than waiting for the
/// first request. Safe to call repeatedly and from multiple crates: `install_default` returns
/// `Err` when a provider is already present, which is the "someone else got here first" case and
/// is not an error.
pub fn install_default_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent() {
        install_default_crypto_provider();
        install_default_crypto_provider();
    }

    /// Without a provider installed, building a config panics. This is the check that catches a
    /// regression the compiler cannot.
    #[test]
    fn tls_config_builds_after_install() {
        install_default_crypto_provider();
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }
}
