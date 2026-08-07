//! Public [`Fetcher`] entry point.
//!
//! Resolves a `(distribution, version, platform)` triple against that
//! distribution's index, downloads the archive into a per-build atomic cache
//! layout, unpacks it, and hands back a path to the executable. If the binary
//! is already cached and runnable, returns the path immediately.
//!
//! Only the first step varies by [`Distribution`]. Download, integrity check,
//! unpack, promote and cache-hit detection are one code path for all three —
//! see [`crate::resolver`] for the part that differs, and [`crate::archive`]
//! for the three packagings it can produce.

use std::path::PathBuf;
use std::sync::Arc;

use crate::cache::{build_dir, default_cache_dir, with_suffix};
use crate::distribution::Distribution;
use crate::download::download;
use crate::error::FetcherError;
use crate::manifest::{
    fetch_channels_manifest_from, fetch_github_releases, fetch_last_change, fetch_manifest_from,
};
use crate::platform::Platform;
use crate::resolver::{
    DEFAULT_GITHUB_API_BASE, DEFAULT_SNAPSHOT_BASE, Resolved, resolve_cft, resolve_cft_channel,
    resolve_snapshot, resolve_ungoogled, snapshot_revision_for,
};
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

/// Chromium binary downloader.
///
/// Build with [`Fetcher::new`], optionally configure distribution / cache dir
/// / version / platform / progress callback, then call
/// [`Fetcher::ensure_chrome`] to resolve the path to a runnable browser
/// binary (downloading + unpacking on cache miss).
///
/// Defaults to [`Distribution::ChromeForTesting`], which is what this crate
/// has always fetched.
pub struct Fetcher {
    cache_dir: Option<PathBuf>,
    distribution: Distribution,
    version: VersionSpec,
    platform: Option<Platform>,
    /// Override for the selected distribution's index endpoint —
    /// `#[doc(hidden)]` test seam. See [`Fetcher::manifest_url`].
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
            .field("distribution", &self.distribution)
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
    /// - distribution = [`Distribution::ChromeForTesting`];
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
            distribution: Distribution::ChromeForTesting,
            version: VersionSpec::Latest,
            platform: None,
            manifest_url: None,
            progress_cb: None,
            expected_sha256: None,
        }
    }

    /// Pick which Chromium build to fetch. Defaults to
    /// [`Distribution::ChromeForTesting`].
    ///
    /// Each distribution namespaces its own cache sub-tree, so switching does
    /// not invalidate anything already downloaded.
    ///
    /// ```no_run
    /// # async fn ex() -> Result<(), zendriver_fetcher::FetcherError> {
    /// use zendriver_fetcher::{Distribution, Fetcher, VersionSpec};
    ///
    /// let chromium = Fetcher::new()
    ///     .distribution(Distribution::UngoogledChromium)
    ///     .version(VersionSpec::Explicit("151.0.7922.71".into()))
    ///     .ensure_chrome()
    ///     .await?;
    /// # let _ = chromium; Ok(()) }
    /// ```
    pub fn distribution(mut self, distribution: Distribution) -> Self {
        self.distribution = distribution;
        self
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

    /// Override the index endpoint for the selected distribution. Test seam —
    /// `#[doc(hidden)]` so users don't accidentally point at a fork.
    ///
    /// What it overrides depends on which index resolution consults:
    ///
    /// - [`Distribution::ChromeForTesting`]: the full manifest URL — the flat
    ///   `known-good-versions-with-downloads.json` shape for
    ///   [`VersionSpec::Latest`]/[`VersionSpec::Stable`]/
    ///   [`VersionSpec::Explicit`]/[`VersionSpec::Channel(Channel::Stable)`](VersionSpec::Channel),
    ///   or the per-channel `last-known-good-versions-with-downloads.json`
    ///   shape for `Beta`/`Dev`/`Canary`.
    /// - [`Distribution::UngoogledChromium`]: the GitHub API base, to which
    ///   `/repos/<owner>/<repo>/releases` is appended.
    /// - [`Distribution::ChromiumSnapshot`]: the snapshot bucket base, to
    ///   which `/<Platform>/…` is appended.
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
        let cache_dir = self.cache_dir.clone().unwrap_or_else(default_cache_dir);
        let platform = self
            .platform
            .or_else(Platform::auto_detect)
            .ok_or(FetcherError::UnsupportedPlatform)?;
        let distribution = self.distribution;
        let progress_cb = self.progress_cb.clone();

        // Phase 1: resolve. The only step that knows about distributions.
        emit(&progress_cb, FetcherPhase::Resolving, 0, None);
        let resolved = self.resolve(platform).await?;

        // Compute final layout + cache hit check.
        let final_dir = build_dir(&cache_dir, distribution, &resolved.build_id);
        // `create_dir_all` on the parent, not the cache root: the
        // non-default distributions live one level deeper.
        if let Some(parent) = final_dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let target_bin = final_dir.join(&resolved.binary_subpath);
        if is_runnable(&target_bin).await && resolves_inside(&target_bin, &final_dir).await {
            emit(&progress_cb, FetcherPhase::Done, 0, None);
            return Ok(target_bin);
        }

        // Phase 2: download to <build>.tmp.<stamp>.<ext>, beside the build
        // directory so the staging paths are namespaced exactly like the final
        // one.
        //
        // The stamp makes staging per-FETCH, not per-build. Two `ensure_chrome`
        // calls for the same build previously derived the same `<build>.tmp`,
        // and each one's "clean up a stale tmp" step deleted the other's
        // half-written tree — after which one of them renamed whatever was left
        // into the cache under the canonical name, where the cache-hit check
        // accepts a truncated browser from then on.
        //
        // ponytail: a hard crash now leaks `<build>.tmp.<stamp>` instead of it
        // being cleared by the next run of the same build. That is the right
        // side of the trade — a leaked directory in a cache is recoverable,
        // a silently truncated browser promoted under the canonical name is
        // not. Reap by mtime age if the leakage ever actually matters.
        let stamp = staging_stamp();
        let tmp_archive = with_suffix(
            &final_dir,
            &format!(".tmp.{stamp}.{}", resolved.archive.tmp_extension()),
        );
        let tmp_dir = with_suffix(&final_dir, &format!(".tmp.{stamp}"));

        emit(&progress_cb, FetcherPhase::Downloading, 0, None);
        download(
            &resolved.url,
            &tmp_archive,
            progress_cb.as_deref().map(|a| a as _),
        )
        .await?;

        // Phase 2b: optional SHA256 integrity check before we trust the
        // archive enough to unpack it.
        if let Some(expected) = self.expected_sha256.as_deref() {
            emit(&progress_cb, FetcherPhase::Verifying, 0, None);
            let actual = sha256_file(&tmp_archive).await?;
            if !sha256_eq(expected, &actual) {
                let _ = tokio::fs::remove_file(&tmp_archive).await;
                return Err(FetcherError::IntegrityFailed {
                    expected: expected.to_string(),
                    actual,
                });
            }
        }

        // Phase 3: unpack into <build>.tmp/. Zip extraction pins the expected
        // top-level directory, rejecting mislabeled / tampered archives.
        emit(&progress_cb, FetcherPhase::Extracting, 0, None);
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let unpack_result =
            crate::archive::install(&resolved.archive, &tmp_archive, &tmp_dir).await;

        // Always clean the download, even if unpacking failed. (The
        // `Executable` archive consumes it by renaming, so this is a no-op
        // there.)
        let _ = tokio::fs::remove_file(&tmp_archive).await;
        unpack_result?;

        // Phase 4: ensure executable bit (Unix) BEFORE the atomic rename.
        // Setting perms after the rename would leave a window where a
        // concurrent `ensure_chrome` on the same cache_dir observes
        // `final_dir` (the cache-hit check above) but the binary inside is
        // still non-executable, forcing a wasteful re-download.
        emit(&progress_cb, FetcherPhase::Verifying, 0, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let tmp_bin = tmp_dir.join(&resolved.binary_subpath);
            // `metadata` and `set_permissions` both FOLLOW symlinks, so this is
            // a chmod aimed at wherever the path really lands. Extraction is
            // supposed to have guaranteed that is inside the staging dir; check
            // it here rather than inherit the guarantee, because the cost of
            // being wrong is `+x` on someone else's file.
            if tmp_bin.exists() && resolves_inside(&tmp_bin, &tmp_dir).await {
                let mut perms = tokio::fs::metadata(&tmp_bin).await?.permissions();
                perms.set_mode(perms.mode() | 0o111);
                tokio::fs::set_permissions(&tmp_bin, perms).await?;
            }
        }

        // Phase 5: atomic promote <build>.tmp -> <build>.
        // If `final_dir` already exists (race: someone else just finished),
        // drop our work and use theirs.
        match tokio::fs::rename(&tmp_dir, &final_dir).await {
            Ok(()) => {}
            Err(_) if final_dir.exists() => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            }
            Err(e) => return Err(FetcherError::Io(e)),
        }

        // The returned path is executed by the caller, so the last thing this
        // function does is confirm it still lands inside the build directory.
        if !resolves_inside(&target_bin, &final_dir).await {
            return Err(FetcherError::Extraction(format!(
                "resolved browser path {} does not lie inside {}; refusing to return it",
                target_bin.display(),
                final_dir.display()
            )));
        }

        emit(&progress_cb, FetcherPhase::Done, 0, None);
        Ok(target_bin)
    }

    /// Turn this fetcher's `(distribution, version, platform)` triple into a
    /// concrete download.
    ///
    /// Each arm fetches that distribution's index and hands it to the
    /// matching pure resolver in [`crate::resolver`].
    async fn resolve(&self, platform: Platform) -> Result<Resolved, FetcherError> {
        match self.distribution {
            Distribution::ChromeForTesting => match &self.version {
                // `Beta`/`Dev`/`Canary` track their own latest known-good
                // build, so they resolve through the per-channel manifest
                // instead of the flat stable-only one; every other spec
                // (including `Channel::Stable`) uses the flat manifest.
                VersionSpec::Channel(
                    channel @ (Channel::Beta | Channel::Dev | Channel::Canary),
                ) => {
                    let url = self.index_base(DEFAULT_CFT_CHANNELS_URL);
                    let manifest = fetch_channels_manifest_from(&url).await?;
                    resolve_cft_channel(&manifest, *channel, platform)
                }
                spec => {
                    let url = self.index_base(DEFAULT_CFT_URL);
                    let manifest = fetch_manifest_from(&url).await?;
                    resolve_cft(&manifest, spec, platform)
                }
            },

            Distribution::UngoogledChromium => {
                // Three per-OS repos, so which repo to ask is itself a
                // function of the target platform.
                let repo = Distribution::ungoogled_repo(platform);
                let api_base = self.index_base(DEFAULT_GITHUB_API_BASE);
                let releases = fetch_github_releases(&api_base, repo).await?;
                resolve_ungoogled(&releases, &self.version, platform, repo)
            }

            Distribution::ChromiumSnapshot => {
                let base = self.index_base(DEFAULT_SNAPSHOT_BASE);
                // Refusals happen here, before any network call.
                let revision = match snapshot_revision_for(&self.version)? {
                    Some(rev) => rev,
                    None => fetch_last_change(&base, platform.as_snapshot_str()).await?,
                };
                Ok(resolve_snapshot(&base, revision, platform))
            }
        }
    }

    /// The configured index endpoint, or `default` when none was set.
    fn index_base(&self, default: &str) -> String {
        self.manifest_url
            .clone()
            .unwrap_or_else(|| default.to_string())
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

/// Token that makes a staging path unique to one fetch.
///
/// Process id covers concurrent processes sharing a cache directory (the CI
/// case the `cache_dir` option exists for), and the clock plus an in-process
/// counter covers concurrent tasks inside one process.
fn staging_stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
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

/// True when `path` exists and, with every symlink followed, still lies inside
/// `root`.
///
/// Both callers hand the path to something that follows links and then does
/// something dangerous with what it finds — one `chmod +x`'s it, the other
/// returns it to be executed as a browser. Extraction already refuses an
/// archive whose symlinks leave the tree; this re-asks the question at the
/// point of use, so the guarantee is checked where it matters rather than
/// inherited from a check that ran earlier over different code.
async fn resolves_inside(path: &std::path::Path, root: &std::path::Path) -> bool {
    let (path, root) = (path.to_path_buf(), root.to_path_buf());
    tokio::task::spawn_blocking(move || {
        let (Ok(real), Ok(root)) = (path.canonicalize(), root.canonicalize()) else {
            return false;
        };
        real.starts_with(root)
    })
    .await
    .unwrap_or(false)
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

    /// The macOS end-to-end case that could never have worked: a Chrome for
    /// Testing `.app` bundle carries framework symlinks, so `ensure_chrome()`
    /// aborted on the first one and no Mac user ever got a binary out of it.
    ///
    /// Ignored by default because it needs the network and downloads ~150 MB.
    /// Downloading is not the assertion — the assertion is that what lands is a
    /// binary that RUNS, which is the only thing that proves the bundle came
    /// out intact rather than merely extracted without error.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "network"]
    async fn mac_chrome_for_testing_extracts_to_a_runnable_binary() {
        let cache = tempfile::tempdir().unwrap();
        let binary = Fetcher::new()
            .cache_dir(cache.path())
            // Explicit rather than relying on the default, so this keeps
            // testing CfT if the default distribution ever changes.
            .distribution(Distribution::ChromeForTesting)
            .version(VersionSpec::Explicit("146.0.7680.153".to_string()))
            .platform(Platform::MacArm64)
            .ensure_chrome()
            .await
            .expect("a macOS CfT archive must extract");

        let out = std::process::Command::new(&binary)
            .arg("--version")
            .output()
            .expect("the extracted binary must be executable");
        let version = String::from_utf8_lossy(&out.stdout);
        assert!(
            version.contains("146.0.7680.153"),
            "wrong or unrunnable binary at {binary:?}: {version:?}"
        );

        // The framework symlink from the original error, resolving to a real
        // directory inside the bundle.
        let resources = binary
            .parent()
            .unwrap()
            .join("../Frameworks/Google Chrome for Testing Framework.framework/Resources");
        assert!(
            std::fs::symlink_metadata(&resources)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the framework link must survive as a symlink"
        );
        assert!(resources.is_dir(), "and must resolve to the versioned dir");
    }

    /// Build a tiny zip in-memory containing a single entry at `entry` with
    /// the given sentinel content. Returns the raw zip bytes.
    fn build_stub_zip(entry: &str, sentinel: &[u8]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            // Use unix mode 0o755 so the extracted file is already
            // executable — matches the real archive layouts.
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            writer.start_file(entry, opts).unwrap();
            writer.write_all(sentinel).unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    /// The Chrome for Testing Linux layout, which most tests here use.
    fn build_stub_chrome_zip(sentinel: &[u8]) -> Vec<u8> {
        build_stub_zip("chrome-linux64/chrome", sentinel)
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

        // No leftover staging artifacts of any shape.
        let leftovers: Vec<_> = std::fs::read_dir(cache_root.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");

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

    /// Two fetches of the SAME build sharing a cache directory — the CI shape
    /// the `cache_dir` option exists for.
    ///
    /// They used to derive the same `<build>.tmp`, so each one's stale-tmp
    /// cleanup deleted the other's half-written tree, and whichever won the
    /// rename published whatever was left under the canonical name. A
    /// truncated browser passes `is_runnable` and is then served from cache
    /// forever, so the damage outlives the run that caused it.
    ///
    /// Both must come back with the whole binary.
    #[tokio::test]
    async fn concurrent_fetches_of_one_build_do_not_corrupt_the_cache() {
        let server = MockServer::start().await;
        let sentinel: Vec<u8> = std::iter::repeat_n(b"chrome-payload-", 4096)
            .flatten()
            .copied()
            .collect();
        let zip_bytes = build_stub_chrome_zip(&sentinel);

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
        let manifest_url = format!("{}/manifest.json", server.uri());

        let fetch = || {
            let (dir, url) = (cache_root.path().to_path_buf(), manifest_url.clone());
            async move {
                Fetcher::new()
                    .cache_dir(dir)
                    .platform(Platform::LinuxX64)
                    .version(VersionSpec::Latest)
                    .manifest_url(url)
                    .ensure_chrome()
                    .await
            }
        };

        let (a, b) = tokio::join!(fetch(), fetch());
        let (a, b) = (a.unwrap(), b.unwrap());

        assert_eq!(a, b, "both fetches must name the same cached binary");
        for path in [&a, &b] {
            assert_eq!(
                tokio::fs::read(path).await.unwrap(),
                sentinel,
                "cache holds a truncated binary at {}",
                path.display()
            );
        }
    }

    /// The default must stay Chrome for Testing, or every existing caller
    /// silently changes browser.
    #[test]
    fn a_fresh_fetcher_defaults_to_chrome_for_testing() {
        assert_eq!(Fetcher::new().distribution, Distribution::ChromeForTesting);
        assert!(format!("{:?}", Fetcher::new()).contains("ChromeForTesting"));
    }

    /// ungoogled end-to-end: the GitHub release list is the index, the tag
    /// carries a packaging suffix, and the zip's top-level directory is the
    /// asset name — three things no other distribution does.
    #[tokio::test]
    async fn ensure_chrome_end_to_end_for_ungoogled_windows() {
        let server = MockServer::start().await;
        let sentinel = b"MZ stub ungoogled chrome.exe";
        let top = "ungoogled-chromium_151.0.7922.71-1.1_windows_x64";
        let zip_bytes = build_stub_zip(&format!("{top}/chrome.exe"), sentinel);

        let releases = format!(
            r#"[{{
                "tag_name": "151.0.7922.71-1.1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {{"name": "{top}.zip", "browser_download_url": "{server}/ug.zip"}}
                ]
            }}]"#,
            server = server.uri()
        );

        Mock::given(method("GET"))
            .and(path(
                "/repos/ungoogled-software/ungoogled-chromium-windows/releases",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(releases))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/ug.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::UngoogledChromium)
            .platform(Platform::Win64)
            // The tag is `151.0.7922.71-1.1`; the version is the prefix.
            .version(VersionSpec::Explicit("151.0.7922.71".into()))
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap();

        // Namespaced under the distribution slug, keyed by the Chrome version
        // rather than the full tag.
        assert_eq!(
            bin_path,
            cache_root
                .path()
                .join("ungoogled/151.0.7922.71")
                .join(top)
                .join("chrome.exe")
        );
        assert_eq!(tokio::fs::read(&bin_path).await.unwrap(), sentinel);
        assert!(!cache_root.path().join("151.0.7922.71").exists());
    }

    /// An AppImage is not an archive — install is a move plus a chmod, and
    /// the AppImage itself is the executable.
    #[tokio::test]
    async fn ensure_chrome_installs_an_ungoogled_appimage_without_extracting() {
        let server = MockServer::start().await;
        let sentinel = b"\x7fELF stub appimage";

        let releases = format!(
            r#"[{{
                "tag_name": "151.0.7922.71-1",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {{"name": "ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage.zsync",
                      "browser_download_url": "{server}/sidecar"}},
                    {{"name": "ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage",
                      "browser_download_url": "{server}/app"}}
                ]
            }}]"#,
            server = server.uri()
        );

        Mock::given(method("GET"))
            .and(path(
                "/repos/ungoogled-software/ungoogled-chromium-portablelinux/releases",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(releases))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(sentinel.to_vec()))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::UngoogledChromium)
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Latest)
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap();

        // The `.zsync` sidecar is served from a route that is never mocked,
        // so picking it would fail the test outright.
        assert_eq!(
            bin_path,
            cache_root
                .path()
                .join("ungoogled/151.0.7922.71/chrome.AppImage")
        );
        assert_eq!(tokio::fs::read(&bin_path).await.unwrap(), sentinel);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let meta = tokio::fs::metadata(&bin_path).await.unwrap();
            assert!(meta.permissions().mode() & 0o111 != 0);
        }
    }

    /// Snapshot end-to-end: `LAST_CHANGE` names the revision, the URL uses
    /// the bucket's own platform spelling, and the cache key is `r<rev>`.
    #[tokio::test]
    async fn ensure_chrome_end_to_end_for_a_chromium_snapshot() {
        let server = MockServer::start().await;
        let sentinel = b"#!/bin/sh\necho stub-snapshot\n";
        let zip_bytes = build_stub_zip("chrome-linux/chrome", sentinel);

        Mock::given(method("GET"))
            .and(path("/Linux_x64/LAST_CHANGE"))
            .respond_with(ResponseTemplate::new(200).set_body_string("1674883"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Linux_x64/1674883/chrome-linux.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::ChromiumSnapshot)
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Latest)
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap();

        assert_eq!(
            bin_path,
            cache_root
                .path()
                .join("snapshot/r1674883/chrome-linux/chrome")
        );
        assert_eq!(tokio::fs::read(&bin_path).await.unwrap(), sentinel);
    }

    /// A pinned revision must skip `LAST_CHANGE` entirely — the mock server
    /// has no route for it, so consulting it would fail.
    #[tokio::test]
    async fn an_explicit_revision_bypasses_last_change() {
        let server = MockServer::start().await;
        let zip_bytes = build_stub_zip("chrome-win/chrome.exe", b"MZ stub");

        Mock::given(method("GET"))
            .and(path("/Win_x64/1600000/chrome-win.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let bin_path = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::ChromiumSnapshot)
            .platform(Platform::Win64)
            .version(VersionSpec::Revision(1_600_000))
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap();

        assert_eq!(
            bin_path,
            cache_root
                .path()
                .join("snapshot/r1600000/chrome-win/chrome.exe")
        );
    }

    /// Asking the snapshot bucket for a version must fail before any request
    /// goes out — there is nothing to look it up in, and returning the newest
    /// snapshot would be a different browser.
    #[tokio::test]
    async fn snapshot_refuses_an_explicit_version_without_touching_the_network() {
        let server = MockServer::start().await;
        // Deliberately no mocks: any request at all fails the test.
        let cache_root = tempfile::tempdir().unwrap();

        let err = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::ChromiumSnapshot)
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Explicit("151.0.7922.76".into()))
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap_err();

        assert!(matches!(err, FetcherError::UnsupportedSelector { .. }));
        assert!(err.to_string().contains("--revision"), "{err}");
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    /// Two distributions publishing the same Chrome version must not share a
    /// cache directory, or the second fetch serves the first one's binary.
    #[tokio::test]
    async fn the_same_version_from_two_distributions_does_not_collide() {
        let server = MockServer::start().await;
        let cft_sentinel = b"chrome-for-testing".to_vec();
        let ug_sentinel = b"ungoogled".to_vec();

        let manifest = format!(
            r#"{{"versions":[{{"version":"151.0.7922.71","revision":"1","downloads":{{"chrome":[
                {{"platform":"linux64","url":"{server}/cft.zip"}}]}}}}]}}"#,
            server = server.uri()
        );
        let releases = format!(
            r#"[{{"tag_name":"151.0.7922.71-1","draft":false,"prerelease":false,"assets":[
                {{"name":"ungoogled-chromium-151.0.7922.71-1-x86_64.AppImage",
                  "browser_download_url":"{server}/ug.AppImage"}}]}}]"#,
            server = server.uri()
        );

        Mock::given(method("GET"))
            .and(path("/manifest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cft.zip"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(build_stub_chrome_zip(&cft_sentinel)),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/ungoogled-software/ungoogled-chromium-portablelinux/releases",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(releases))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/ug.AppImage"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(ug_sentinel.clone()))
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();

        let cft = Fetcher::new()
            .cache_dir(cache_root.path())
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Explicit("151.0.7922.71".into()))
            .manifest_url(format!("{}/manifest.json", server.uri()))
            .ensure_chrome()
            .await
            .unwrap();
        let ug = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::UngoogledChromium)
            .platform(Platform::LinuxX64)
            .version(VersionSpec::Explicit("151.0.7922.71".into()))
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap();

        assert_ne!(cft, ug);
        assert_eq!(tokio::fs::read(&cft).await.unwrap(), cft_sentinel);
        assert_eq!(tokio::fs::read(&ug).await.unwrap(), ug_sentinel);
    }

    /// GitHub answers 403 with `x-ratelimit-remaining: 0` when the
    /// unauthenticated 60/hour budget is gone. That has to reach the operator
    /// as an explanation, not as a bare HTTP status.
    #[tokio::test]
    async fn github_rate_limiting_surfaces_as_an_actionable_message() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/ungoogled-software/ungoogled-chromium-macos/releases",
            ))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", "1786023295")
                    .set_body_string(r#"{"message":"API rate limit exceeded"}"#),
            )
            .mount(&server)
            .await;

        let cache_root = tempfile::tempdir().unwrap();
        let err = Fetcher::new()
            .cache_dir(cache_root.path())
            .distribution(Distribution::UngoogledChromium)
            .platform(Platform::MacArm64)
            .version(VersionSpec::Latest)
            .manifest_url(server.uri())
            .ensure_chrome()
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(matches!(err, FetcherError::GitHubRateLimited { .. }));
        assert!(msg.contains("60 requests/hour"), "{msg}");
        assert!(msg.contains("GITHUB_TOKEN"), "{msg}");
        assert!(msg.contains("1786023295"), "{msg}");
    }
}
