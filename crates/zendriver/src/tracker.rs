//! Tracker/fingerprinter blocklist sourcing for the `tracker-blocking`
//! feature.
//!
//! Holds the bundled curated list (`trackers.txt`), a tolerant parser, and a
//! download-on-first-use cache for `tracker_blocklist_url` sources. The pure
//! matching lives in `zendriver-interception` ([`HostMatcher`]); this module
//! only produces the host strings that feed it.
//!
//! [`HostMatcher`]: zendriver_interception::HostMatcher

use std::path::PathBuf;

/// Bundled curated list, embedded at compile time (feature-gated, so it only
/// costs binary size when `tracker-blocking` is enabled).
const BUNDLED: &str = include_str!("trackers.txt");

/// Parse the bundled list into hosts.
pub(crate) fn bundled_hosts() -> Vec<String> {
    parse_blocklist(BUNDLED)
}

/// Parse a blocklist text into hostnames.
///
/// Tolerant of two common formats so user `_file`/`_url` sources work as-is:
/// plain `host` per line, and hosts-file `0.0.0.0 host` / `127.0.0.1 host`
/// (the last whitespace token on the line is taken as the host). `#` starts a
/// comment (whole-line or inline); blank lines are ignored. Hosts are
/// lower-cased.
pub(crate) fn parse_blocklist(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split_whitespace()
                .last()
                .unwrap_or(line)
                .to_ascii_lowercase()
        })
        .collect()
}

/// Cache file path for a downloaded `_url` source — keyed by a hash of the URL
/// under the same cache root as `zendriver-fetcher`/`zendriver-fingerprints`.
fn cache_path(url: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zendriver/trackers")
        .join(format!("{:016x}.txt", h.finish()))
}

/// Load a host list from local cache, or download from `url` and cache it.
///
/// Mirrors the atomic-write download-on-first-use pattern in
/// `zendriver-fingerprints` `pool::load_or_download` (write `<path>.tmp`, then
/// `rename`). `reqwest`/IO failures are surfaced as [`std::io::Error`] so the
/// caller folds them into `ZendriverError::Io` without a new public error
/// variant.
pub(crate) async fn load_or_download_blocklist(url: &str) -> Result<Vec<String>, std::io::Error> {
    let cache = cache_path(url);

    // Fast path: cache hit.
    if let Ok(text) = std::fs::read_to_string(&cache) {
        tracing::debug!(path = %cache.display(), "tracker blocklist cache hit");
        return Ok(parse_blocklist(&text));
    }

    tracing::debug!(url, "tracker blocklist cache miss — downloading");
    let body = reqwest::get(url)
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(std::io::Error::other)?
        .text()
        .await
        .map_err(std::io::Error::other)?;

    // Atomic write: tmp -> rename.
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = cache.with_extension("tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, &cache)?;

    tracing::debug!(path = %cache.display(), "tracker blocklist cached");
    Ok(parse_blocklist(&body))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parser_ignores_comments_and_blanks() {
        let text = "# header\n\n  evil.com  \nbad.net # inline comment\n   \n";
        let hosts = parse_blocklist(text);
        assert_eq!(hosts, vec!["evil.com".to_string(), "bad.net".to_string()]);
    }

    #[test]
    fn parser_accepts_hosts_file_format() {
        // uBlock / Peter Lowe's hosts format: "0.0.0.0 host" or "127.0.0.1 host".
        let text = "0.0.0.0 tracker.com\n127.0.0.1 fp.example.org\n";
        let hosts = parse_blocklist(text);
        assert_eq!(
            hosts,
            vec!["tracker.com".to_string(), "fp.example.org".to_string()]
        );
    }

    #[test]
    fn parser_lowercases() {
        assert_eq!(parse_blocklist("EVIL.COM\n"), vec!["evil.com".to_string()]);
    }

    #[test]
    fn bundled_list_parses_to_many_hosts() {
        let hosts = bundled_hosts();
        // Spec target is ~50-150; assert a sane floor + a couple of known
        // entries are present and a known anti-bot vendor is ABSENT.
        assert!(hosts.len() >= 50, "bundled list too small: {}", hosts.len());
        assert!(hosts.iter().any(|h| h == "fingerprintjs.com"));
        assert!(hosts.iter().any(|h| h == "doubleclick.net"));
        // Curation principle: no active anti-bot challenge vendors.
        for banned in [
            "datadome.co",
            "hcaptcha.com",
            "perimeterx.net",
            "imperva.com",
        ] {
            assert!(
                !hosts.iter().any(|h| h == banned),
                "anti-bot vendor {banned} must NOT be in the bundled list"
            );
        }
    }
}
