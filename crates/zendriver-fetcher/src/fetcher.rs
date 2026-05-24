//! Public [`Fetcher`] entry point.
//!
//! Resolves a `(version, platform)` pair against the Chrome for Testing
//! manifest, downloads the zip into a per-version atomic cache layout,
//! extracts it, and hands back a path to the executable. If the binary is
//! already cached and runnable, returns the path immediately.

use std::path::PathBuf;
use std::sync::Arc;

use crate::cache::{binary_path, default_cache_dir};
use crate::download::download;
use crate::error::FetcherError;
use crate::extract::extract;
use crate::manifest::fetch_manifest_from;
use crate::platform::Platform;
use crate::resolver::resolve_download_url;
use crate::version::VersionSpec;
use crate::{FetcherPhase, FetcherProgress};

/// Canonical Chrome for Testing manifest URL.
pub(crate) const DEFAULT_CFT_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json";

/// Chrome for Testing binary downloader.
///
/// Build with [`Fetcher::new`], optionally configure cache dir / version /
/// platform / progress callback, then call [`Fetcher::ensure_chrome`] to
/// resolve the path to a runnable Chrome binary (downloading + extracting
/// on cache miss).
pub struct Fetcher {
    cache_dir: Option<PathBuf>,
    version: VersionSpec,
    platform: Option<Platform>,
    /// Override for the CFT manifest URL — `#[doc(hidden)]` test seam.
    manifest_url: Option<String>,
    progress_cb: Option<Arc<dyn Fn(FetcherProgress) + Send + Sync>>,
}

impl std::fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetcher")
            .field("cache_dir", &self.cache_dir)
            .field("version", &self.version)
            .field("platform", &self.platform)
            .field("manifest_url", &self.manifest_url)
            .field("progress_cb", &self.progress_cb.as_ref().map(|_| "..."))
            .finish()
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher {
    /// Construct a new fetcher with default options
    /// (auto-detected platform, OS cache dir, latest version).
    pub fn new() -> Self {
        Self {
            cache_dir: None,
            version: VersionSpec::Latest,
            platform: None,
            manifest_url: None,
            progress_cb: None,
        }
    }

    /// Override the cache directory root.
    pub fn cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// Pick a specific version selector.
    pub fn version(mut self, spec: VersionSpec) -> Self {
        self.version = spec;
        self
    }

    /// Override the target platform (skips auto-detection).
    pub fn platform(mut self, p: Platform) -> Self {
        self.platform = Some(p);
        self
    }

    /// Register a progress callback fired during download + key phase transitions.
    pub fn on_progress(mut self, cb: impl Fn(FetcherProgress) + Send + Sync + 'static) -> Self {
        self.progress_cb = Some(Arc::new(cb));
        self
    }

    /// Override the CFT manifest URL. Test seam — `#[doc(hidden)]` so users
    /// don't accidentally point at a fork.
    #[doc(hidden)]
    pub fn manifest_url(mut self, url: impl Into<String>) -> Self {
        self.manifest_url = Some(url.into());
        self
    }

    /// Resolve, download, extract (on cache miss), and return the path
    /// to the cached Chrome binary.
    ///
    /// On a cache hit, returns immediately without touching the network.
    ///
    /// # Errors
    ///
    /// See [`FetcherError`].
    pub async fn ensure_chrome(self) -> Result<PathBuf, FetcherError> {
        let cache_dir = self.cache_dir.unwrap_or_else(default_cache_dir);
        let platform = self
            .platform
            .or_else(Platform::auto_detect)
            .ok_or(FetcherError::UnsupportedPlatform)?;
        let progress_cb = self.progress_cb;
        let manifest_url = self.manifest_url.unwrap_or_else(|| DEFAULT_CFT_URL.into());

        // Phase 1: resolve.
        emit(&progress_cb, FetcherPhase::Resolving, 0, None);
        let manifest = fetch_manifest_from(&manifest_url).await?;
        let (version, url) = resolve_download_url(&manifest, &self.version, platform).await?;

        // Compute final layout + cache hit check.
        tokio::fs::create_dir_all(&cache_dir).await?;
        let final_dir = cache_dir.join(&version);
        let target_bin = binary_path(&cache_dir, &version, platform);
        if is_runnable(&target_bin).await {
            emit(&progress_cb, FetcherPhase::Done, 0, None);
            return Ok(target_bin);
        }

        // Phase 2: download to <version>.tmp.zip.
        let tmp_zip = cache_dir.join(format!("{version}.tmp.zip"));
        let tmp_dir = cache_dir.join(format!("{version}.tmp"));

        // Clean up any stale tmp from a prior crashed run.
        let _ = tokio::fs::remove_file(&tmp_zip).await;
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

        emit(&progress_cb, FetcherPhase::Downloading, 0, None);
        download(&url, &tmp_zip, progress_cb.as_deref().map(|a| a as _)).await?;

        // Phase 3: extract to <version>.tmp/.
        emit(&progress_cb, FetcherPhase::Extracting, 0, None);
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let extract_result = extract(&tmp_zip, &tmp_dir).await;

        // Always clean the zip, even if extract failed.
        let _ = tokio::fs::remove_file(&tmp_zip).await;
        extract_result?;

        // Phase 4: atomic promote <version>.tmp -> <version>.
        // If `final_dir` already exists (race: someone else just finished),
        // drop our work and use theirs.
        match tokio::fs::rename(&tmp_dir, &final_dir).await {
            Ok(()) => {}
            Err(_) if final_dir.exists() => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            }
            Err(e) => return Err(FetcherError::Io(e)),
        }

        // Phase 5: ensure executable bit (Unix).
        emit(&progress_cb, FetcherPhase::Verifying, 0, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if target_bin.exists() {
                let mut perms = tokio::fs::metadata(&target_bin).await?.permissions();
                perms.set_mode(perms.mode() | 0o111);
                tokio::fs::set_permissions(&target_bin, perms).await?;
            }
        }

        emit(&progress_cb, FetcherPhase::Done, 0, None);
        Ok(target_bin)
    }
}

fn emit(
    cb: &Option<Arc<dyn Fn(FetcherProgress) + Send + Sync>>,
    phase: FetcherPhase,
    downloaded: u64,
    total: Option<u64>,
) {
    if let Some(cb) = cb {
        cb(FetcherProgress {
            downloaded,
            total,
            phase,
        });
    }
}

/// True iff `path` exists and (on Unix) has any executable bit set.
async fn is_runnable(path: &std::path::Path) -> bool {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a tiny zip in-memory containing `chrome-linux64/chrome` with
    /// the given sentinel content. Returns the raw zip bytes.
    fn build_stub_chrome_zip(sentinel: &[u8]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            // Use unix mode 0o755 so the extracted file is already
            // executable — matches the real CFT zip layout.
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            writer.start_file("chrome-linux64/chrome", opts).unwrap();
            writer.write_all(sentinel).unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[tokio::test]
    async fn ensure_chrome_end_to_end_with_stub_manifest_and_zip() {
        let server = MockServer::start().await;
        let sentinel = b"#!/bin/sh\necho stub-chrome\n";
        let zip_bytes = build_stub_chrome_zip(sentinel);

        let manifest_json = format!(
            r#"{{"versions":[{{"version":"120.0.6099.234","revision":"1234","downloads":{{"chrome":[{{"platform":"linux64","url":"{server}/chrome.zip"}}]}}}}]}}"#,
            server = server.uri()
        );

        Mock::given(method("GET"))
            .and(path("/manifest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest_json))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/chrome.zip"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", zip_bytes.len().to_string().as_str())
                    .set_body_bytes(zip_bytes),
            )
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let manifest_url = format!("{}/manifest.json", server.uri());

        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Latest)
            .manifest_url(&manifest_url)
            .ensure_chrome()
            .await
            .unwrap();

        // Path matches the CFT layout.
        assert_eq!(
            bin_path,
            cache_root
                .path()
                .join("120.0.6099.234/chrome-linux64/chrome")
        );

        // Binary exists with the sentinel content.
        let extracted = tokio::fs::read(&bin_path).await.unwrap();
        assert_eq!(extracted, sentinel);

        // No leftover tmp artifacts.
        assert!(!cache_root.path().join("120.0.6099.234.tmp.zip").exists());
        assert!(!cache_root.path().join("120.0.6099.234.tmp").exists());

        // Executable bit set on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let meta = tokio::fs::metadata(&bin_path).await.unwrap();
            assert!(meta.permissions().mode() & 0o111 != 0);
        }
    }
}
