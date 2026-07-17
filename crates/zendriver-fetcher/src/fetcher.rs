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
use crate::manifest::{fetch_channels_manifest_from, fetch_manifest_from};
use crate::platform::Platform;
use crate::resolver::{resolve_channel_download_url, resolve_download_url};
use crate::version::{Channel, VersionSpec};
use crate::{FetcherPhase, FetcherProgress};

/// Canonical Chrome for Testing manifest URL — flat version history,
/// stable channel only.
pub(crate) const DEFAULT_CFT_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json";

/// Canonical Chrome for Testing per-channel manifest URL — used to resolve
/// [`VersionSpec::Channel`] for the `Beta`/`Dev`/`Canary` channels (`Stable`
/// resolves through [`DEFAULT_CFT_URL`] like [`VersionSpec::Latest`]).
pub(crate) const DEFAULT_CFT_CHANNELS_URL: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

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
    /// Optional SHA256 the downloaded archive must match before extraction.
    /// The CfT manifest does not publish per-download hashes, so verifying
    /// integrity is opt-in: callers pinning a known-good build (e.g. CI on
    /// a frozen Chrome major) supply the expected hash via
    /// [`Fetcher::expected_sha256`]. When `None` the check is skipped.
    expected_sha256: Option<String>,
}

impl std::fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetcher")
            .field("cache_dir", &self.cache_dir)
            .field("version", &self.version)
            .field("platform", &self.platform)
            .field("manifest_url", &self.manifest_url)
            .field("progress_cb", &self.progress_cb.as_ref().map(|_| "..."))
            .field("expected_sha256", &self.expected_sha256)
            .finish()
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher {
    /// Construct a new fetcher with default options:
    ///
    /// - cache dir = OS cache dir (`$XDG_CACHE_HOME/zendriver/chrome` on
    ///   Linux, `~/Library/Caches/zendriver/chrome` on macOS, ...);
    /// - platform = auto-detected via [`Platform::auto_detect`];
    /// - version = [`VersionSpec::Latest`].
    ///
    /// ```
    /// use zendriver_fetcher::Fetcher;
    /// let _fetcher = Fetcher::new();
    /// ```
    pub fn new() -> Self {
        Self {
            cache_dir: None,
            version: VersionSpec::Latest,
            platform: None,
            manifest_url: None,
            progress_cb: None,
            expected_sha256: None,
        }
    }

    /// Override the cache directory root.
    ///
    /// Useful for CI runs that mount a shared persistent volume — point the
    /// fetcher at it and a single download serves every job.
    pub fn cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// Pick a specific version selector. Defaults to [`VersionSpec::Latest`].
    pub fn version(mut self, spec: VersionSpec) -> Self {
        self.version = spec;
        self
    }

    /// Override the target platform, skipping [`Platform::auto_detect`].
    pub fn platform(mut self, p: Platform) -> Self {
        self.platform = Some(p);
        self
    }

    /// Register a progress callback fired during download + at key phase
    /// transitions.
    ///
    /// The callback receives a [`FetcherProgress`] snapshot with the current
    /// [`FetcherPhase`]; it's called from `tokio` worker threads so any heavy
    /// work should `spawn_blocking` itself off the runtime.
    pub fn on_progress(mut self, cb: impl Fn(FetcherProgress) + Send + Sync + 'static) -> Self {
        self.progress_cb = Some(Arc::new(cb));
        self
    }

    /// Override the CFT manifest URL. Test seam — `#[doc(hidden)]` so users
    /// don't accidentally point at a fork.
    ///
    /// Used for whichever manifest [`Fetcher::version`] resolves against:
    /// the flat `known-good-versions-with-downloads.json` shape for
    /// [`VersionSpec::Latest`]/[`VersionSpec::Stable`]/
    /// [`VersionSpec::Explicit`]/[`VersionSpec::Channel(Channel::Stable)`](VersionSpec::Channel),
    /// or the per-channel `last-known-good-versions-with-downloads.json`
    /// shape for [`VersionSpec::Channel`]'s `Beta`/`Dev`/`Canary` variants.
    #[doc(hidden)]
    pub fn manifest_url(mut self, url: impl Into<String>) -> Self {
        self.manifest_url = Some(url.into());
        self
    }

    /// Verify the downloaded archive against `sha256` (lowercase hex) before
    /// extracting. If the hash does not match,
    /// [`FetcherError::IntegrityFailed`] is returned and the tmp file is
    /// cleaned up — no extraction occurs.
    ///
    /// The CfT manifest does not publish per-download hashes, so this is an
    /// opt-in check for callers that pin a specific build and want to
    /// reject CDN tampering or transit corruption.
    ///
    /// ```no_run
    /// # async fn ex() -> Result<(), zendriver_fetcher::FetcherError> {
    /// use zendriver_fetcher::{Fetcher, VersionSpec};
    /// let _ = Fetcher::new()
    ///     .version(VersionSpec::Explicit("126.0.6478.182".into()))
    ///     .expected_sha256("0123abcd...")
    ///     .ensure_chrome()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn expected_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256.into());
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
    ///
    /// ```no_run
    /// # async fn ex() -> Result<(), zendriver_fetcher::FetcherError> {
    /// use zendriver_fetcher::{Fetcher, Platform, VersionSpec};
    ///
    /// let path = Fetcher::new()
    ///     .platform(Platform::MacArm64)
    ///     .version(VersionSpec::Explicit("126.0.6478.182".into()))
    ///     .ensure_chrome()
    ///     .await?;
    /// println!("{}", path.display());
    /// # Ok(()) }
    /// ```
    pub async fn ensure_chrome(self) -> Result<PathBuf, FetcherError> {
        let cache_dir = self.cache_dir.unwrap_or_else(default_cache_dir);
        let platform = self
            .platform
            .or_else(Platform::auto_detect)
            .ok_or(FetcherError::UnsupportedPlatform)?;
        let progress_cb = self.progress_cb;

        // Phase 1: resolve. `Beta`/`Dev`/`Canary` track their own latest
        // known-good build, so they resolve through the per-channel
        // manifest instead of the flat stable-only one; every other spec
        // (including `Channel::Stable`) uses the flat manifest.
        emit(&progress_cb, FetcherPhase::Resolving, 0, None);
        let (version, url) = match self.version {
            VersionSpec::Channel(channel @ (Channel::Beta | Channel::Dev | Channel::Canary)) => {
                let manifest_url = self
                    .manifest_url
                    .unwrap_or_else(|| DEFAULT_CFT_CHANNELS_URL.into());
                let manifest = fetch_channels_manifest_from(&manifest_url).await?;
                resolve_channel_download_url(&manifest, channel, platform).await?
            }
            ref spec => {
                let manifest_url = self.manifest_url.unwrap_or_else(|| DEFAULT_CFT_URL.into());
                let manifest = fetch_manifest_from(&manifest_url).await?;
                resolve_download_url(&manifest, spec, platform).await?
            }
        };

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

        // Phase 2b: optional SHA256 integrity check before we trust the
        // archive enough to extract it.
        if let Some(expected) = self.expected_sha256.as_deref() {
            emit(&progress_cb, FetcherPhase::Verifying, 0, None);
            let actual = sha256_file(&tmp_zip).await?;
            if !sha256_eq(expected, &actual) {
                let _ = tokio::fs::remove_file(&tmp_zip).await;
                return Err(FetcherError::IntegrityFailed {
                    expected: expected.to_string(),
                    actual,
                });
            }
        }

        // Phase 3: extract to <version>.tmp/.
        emit(&progress_cb, FetcherPhase::Extracting, 0, None);
        tokio::fs::create_dir_all(&tmp_dir).await?;
        // CfT archives place everything under a single `chrome-<platform>/`
        // directory; pinning the expected prefix locks the extraction to
        // that layout and rejects mislabeled / tampered archives.
        let expected_top = platform.cft_top_dir();
        let extract_result = extract(&tmp_zip, &tmp_dir, Some(expected_top)).await;

        // Always clean the zip, even if extract failed.
        let _ = tokio::fs::remove_file(&tmp_zip).await;
        extract_result?;

        // Phase 4: ensure executable bit (Unix) BEFORE the atomic rename.
        // Setting perms after the rename would leave a window where a
        // concurrent `Fetcher::ensure(...)` on the same cache_dir observes
        // `final_dir` (cache-hit check at L159) but the binary inside is
        // still non-executable, forcing a wasteful re-download.
        emit(&progress_cb, FetcherPhase::Verifying, 0, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let tmp_bin = tmp_dir.join(crate::cache::binary_subpath(platform));
            if tmp_bin.exists() {
                let mut perms = tokio::fs::metadata(&tmp_bin).await?.permissions();
                perms.set_mode(perms.mode() | 0o111);
                tokio::fs::set_permissions(&tmp_bin, perms).await?;
            }
        }

        // Phase 5: atomic promote <version>.tmp -> <version>.
        // If `final_dir` already exists (race: someone else just finished),
        // drop our work and use theirs.
        match tokio::fs::rename(&tmp_dir, &final_dir).await {
            Ok(()) => {}
            Err(_) if final_dir.exists() => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            }
            Err(e) => return Err(FetcherError::Io(e)),
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

/// Compute the lowercase-hex SHA256 of `path`'s contents. The hash is
/// computed on a `spawn_blocking` thread so a multi-hundred-MB Chrome zip
/// doesn't block the runtime.
async fn sha256_file(path: &std::path::Path) -> Result<String, FetcherError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use sha2::Digest as _;
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = sha2::Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hex_encode(&hasher.finalize()))
    })
    .await
    .map_err(|e| FetcherError::Extraction(format!("sha256 join error: {e}")))?
    .map_err(FetcherError::Io)
}

/// Case-insensitive hex compare. Tolerates either casing on the
/// caller-supplied expected hash.
fn sha256_eq(expected: &str, actual: &str) -> bool {
    expected.eq_ignore_ascii_case(actual)
}

/// Encode bytes as lowercase hex. Inlined to avoid pulling in the `hex`
/// crate for one helper.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
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

    /// With a matching expected SHA256, ensure_chrome should still succeed
    /// end-to-end.
    #[tokio::test]
    async fn ensure_chrome_passes_when_expected_sha256_matches() {
        let server = MockServer::start().await;
        let sentinel = b"hello-cft\n";
        let zip_bytes = build_stub_chrome_zip(sentinel);
        let zip_hash = {
            use sha2::Digest as _;
            let mut h = sha2::Sha256::new();
            h.update(&zip_bytes);
            hex_encode(&h.finalize())
        };

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
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Latest)
            .manifest_url(format!("{}/manifest.json", server.uri()))
            .expected_sha256(zip_hash)
            .ensure_chrome()
            .await
            .unwrap();
        assert!(bin_path.exists());
    }

    /// With a wrong expected SHA256, ensure_chrome surfaces
    /// `IntegrityFailed` and cleans up the tmp zip before any extraction
    /// touches the cache.
    #[tokio::test]
    async fn ensure_chrome_rejects_mismatched_expected_sha256() {
        let server = MockServer::start().await;
        let sentinel = b"hello-cft\n";
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
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let err = Fetcher::new()
            .cache_dir(cache_root.path())
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Latest)
            .manifest_url(format!("{}/manifest.json", server.uri()))
            .expected_sha256("0".repeat(64))
            .ensure_chrome()
            .await
            .unwrap_err();

        assert!(matches!(err, FetcherError::IntegrityFailed { .. }));
        // Tmp zip cleaned up; extraction never ran so no version dir.
        assert!(!cache_root.path().join("120.0.6099.234.tmp.zip").exists());
        assert!(!cache_root.path().join("120.0.6099.234").exists());
    }

    /// End-to-end for a non-stable channel: `VersionSpec::Channel(Channel::Beta)`
    /// should resolve through the per-channel manifest shape (`channels: {
    /// "Beta": {...} }`) — not the flat `versions: [...]` manifest — then
    /// download + extract exactly like the `Latest`/`Stable` path.
    #[tokio::test]
    async fn ensure_chrome_resolves_beta_channel_from_channels_manifest() {
        let server = MockServer::start().await;
        let sentinel = b"#!/bin/sh\necho stub-chrome-beta\n";
        let zip_bytes = build_stub_chrome_zip(sentinel);

        let channels_json = format!(
            r#"{{
                "timestamp": "2026-07-16T00:00:00.000Z",
                "channels": {{
                    "Beta": {{
                        "channel": "Beta",
                        "version": "121.0.6100.10",
                        "revision": "1235",
                        "downloads": {{
                            "chrome": [
                                {{"platform": "linux64", "url": "{server}/chrome-beta.zip"}}
                            ]
                        }}
                    }}
                }}
            }}"#,
            server = server.uri()
        );

        Mock::given(method("GET"))
            .and(path("/channels.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(channels_json))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/chrome-beta.zip"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", zip_bytes.len().to_string().as_str())
                    .set_body_bytes(zip_bytes),
            )
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let channels_url = format!("{}/channels.json", server.uri());

        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Channel(Channel::Beta))
            .manifest_url(&channels_url)
            .ensure_chrome()
            .await
            .unwrap();

        assert_eq!(
            bin_path,
            cache_root
                .path()
                .join("121.0.6100.10/chrome-linux64/chrome")
        );
        let extracted = tokio::fs::read(&bin_path).await.unwrap();
        assert_eq!(extracted, sentinel);
    }
}
