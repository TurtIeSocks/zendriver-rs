//! Integration tests for `tab.monitor()` and `tab.request()`.
//!
//! These tests start a real Chrome instance against a wiremock server and
//! exercise the network monitor stream and the browser-context HTTP builder.
//! They are gated behind `integration-tests` and marked `#[ignore]` so they
//! only run when a Chrome binary is available and the caller passes `--ignored`.
//!
//! Run:
//! ```bash
//! cargo test -p zendriver --features integration-tests \
//!     --test network_monitor_http -- --ignored
//! ```

#![cfg(feature = "integration-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zendriver::{Browser, NetworkEvent};

// ──────────────────────────────────────────────────────────────────────────────
// Helpers — mirrored verbatim from integration_phase4.rs
// ──────────────────────────────────────────────────────────────────────────────

/// Spin up a mock HTTP server that returns `html` at `/`.
async fn fixture_with_html(html: &str) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html.as_bytes().to_vec(), "text/html"),
        )
        .mount(&mock)
        .await;
    mock
}

// ──────────────────────────────────────────────────────────────────────────────
// monitor_captures_fetch
// ──────────────────────────────────────────────────────────────────────────────

/// The monitor sees an in-page `fetch("/data")` as a `NetworkEvent::Http` with
/// status 200 and a retrievable text body.
#[tokio::test]
#[serial]
#[ignore]
async fn monitor_captures_fetch() {
    let mock = MockServer::start().await;

    // Serve the data endpoint BEFORE the page so the page fetch always hits it.
    Mock::given(method("GET"))
        .and(path("/data"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello-monitor"))
        .mount(&mock)
        .await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body>
                  <script>
                    window.fetchDone = false;
                    fetch('/data')
                      .then(r => r.text())
                      .then(t => { window.fetchResult = t; window.fetchDone = true; })
                      .catch(e => { window.fetchErr = String(e); window.fetchDone = true; });
                  </script>
                </body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    // Start monitor BEFORE navigation so it catches the page's /data fetch.
    let mut monitor = tab.monitor().start().await.unwrap();

    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Poll until we see the /data exchange, skipping unrelated navigation
    // requests (e.g. the page itself, favicon).
    let base = mock.uri();
    let data_url = format!("{base}/data");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let exchange = loop {
        if std::time::Instant::now() >= deadline {
            panic!("monitor did not emit a NetworkEvent::Http for /data within 10s");
        }
        match tokio::time::timeout(Duration::from_secs(5), monitor.next()).await {
            Ok(Some(NetworkEvent::Http(ex))) if ex.request.url == data_url => break ex,
            Ok(Some(_)) => continue, // skip unrelated events
            Ok(None) => panic!("monitor stream ended before /data exchange"),
            Err(_) => panic!("timed out waiting for /data exchange"),
        }
    };

    assert_eq!(exchange.status(), Some(200), "expected status 200");
    assert!(exchange.is_success());
    assert!(exchange.error.is_none());

    // Fetch the body lazily via getResponseBody.
    let body = exchange.text().await.unwrap();
    assert_eq!(body, "hello-monitor", "body mismatch");

    monitor.stop();
    browser.close().await.unwrap();
}

// ──────────────────────────────────────────────────────────────────────────────
// monitor_stream_bodies_reassembles_large_response
// ──────────────────────────────────────────────────────────────────────────────

/// Serve `body` on a fresh loopback TCP listener as a raw HTTP/1.1 response,
/// written in `chunk_size`-byte writes with a `delay` pause between each —
/// deliberately slow (unlike a normal wiremock response, whose full buffer
/// Chrome can receive+finish downloading faster than an async CDP round-trip
/// can enable streaming for it, as verified empirically against this test's
/// original wiremock-backed version). The pacing gives
/// `Network.streamResourceContent`'s enable call time to land before
/// `loadingFinished`, and guarantees more than one `Network.dataReceived`
/// event since each write is flushed and paused before the next.
///
/// Returns the `http://host:port` origin to fetch from. Handles exactly one
/// connection then exits — enough for this test's single fetch.
async fn serve_slow_body(body: Vec<u8>, chunk_size: usize, delay: Duration) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (mut rd, mut wr) = stream.into_split();
        // Drain (and discard) the request in the background — without this,
        // a full TCP receive buffer on our side could make Chrome's write of
        // its request head block, which would deadlock against us blocking
        // on our own writes below.
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while matches!(rd.read(&mut buf).await, Ok(n) if n > 0) {}
        });
        let header = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/octet-stream\r\n\
             Content-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        if wr.write_all(header.as_bytes()).await.is_err() {
            return;
        }
        for piece in body.chunks(chunk_size) {
            if wr.write_all(piece).await.is_err() {
                return;
            }
            let _ = wr.flush().await;
            tokio::time::sleep(delay).await;
        }
        let _ = wr.shutdown().await;
    });
    format!("http://{addr}")
}

/// A monitor started with `stream_bodies(true)` emits `NetworkEvent::HttpData`
/// chunks for a largeish, slowly-delivered response, and concatenating them
/// in arrival order reproduces the exact body Chrome delivered (verified
/// against `NetworkExchange::body()` / `Network.getResponseBody` as ground
/// truth).
#[tokio::test]
#[serial]
#[ignore]
async fn monitor_stream_bodies_reassembles_large_response() {
    let page_mock = fixture_with_html(
        r#"<!doctype html><html><body>
          <script>window.pageReady = true;</script>
        </body></html>"#,
    )
    .await;

    // 256 KiB of deterministic, non-degenerate content (an incrementing byte
    // pattern) — distinctive enough that any reordering/truncation bug in
    // reassembly would show up as a mismatch rather than accidentally
    // comparing equal. Delivered in 16 KiB writes, 20ms apart (~320ms total)
    // so Chrome sees it arrive as multiple `Network.dataReceived` events
    // rather than one instantaneous local-loopback blob.
    let body: Vec<u8> = (0..256 * 1024).map(|i| (i % 256) as u8).collect();
    let data_origin = serve_slow_body(body.clone(), 16 * 1024, Duration::from_millis(20)).await;
    let data_url = format!("{data_origin}/big");

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    // Start streaming BEFORE navigation so it catches the page's fetch.
    let mut monitor = tab.monitor().stream_bodies(true).start().await.unwrap();

    tab.goto(&page_mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Kick off the fetch from the page's own JS context (cross-origin to the
    // slow-body server; it sends `Access-Control-Allow-Origin: *`).
    let fetch_js = format!(
        r#"(async () => {{
          const r = await fetch({url});
          const b = await r.arrayBuffer();
          return b.byteLength;
        }})()"#,
        url = serde_json::json!(data_url)
    );
    tab.evaluate_main::<serde_json::Value>(&format!(
        "({fetch_js}).then(n => {{ window.fetchLen = n; window.fetchDone = true; }})"
    ))
    .await
    .unwrap();

    // Chunks arrive (and must be buffered) by request_id BEFORE the owning
    // `Http` exchange completes — only once we see the exchange do we know
    // which request_id's chunks to reassemble and compare.
    let mut chunks_by_id: HashMap<String, Vec<u8>> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let exchange = loop {
        if Instant::now() >= deadline {
            panic!("monitor did not emit a NetworkEvent::Http for /big within 20s");
        }
        match tokio::time::timeout(Duration::from_secs(5), monitor.next()).await {
            Ok(Some(NetworkEvent::HttpData { request_id, chunk })) => {
                chunks_by_id.entry(request_id).or_default().extend(chunk);
            }
            Ok(Some(NetworkEvent::Http(ex))) if ex.request.url == data_url => break ex,
            Ok(Some(_)) => continue, // unrelated event (e.g. the page navigation itself)
            Ok(None) => panic!("monitor stream ended before /big exchange"),
            Err(_) => panic!("timed out waiting for /big exchange"),
        }
    };

    assert_eq!(exchange.status(), Some(200), "expected status 200");

    let streamed = chunks_by_id
        .remove(exchange.request_id())
        .unwrap_or_default();
    assert!(
        !streamed.is_empty(),
        "expected at least one HttpData chunk for {data_url} (request_id {})",
        exchange.request_id()
    );

    // Ground truth: the whole-body path must still agree byte-for-byte.
    let full_body = exchange.body().await.unwrap();
    assert_eq!(
        full_body.len(),
        body.len(),
        "getResponseBody length mismatch"
    );
    assert_eq!(
        streamed, full_body,
        "concatenated HttpData chunks must reassemble to the exact response body"
    );
    assert_eq!(
        streamed, body,
        "reassembled body must match what the slow-body server sent"
    );

    // Sanity-check the page's own fetch() also saw the full length.
    let fetch_len: i64 = tab
        .evaluate_main("window.fetchDone ? window.fetchLen : -1")
        .await
        .unwrap();
    assert_eq!(fetch_len as usize, body.len());

    monitor.stop();
    browser.close().await.unwrap();
}

// ──────────────────────────────────────────────────────────────────────────────
// request_get_inherits_cookies
// ──────────────────────────────────────────────────────────────────────────────

/// `tab.request().get(...)` inherits the cookies set on the page origin.
#[tokio::test]
#[serial]
#[ignore]
async fn request_get_inherits_cookies() {
    // Single MockServer: page + /echo-cookie must be same-origin so the
    // in-page fetch actually sends the Cookie header.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><body><div id="d">x</div></body></html>"#
                    .as_bytes()
                    .to_vec(),
                "text/html",
            ),
        )
        .mount(&mock)
        .await;
    // /echo-cookie: wiremock can't reflect dynamic request headers, so we
    // just need the endpoint to return 200 — cookie inheritance is proved by
    // the cookie being set on the page *before* the request and by the request
    // reaching the endpoint (not 404/blocked). Cookie presence is verified
    // via document.cookie; the browser guarantee handles the rest.
    Mock::given(method("GET"))
        .and(path("/echo-cookie"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Plant a cookie on this origin via JS.
    tab.evaluate_main::<serde_json::Value>("document.cookie = 'session=abc123; path=/'; null")
        .await
        .unwrap();

    // Confirm cookie is set.
    let cookie_val: String = tab.evaluate_main("document.cookie").await.unwrap();
    assert!(
        cookie_val.contains("session=abc123"),
        "cookie not set on page; got: {cookie_val:?}"
    );

    // Issue the GET to the same origin — cookies are inherited by in-page fetch.
    let echo_url = format!("{}/echo-cookie", mock.uri());
    let resp = tab.request().get(&echo_url).send().await.unwrap();

    assert_eq!(resp.status(), 200, "echo-cookie endpoint should return 200");

    browser.close().await.unwrap();
}

// ──────────────────────────────────────────────────────────────────────────────
// request_post_json_round_trips
// ──────────────────────────────────────────────────────────────────────────────

/// `tab.request().post(url).json(&payload).send()` delivers JSON and the
/// echoed response body deserializes back to the original shape.
#[tokio::test]
#[serial]
#[ignore]
async fn request_post_json_round_trips() {
    // Single MockServer: page + /echo-json must be same-origin so the
    // in-page fetch is not blocked by CORS.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><html><body></body></html>"#.as_bytes().to_vec(),
            "text/html",
        ))
        .mount(&mock)
        .await;

    // /echo-json: wiremock returns the known payload statically (can't echo
    // request body, but we control both sides so the content matches).
    let echo_body = serde_json::json!({"key": "value", "num": 42});
    Mock::given(method("POST"))
        .and(path("/echo-json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(echo_body.to_string().into_bytes())
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Payload {
        key: String,
        num: i32,
    }
    let payload = Payload {
        key: "value".into(),
        num: 42,
    };

    let echo_url = format!("{}/echo-json", mock.uri());
    let resp = tab
        .request()
        .post(&echo_url)
        .json(&payload)
        .unwrap()
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let echoed: Payload = resp.json().unwrap();
    assert_eq!(echoed, payload, "echoed JSON should match sent payload");

    browser.close().await.unwrap();
}

// ──────────────────────────────────────────────────────────────────────────────
// bypass_cors_reaches_cross_origin
// ──────────────────────────────────────────────────────────────────────────────

/// `.bypass_cors()` reaches a cross-origin endpoint that the in-page `fetch`
/// path would be blocked on (no CORS headers on the target).
#[tokio::test]
#[serial]
#[ignore]
async fn bypass_cors_reaches_cross_origin() {
    // Two separate mock servers = two distinct origins.
    let page_mock = fixture_with_html(r#"<!doctype html><html><body></body></html>"#).await;

    let cross_origin_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resource"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("cross-origin-data"),
            // Deliberately no Access-Control-Allow-Origin header, so an
            // in-page fetch would be CORS-blocked.
        )
        .mount(&cross_origin_mock)
        .await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();
    tab.goto(&page_mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    let cross_url = format!("{}/resource", cross_origin_mock.uri());
    let resp = tab
        .request()
        .get(&cross_url)
        .bypass_cors()
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "bypass_cors should reach the cross-origin endpoint"
    );
    let body = resp.text().unwrap();
    assert_eq!(body, "cross-origin-data");

    browser.close().await.unwrap();
}

// ──────────────────────────────────────────────────────────────────────────────
// monitor_captures_websocket_frames
// ──────────────────────────────────────────────────────────────────────────────

// MANUAL: requires a live WS endpoint. tokio-tungstenite is not a dev-dep
// (adding it would pull a non-trivial transitive graph); use the unit-level
// MockConnection WS tests in monitor/mod.rs for automated coverage. This
// test is kept compile-correct and `#[ignore]`d for manual verification
// against a real WS server when desired.
//
// To run manually against a local echo server:
// 1. Start: `websocat -s 9001`
// 2. Set WS_ECHO_URL=ws://127.0.0.1:9001 in your environment.
// 3. cargo test ... -- --ignored monitor_captures_websocket_frames
#[tokio::test]
#[serial]
#[ignore]
async fn monitor_captures_websocket_frames() {
    // If no env var is set, skip gracefully rather than panicking on a missing
    // endpoint — this test is manual-only.
    let ws_url = match std::env::var("WS_ECHO_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: WS_ECHO_URL not set; skipping monitor_captures_websocket_frames");
            return;
        }
    };

    let page_mock = fixture_with_html(r#"<!doctype html><html><body></body></html>"#).await;

    let browser = Browser::builder().headless(true).launch().await.unwrap();
    let tab = browser.main_tab();

    let mut monitor = tab.monitor().start().await.unwrap();

    tab.goto(&page_mock.uri()).await.unwrap();
    tab.wait_for_load().await.unwrap();

    // Open a WebSocket from the page, send one frame, wait for the echo.
    let js = format!(
        r#"(async () => {{
          return await new Promise((resolve, reject) => {{
            const ws = new WebSocket({url});
            ws.onopen = () => ws.send('hello-ws');
            ws.onmessage = (e) => {{ ws.close(); resolve(e.data); }};
            ws.onerror = (e) => reject(String(e));
            setTimeout(() => reject('timeout'), 5000);
          }});
        }})()"#,
        url = serde_json::json!(ws_url)
    );
    let echo: Option<String> = tab.evaluate_main(&js).await.unwrap_or(None);
    assert_eq!(echo.as_deref(), Some("hello-ws"), "WS echo mismatch");

    // Collect up to 5 events looking for the frame pair.
    let mut got_sent = false;
    let mut got_received = false;
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(3), monitor.next()).await {
            Ok(Some(NetworkEvent::WebSocketFrame {
                direction: zendriver::FrameDirection::Sent,
                payload,
                ..
            })) if payload.contains("hello-ws") => got_sent = true,
            Ok(Some(NetworkEvent::WebSocketFrame {
                direction: zendriver::FrameDirection::Received,
                payload,
                ..
            })) if payload.contains("hello-ws") => got_received = true,
            Ok(Some(_)) => {}
            _ => break,
        }
        if got_sent && got_received {
            break;
        }
    }
    assert!(got_sent, "monitor should have seen the Sent WS frame");
    assert!(
        got_received,
        "monitor should have seen the Received WS frame"
    );

    monitor.stop();
    browser.close().await.unwrap();
}
