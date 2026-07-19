//! Generative-local download-on-first-use + cache (mirrors the pool's pattern;
//! kept local so the pool's currently-untested download path is not perturbed).

use std::fs;
use std::path::{Path, PathBuf};

use super::GenError;
use crate::CachePolicy;

/// Cache location for the network ZIP — same root as the pool / fetcher.
pub(super) fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zendriver/fingerprints/fingerprint-network.zip")
}

/// Read `cache` if present, non-empty, and fresh under `policy`, else GET
/// `url` and atomically cache it.
///
/// `policy: CachePolicy::default()` (`ttl: None`, `force_refresh: false`)
/// reproduces the original permanent-cache behavior exactly: no mtime read at
/// all, just the same read-and-check-non-empty fast path as before.
pub(super) async fn fetch_or_cached_bytes(
    url: &str,
    cache: &Path,
    policy: CachePolicy,
) -> Result<Vec<u8>, GenError> {
    if !policy.force_refresh && !crate::cache::path_is_stale(cache, policy.ttl) {
        if let Ok(bytes) = fs::read(cache) {
            if !bytes.is_empty() {
                tracing::debug!(path = %cache.display(), "fp network cache hit");
                return Ok(bytes);
            }
        }
    }
    tracing::debug!(url, "fp network cache miss — downloading");
    let bytes = reqwest::get(url).await?.bytes().await?.to_vec();
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = cache.with_extension("tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, cache)?;
    tracing::debug!(path = %cache.display(), "fp network cached");
    Ok(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::CachePolicy;
    use std::time::Duration;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn downloads_then_serves_from_cache() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(super::super::TEST_NETWORK_ZIP))
            .expect(1) // exactly one network hit — second call must hit cache
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("net.zip");

        let first = fetch_or_cached_bytes(&server.uri(), &cache, CachePolicy::default())
            .await
            .unwrap();
        let second = fetch_or_cached_bytes(&server.uri(), &cache, CachePolicy::default())
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first, super::super::TEST_NETWORK_ZIP);
        // `.expect(1)` is verified on server drop.
    }

    /// Default policy (`ttl: None`) = permanent cache. Regression guard: an
    /// old file is still served from cache, never re-downloaded due to age.
    #[tokio::test]
    async fn default_policy_never_redownloads_even_with_old_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(super::super::TEST_NETWORK_ZIP))
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("net.zip");

        let first = fetch_or_cached_bytes(&server.uri(), &cache, CachePolicy::default())
            .await
            .unwrap();
        // Backdate the mtime far into the past — still must not re-download.
        let ancient = std::time::SystemTime::now() - Duration::from_secs(1_000_000_000);
        let file = std::fs::File::open(&cache).unwrap();
        file.set_modified(ancient).unwrap();

        let second = fetch_or_cached_bytes(&server.uri(), &cache, CachePolicy::default())
            .await
            .unwrap();
        assert_eq!(first, second);
    }

    /// `ttl: Some(Duration::ZERO)` is always-stale (simplest way to force a
    /// re-download deterministically without mtime munging): the mock must be
    /// hit twice.
    #[tokio::test]
    async fn zero_ttl_triggers_redownload_on_every_call() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(super::super::TEST_NETWORK_ZIP))
            .expect(2) // both calls must hit the network — cache never "fresh"
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("net.zip");
        let policy = CachePolicy::with_ttl(Duration::ZERO);

        fetch_or_cached_bytes(&server.uri(), &cache, policy)
            .await
            .unwrap();
        fetch_or_cached_bytes(&server.uri(), &cache, policy)
            .await
            .unwrap();
        // `.expect(2)` is verified on server drop.
    }

    /// `force_refresh: true` always re-downloads, even with `ttl: None`
    /// (which alone would mean permanent cache).
    #[tokio::test]
    async fn force_refresh_always_redownloads() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(super::super::TEST_NETWORK_ZIP))
            .expect(2)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("net.zip");

        fetch_or_cached_bytes(&server.uri(), &cache, CachePolicy::default())
            .await
            .unwrap();
        // force_refresh ignores the cache hit from the call above.
        fetch_or_cached_bytes(&server.uri(), &cache, CachePolicy::force_refresh())
            .await
            .unwrap();
        // `.expect(2)` is verified on server drop.
    }

    /// Clock-skew fail-closed, end to end: a future-dated mtime (simulating a
    /// clock jump) must still trigger a re-download, not a panic and not a
    /// stale-but-treated-as-fresh read.
    #[tokio::test]
    async fn future_mtime_clock_skew_triggers_redownload_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(super::super::TEST_NETWORK_ZIP))
            .expect(2)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("net.zip");
        let policy = CachePolicy::with_ttl(Duration::from_secs(60));

        fetch_or_cached_bytes(&server.uri(), &cache, policy)
            .await
            .unwrap();
        // Simulate clock skew: mtime jumps into the future relative to "now".
        let future = std::time::SystemTime::now() + Duration::from_secs(3600);
        let file = std::fs::File::open(&cache).unwrap();
        file.set_modified(future).unwrap();

        let second = fetch_or_cached_bytes(&server.uri(), &cache, policy).await;
        assert!(second.is_ok(), "must not panic on clock skew");
        // `.expect(2)` is verified on server drop.
    }
}
