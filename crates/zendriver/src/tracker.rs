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

/// Ceiling on the body of a `tracker_blocklist_url` download.
///
/// The time bound above is not a resource bound: it caps how long a source may
/// keep sending, and on a datacentre link twenty seconds of chunked response is
/// gigabytes of `String` inside `Browser::launch()`, followed by
/// [`write_atomic`](crate::io::write_atomic) copying all of it into the cache
/// directory. The URL is one the user pasted in — typically a third-party list
/// mirror — so "the source is honest about its size" is not something this
/// path gets to assume.
///
/// 32 MiB is roughly thirty times the largest lists in circulation (uBlock's
/// and Peter Lowe's are around 1 MB), so it is a ceiling on abuse rather than a
/// limit a real source will meet.
const MAX_BLOCKLIST_BYTES: usize = 32 * 1024 * 1024;

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

/// Download a blocklist over HTTP with all three bounds applied.
///
/// Split out from [`load_or_download_blocklist`] so the bounds are injectable
/// in tests; production callers pass the three constants above. `reqwest`
/// failures are surfaced as [`std::io::Error`] so the caller folds them into
/// `ZendriverError::Io` without a new public error variant.
///
/// The body is accumulated chunk by chunk rather than through
/// `Response::text()`, because `max_bytes` has to hold against a source that
/// simply keeps sending: a `Content-Length` check only helps when the server
/// is honest, and a chunked response advertises nothing at all.
async fn download_blocklist(
    url: &str,
    connect_timeout: Duration,
    request_timeout: Duration,
    max_bytes: usize,
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
    let mut resp = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(std::io::Error::other)?;

    // Cheap rejection when the server declares its size, before a byte of body
    // is read.
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("blocklist at {url} advertises {len} bytes, over the {max_bytes} cap"),
            ));
        }
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(std::io::Error::other)? {
        if body.len() + chunk.len() > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("blocklist at {url} exceeded the {max_bytes} byte cap while downloading"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.utf8_error()))
}

/// Load a host list from local cache, or download from `url` and cache it.
///
/// Mirrors the atomic-write download-on-first-use pattern in
/// `zendriver-fingerprints` `pool::load_or_download` (write a temp sibling,
/// then `rename`) via the shared [`crate::io::write_atomic`] helper. The
/// download is bounded by [`DOWNLOAD_CONNECT_TIMEOUT`] / [`DOWNLOAD_TIMEOUT`]
/// in time and [`MAX_BLOCKLIST_BYTES`] in size; `reqwest`/IO failures are
/// surfaced as [`std::io::Error`] so the caller folds them into
/// `ZendriverError::Io` without a new public error variant.
///
/// The cache file inherits that helper's owner-only `0600` default. Nothing in
/// a public blocklist is secret, so the restriction buys no confidentiality
/// here — but it costs nothing either (the cache root is already per-user, and
/// a miss just re-downloads), and one rule across both callers beats a second
/// mode knob threaded through the helper for a file nobody needs to share.
/// An operator who does want it group-readable can `chmod` once; the helper
/// preserves an existing destination's mode on every later refresh.
pub(crate) async fn load_or_download_blocklist(url: &str) -> Result<Vec<String>, std::io::Error> {
    let cache = cache_path(url);

    // Fast path: cache hit. A miss is the expected `NotFound`; anything else
    // (a cache file the process cannot read, a broken cache root) would
    // otherwise re-download silently on every single launch, so it gets said
    // out loud.
    match tokio::fs::read_to_string(&cache).await {
        Ok(text) => {
            tracing::debug!(path = %cache.display(), "tracker blocklist cache hit");
            return Ok(parse_blocklist(&text));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(url, "tracker blocklist cache miss — downloading");
        }
        Err(e) => {
            tracing::warn!(
                path = %cache.display(),
                error = %e,
                "tracker blocklist cache is unreadable — re-downloading (this repeats every launch \
                 until the cache file is readable or removed)"
            );
        }
    }

    let body = download_blocklist(
        url,
        DOWNLOAD_CONNECT_TIMEOUT,
        DOWNLOAD_TIMEOUT,
        MAX_BLOCKLIST_BYTES,
    )
    .await?;

    if let Some(parent) = cache.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    crate::io::write_atomic(&cache, body.as_bytes()).await?;

    tracing::debug!(path = %cache.display(), "tracker blocklist cached");
    Ok(parse_blocklist(&body))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tokio::io::AsyncWriteExt;

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
            download_blocklist(
                &url,
                Duration::from_millis(500),
                Duration::from_millis(200),
                MAX_BLOCKLIST_BYTES,
            ),
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

    /// A source that declares an oversized body is rejected on the headers,
    /// before any of it is buffered.
    #[tokio::test]
    async fn download_rejects_an_oversized_content_length() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/trackers.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 8192]))
            .mount(&server)
            .await;

        let err = download_blocklist(
            &format!("{}/trackers.txt", server.uri()),
            Duration::from_secs(2),
            Duration::from_secs(5),
            1024,
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("advertises 8192 bytes"),
            "the declared size should be rejected up front, got {err}"
        );
    }

    /// The real bound: a chunked response advertises no length at all, so the
    /// cap has to hold against the accumulating body. Without it this streams
    /// until the request timeout and buffers everything it received — the
    /// gigabytes-in-`Browser::launch()` case.
    #[tokio::test]
    async fn download_caps_a_chunked_body_that_never_ends() {
        const CAP: usize = 64 * 1024;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // `wiremock` only serves fixed bodies, so the endless chunked source
        // is hand-rolled. It writes until the client hangs up.
        let server = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            // Enough of the request to get past the headers; the body is what
            // is under test, not the parsing.
            let mut scratch = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut scratch).await;
            if stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            let chunk = format!("1000\r\n{}\r\n", "x".repeat(0x1000));
            while stream.write_all(chunk.as_bytes()).await.is_ok() {}
        });

        let url = format!("http://{addr}/trackers.txt");
        let err = tokio::time::timeout(
            Duration::from_secs(10),
            // A request timeout far longer than the test's own: if the cap
            // never fires, this fails on the outer timeout rather than
            // passing because a *different* bound rescued it.
            download_blocklist(&url, Duration::from_secs(2), Duration::from_secs(60), CAP),
        )
        .await
        .expect("the size cap never fired — the download ran past its own bound")
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains(&format!("exceeded the {CAP} byte cap")),
            "expected the accumulating-body cap, got {err}"
        );

        server.abort();
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
