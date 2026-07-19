//! Cache-freshness policy shared by the pool and generative download-on-first-use
//! caches.
//!
//! ponytail: TTL-on-access, not a background scheduler. Both `load_or_download`
//! call sites read the cache once per call and never hold the parsed set in
//! memory across time, so there is no long-lived holder to refresh mid-life —
//! add a scheduler only if one ever appears.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// Freshness policy for a download-on-first-use cache file.
///
/// The default — `CachePolicy::default()`, equal to [`CachePolicy::PERMANENT`]
/// — matches the original behavior of both `pool::load_or_download` and
/// `generative::Generator::load_or_download`: any successfully-read, successfully
/// -parsed cache file is used forever, no matter its age.
///
/// ```
/// use zendriver_fingerprints::CachePolicy;
/// use std::time::Duration;
///
/// // Default: permanent cache (never re-downloads due to age).
/// assert_eq!(CachePolicy::default(), CachePolicy::PERMANENT);
///
/// // Re-download once the cached file is older than an hour.
/// let hourly = CachePolicy::with_ttl(Duration::from_secs(3600));
///
/// // Always re-download, ignoring any cache hit.
/// let forced = CachePolicy::force_refresh();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    /// Re-download when the cached file's mtime is older than this. `None`
    /// (the default) means permanent cache: a valid cache hit is never
    /// considered stale by age.
    pub ttl: Option<Duration>,
    /// Always re-download, ignoring any cache hit (and `ttl`). Writes a fresh
    /// cache file on success, same as any other download.
    pub force_refresh: bool,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self::PERMANENT
    }
}

impl CachePolicy {
    /// Permanent cache: identical to `CachePolicy::default()`, spelled out for
    /// call sites that want to be explicit about it.
    pub const PERMANENT: Self = Self {
        ttl: None,
        force_refresh: false,
    };

    /// Re-download when the cached file is older than `ttl`.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl: Some(ttl),
            force_refresh: false,
        }
    }

    /// Always re-download, ignoring any cache hit.
    pub fn force_refresh() -> Self {
        Self {
            ttl: None,
            force_refresh: true,
        }
    }
}

/// Is a cache file with modification time `mtime`, observed at `now`, stale
/// under `ttl`?
///
/// `ttl: None` is never stale (permanent cache — the regression guard for the
/// original always-cache-forever behavior).
///
/// **Fails closed on clock error:** `now.duration_since(mtime)` errors when
/// `mtime` is after `now` (e.g. clock skew, or a cache file with a
/// future-dated mtime). That error is treated as **stale** — never panics,
/// never treats the file as fresh.
pub(crate) fn is_stale(mtime: SystemTime, now: SystemTime, ttl: Option<Duration>) -> bool {
    match ttl {
        None => false,
        // `>=` (not `>`) so `ttl: Duration::ZERO` means "always stale", even
        // for a file written in the same instant as the freshness check.
        Some(ttl) => match now.duration_since(mtime) {
            Ok(elapsed) => elapsed >= ttl,
            Err(_) => true, // clock skew / future mtime -> fail closed: stale
        },
    }
}

/// I/O wrapper around [`is_stale`]: reads `cache`'s mtime and checks it against
/// `ttl` at the current time.
///
/// A missing file or an mtime read failure is reported as **not stale** here —
/// the caller's subsequent cache-read attempt will fail on its own and fall
/// through to a fresh download, exactly like the original no-cache-yet path.
/// This also means: when `ttl` is `None` (the default), this never touches the
/// filesystem at all, so the permanent-cache path has zero behavioral or
/// syscall difference from the pre-TTL code.
pub(crate) fn path_is_stale(cache: &Path, ttl: Option<Duration>) -> bool {
    let Some(ttl) = ttl else {
        return false;
    };
    match std::fs::metadata(cache).and_then(|m| m.modified()) {
        Ok(mtime) => is_stale(mtime, SystemTime::now(), Some(ttl)),
        Err(_) => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn permanent_cache_is_never_stale() {
        let now = std::time::SystemTime::now();
        let ancient = now - Duration::from_secs(1_000_000_000);
        assert!(!is_stale(ancient, now, None));
    }

    #[test]
    fn within_ttl_is_not_stale() {
        let now = std::time::SystemTime::now();
        let mtime = now - Duration::from_secs(10);
        assert!(!is_stale(mtime, now, Some(Duration::from_secs(60))));
    }

    #[test]
    fn older_than_ttl_is_stale() {
        let now = std::time::SystemTime::now();
        let mtime = now - Duration::from_secs(120);
        assert!(is_stale(mtime, now, Some(Duration::from_secs(60))));
    }

    #[test]
    fn zero_ttl_is_always_stale() {
        let now = std::time::SystemTime::now();
        assert!(is_stale(now, now, Some(Duration::ZERO)));
    }

    #[test]
    fn clock_skew_future_mtime_fails_closed_to_stale() {
        // mtime "in the future" relative to now — the classic clock-skew case
        // where `now.duration_since(mtime)` errors. Must fail closed (stale),
        // never panic, never treat as fresh.
        let now = std::time::SystemTime::now();
        let future_mtime = now + Duration::from_secs(3600);
        assert!(is_stale(future_mtime, now, Some(Duration::from_secs(60))));
    }

    #[test]
    fn default_policy_is_permanent_cache() {
        let policy = CachePolicy::default();
        assert_eq!(policy.ttl, None);
        assert!(!policy.force_refresh);
    }

    #[test]
    fn path_is_stale_missing_file_is_not_stale() {
        // No file at all -> not stale (the subsequent read attempt fails on
        // its own and falls through to a fresh download).
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        assert!(!path_is_stale(&missing, Some(Duration::from_secs(60))));
    }

    #[test]
    fn path_is_stale_fresh_file_within_ttl_is_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cache.json");
        std::fs::write(&file, b"{}").unwrap();
        assert!(!path_is_stale(&file, Some(Duration::from_secs(3600))));
    }

    #[test]
    fn path_is_stale_none_ttl_never_touches_disk_state() {
        // A missing file with `ttl: None` still reports "not stale" — the
        // permanent-cache path short-circuits before any metadata read.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        assert!(!path_is_stale(&missing, None));
    }
}
