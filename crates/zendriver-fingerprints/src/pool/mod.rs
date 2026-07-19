//! Pool of real-device personas: download-on-first-use + seeded sampling.

use std::env;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zendriver_stealth::{Persona, Seed};

use crate::CachePolicy;

/// A parsed pool of real-device personas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSet {
    records: Vec<Persona>,
}

impl PoolSet {
    /// Build from an in-memory list (tests / manual construction).
    pub fn from_records(records: Vec<Persona>) -> Self {
        Self { records }
    }

    /// Parse a downloaded pool asset (JSON array of personas).
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        Ok(Self {
            records: serde_json::from_str(s)?,
        })
    }

    /// Deterministically pick one persona by seed.
    ///
    /// `idx = seed.value() as usize % len` — stable across versions.
    ///
    /// # Panics
    /// Panics if the pool is empty.
    pub fn sample(&self, seed: Seed) -> Persona {
        assert!(!self.records.is_empty(), "pool is empty");
        let idx = (seed.value() as usize) % self.records.len();
        self.records[idx].clone()
    }
}

// ---------------------------------------------------------------------------
// Pool error
// ---------------------------------------------------------------------------

/// Error from pool operations.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Cache helpers
// ---------------------------------------------------------------------------

/// Cache file path for pool assets — same root as `zendriver-fetcher`
/// (`dirs::cache_dir() / zendriver / fingerprints / pool.json`).
pub fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(env::temp_dir)
        .join("zendriver/fingerprints/pool.json")
}

// ---------------------------------------------------------------------------
// Download-on-first-use
// ---------------------------------------------------------------------------

/// Load pool from local cache or download and cache from `url`.
///
/// Uses the same atomic-write pattern as `zendriver-fetcher` (write to
/// `<path>.tmp`, then `rename` into place).
///
/// `policy` controls cache freshness — see [`CachePolicy`]. Pass
/// `CachePolicy::default()` for the original permanent-cache behavior (which
/// also skips the mtime read entirely — zero behavioral difference from
/// before this knob existed).
///
/// ```no_run
/// use zendriver_fingerprints::CachePolicy;
/// use zendriver_fingerprints::pool::load_or_download;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // Permanent cache (default behavior).
/// let pool = load_or_download("https://example.com/pool.json", CachePolicy::default()).await?;
///
/// // Re-download once the cached file is older than a day.
/// let ttl = CachePolicy::with_ttl(std::time::Duration::from_secs(86_400));
/// let pool = load_or_download("https://example.com/pool.json", ttl).await?;
/// # let _ = pool;
/// # Ok(())
/// # }
/// ```
pub async fn load_or_download(url: &str, policy: CachePolicy) -> Result<PoolSet, PoolError> {
    let cache = cache_path();

    // Fast path: cache hit (only when not force-refreshing and not stale).
    if !policy.force_refresh && !crate::cache::path_is_stale(&cache, policy.ttl) {
        if let Ok(bytes) = std::fs::read_to_string(&cache) {
            if let Ok(set) = PoolSet::from_json(&bytes) {
                tracing::debug!(path = %cache.display(), "pool cache hit");
                return Ok(set);
            }
        }
    }

    tracing::debug!(url, "pool cache miss — downloading");
    let body = reqwest::get(url).await?.text().await?;
    let set = PoolSet::from_json(&body)?;

    // Atomic write: tmp → rename.
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = cache.with_extension("tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, &cache)?;

    tracing::debug!(path = %cache.display(), "pool cached");
    Ok(set)
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a minimal [`Persona`] for tests.
#[cfg(test)]
pub(crate) fn mk(platform: &str, mem: u32) -> Persona {
    use zendriver_stealth::Platform;
    let plat = match platform {
        "Win32" => Platform::Win32,
        "MacIntel" => Platform::MacIntel,
        _ => Platform::LinuxX86_64,
    };
    Persona {
        platform: Some(plat),
        device_memory_gb: Some(mem),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn sample_is_deterministic_for_seed() {
        let set = PoolSet::from_records(vec![mk("Win32", 8), mk("MacIntel", 16), mk("Win32", 4)]);
        let a = set.sample(Seed::from_u64(42));
        let b = set.sample(Seed::from_u64(42));
        assert_eq!(a.device_memory_gb, b.device_memory_gb);
    }

    #[test]
    fn sample_selects_correct_index() {
        let set = PoolSet::from_records(vec![mk("Win32", 8), mk("MacIntel", 16), mk("Win32", 4)]);
        // seed 42 % 3 = 0  → Win32/8
        let p = set.sample(Seed::from_u64(42));
        assert_eq!(p.device_memory_gb, Some(8));
        // seed 1 % 3 = 1 → MacIntel/16
        let q = set.sample(Seed::from_u64(1));
        assert_eq!(q.device_memory_gb, Some(16));
    }

    #[test]
    fn from_json_round_trip() {
        let original = PoolSet::from_records(vec![mk("Win32", 8), mk("MacIntel", 16)]);
        // PoolSet serializes as {"records":[...]}; from_json expects a bare array.
        // So we serialize just the records array.
        let records_json = serde_json::to_string(&original.records).unwrap();
        let parsed = PoolSet::from_json(&records_json).unwrap();
        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.records[0].device_memory_gb, Some(8));
        assert_eq!(parsed.records[1].device_memory_gb, Some(16));
    }
}
