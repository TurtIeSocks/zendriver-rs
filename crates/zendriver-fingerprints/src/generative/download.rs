//! Generative-local download-on-first-use + cache (mirrors the pool's pattern;
//! kept local so the pool's currently-untested download path is not perturbed).

use std::fs;
use std::path::{Path, PathBuf};

use super::GenError;

/// Cache location for the network ZIP — same root as the pool / fetcher.
pub(super) fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zendriver/fingerprints/fingerprint-network.zip")
}

/// Read `cache` if present and non-empty, else GET `url` and atomically cache it.
pub(super) async fn fetch_or_cached_bytes(url: &str, cache: &Path) -> Result<Vec<u8>, GenError> {
    if let Ok(bytes) = fs::read(cache) {
        if !bytes.is_empty() {
            tracing::debug!(path = %cache.display(), "fp network cache hit");
            return Ok(bytes);
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

        let first = fetch_or_cached_bytes(&server.uri(), &cache).await.unwrap();
        let second = fetch_or_cached_bytes(&server.uri(), &cache).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first, super::super::TEST_NETWORK_ZIP);
        // `.expect(1)` is verified on server drop.
    }
}
