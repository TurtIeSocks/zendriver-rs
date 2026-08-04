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
use std::time::Duration;

/// Bundled curated list, embedded at compile time (feature-gated, so it only
/// costs binary size when `tracker-blocking` is enabled).
const BUNDLED: &str = include_str!("trackers.txt");

/// Connect timeout for a `tracker_blocklist_url` download.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Whole-request timeout (connect, headers, and body) for a
/// `tracker_blocklist_url` download.
///
/// This download runs on the `Browser::launch()` path, so an unbounded fetch
/// against a half-open host would hang the launch forever. Both bounds exist
/// to keep the worst case a bounded, reported failure.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);

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

/// Download a blocklist over HTTP with both bounds applied.
///
/// Split out from [`load_or_download_blocklist`] so the timeouts are
/// injectable in tests; production callers pass the two constants above.
/// `reqwest` failures are surfaced as [`std::io::Error`] so the caller folds
/// them into `ZendriverError::Io` without a new public error variant.
async fn download_blocklist(
    url: &str,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<String, std::io::Error> {
    // reqwest 0.13 installs no rustls crypto provider of its own (see the
    // workspace manifest), and the omission surfaces as a runtime panic rather
    // than a build error. Install before the client exists.
    crate::tls::install_default_crypto_provider();
    let client = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .map_err(std::io::Error::other)?;
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(std::io::Error::other)?
        .text()
        .await
        .map_err(std::io::Error::other)
}

/// Load a host list from local cache, or download from `url` and cache it.
///
/// Mirrors the atomic-write download-on-first-use pattern in
/// `zendriver-fingerprints` `pool::load_or_download` (write a temp sibling,
/// then `rename`) via the shared [`crate::cookies::persistence::write_atomic`]
/// helper. The download is bounded by [`DOWNLOAD_CONNECT_TIMEOUT`] /
/// [`DOWNLOAD_TIMEOUT`]; `reqwest`/IO failures are surfaced as
/// [`std::io::Error`] so the caller folds them into `ZendriverError::Io`
/// without a new public error variant.
pub(crate) async fn load_or_download_blocklist(url: &str) -> Result<Vec<String>, std::io::Error> {
    let cache = cache_path(url);

    // Fast path: cache hit.
    if let Ok(text) = std::fs::read_to_string(&cache) {
        tracing::debug!(path = %cache.display(), "tracker blocklist cache hit");
        return Ok(parse_blocklist(&text));
    }

    tracing::debug!(url, "tracker blocklist cache miss — downloading");
    let body = download_blocklist(url, DOWNLOAD_CONNECT_TIMEOUT, DOWNLOAD_TIMEOUT).await?;

    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::cookies::persistence::write_atomic(&cache, body.as_bytes()).await?;

    tracing::debug!(path = %cache.display(), "tracker blocklist cached");
    Ok(parse_blocklist(&body))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
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

    /// A host that completes the TCP handshake and then never answers must
    /// not hang the caller — and with it `Browser::launch()`. The request
    /// timeout has to turn it into a bounded error.
    #[tokio::test]
    async fn download_times_out_against_a_silent_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept connections and hold them open, never writing a response.
        let _server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let url = format!("http://{addr}/trackers.txt");
        let started = std::time::Instant::now();
        // The outer timeout is the failure mode under test: without a client
        // timeout the inner future never resolves and this expect() fires.
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            download_blocklist(&url, Duration::from_millis(500), Duration::from_millis(200)),
        )
        .await
        .expect("download_blocklist hung past its own request timeout");

        assert!(
            res.is_err(),
            "a server that never answers must surface as an error, got {res:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the 200ms request timeout should fire promptly, took {:?}",
            started.elapsed()
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
