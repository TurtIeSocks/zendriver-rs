//! Real-Chrome probe documenting that `Accept-Encoding` is **not** correctable
//! over CDP: Chrome's network service owns the header. A stealth profile that
//! pins a pre-`zstd` Chrome major (120) on a `zstd`-capable binary (>=123) still
//! advertises `zstd` on the wire — the Accept-Encoding-vs-User-Agent mismatch
//! that `zendriver_stealth` only *warns* about (it cannot fix it).
//!
//! Verified the negative result against Chrome 148:
//!   - `Network.setExtraHTTPHeaders` for `Accept-Encoding` is silently dropped.
//!   - `--disable-features=ZstdContentEncoding` is inert on current builds.
//!
//! See `docs/superpowers/specs/2026-06-02-header-coherence-design.md`.
//!
//! Gated by `#[cfg(feature = "integration-tests")]` + `#[ignore]` (same
//! convention as `fingerprint_integration.rs`). Run on a machine with a real
//! Chrome **>= 123**:
//! ```sh
//! cargo test -p zendriver --test accept_encoding_coherence \
//!     --features integration-tests -- --ignored --nocapture
//! ```
#![cfg(feature = "integration-tests")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

use serial_test::serial;
use zendriver::Browser;
use zendriver::stealth::StealthProfile;

/// With a forced claimed-major skew (claim Chrome 120 on a >=123 binary), the
/// navigation request STILL advertises the binary's encodings (`zstd` present) —
/// proving the header is binary-controlled and the stealth layer is right to
/// only warn. Exactly one `Accept-Encoding` header (no duplication) is asserted
/// to rule out any accidental append.
#[tokio::test]
#[serial]
#[ignore] // requires a real Chrome binary >= 123; run with --ignored
async fn pinned_chrome_major_leaks_binary_accept_encoding() {
    // One-shot local HTTP/1.1 server that records the request headers it sees.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(&stream);
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
                headers.push(line.trim_end().to_string());
            }
            let mut w = &stream;
            let _ =
                w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = tx.send(headers);
        }
    });

    // Claim Chrome 120 (pre-`zstd`). On a real >=123 binary this is a skew the
    // stealth layer warns about but cannot correct.
    let profile = StealthProfile::spoofed().chrome_version(120);
    let browser = Browser::builder()
        .stealth(profile)
        .headless(true)
        .launch()
        .await
        .expect("launch");
    let tab = browser.main_tab();
    let _ = tab.goto(format!("http://127.0.0.1:{port}/")).await;

    let headers = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("server saw a request");
    let ae: Vec<&String> = headers
        .iter()
        .filter(|h| h.to_ascii_lowercase().starts_with("accept-encoding:"))
        .collect();
    // Exactly one Accept-Encoding header — the binary's, undisturbed.
    assert_eq!(
        ae.len(),
        1,
        "expected one Accept-Encoding header, got {ae:?}"
    );
    assert!(
        ae[0].to_ascii_lowercase().contains("zstd"),
        "expected the >=123 binary to leak `zstd` despite the claimed major; \
         got {ae:?} (is this Chrome older than 123?)",
    );
    browser.close().await.ok();
}
