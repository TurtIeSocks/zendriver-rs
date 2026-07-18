//! Demonstrates the network monitor API (`tab.monitor()`).
//!
//! Launches a browser, navigates to example.com, and runs a network monitor
//! that prints every HTTP exchange (method / URL / status) and WebSocket
//! frame observed while the page loads.
//!
//! The monitor is a [`futures::Stream`] over
//! [`zendriver::NetworkEvent`](zendriver::monitor::NetworkEvent) — it runs in
//! the background and delivers events as the browser fires them. Dropping the
//! monitor (or calling `.stop()`) cancels its background task.
//!
//! Requires the `monitor` cargo feature:
//! `cargo run --example network_monitor --features monitor`.

use futures::StreamExt;
use zendriver::Browser;
use zendriver::monitor::{FrameDirection, NetworkDeliveryBoundary, NetworkEvent};

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();

    // Start the monitor BEFORE navigating so no events are missed.
    // An optional URL pattern restricts events to matching URLs (substring
    // match). Omit `.url_pattern(...)` to observe all network activity.
    let mut monitor = tab.monitor().url_pattern("example.com").start().await?;

    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    // Drain events until the channel is empty (monitor still running but
    // nothing new is in flight). In a real application you would drive the
    // stream until a specific event arrives or a timeout fires.
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(500), monitor.next()).await
    {
        match event {
            NetworkEvent::Http(exchange) => {
                let status = exchange.response.as_ref().map_or_else(
                    || exchange.error.clone().unwrap_or_default(),
                    |r| r.status.to_string(),
                );
                println!(
                    "[HTTP] {} {} -> {}",
                    exchange.request.method, exchange.request.url, status
                );
            }
            NetworkEvent::WebSocketOpen { url, request_id } => {
                println!("[WS  ] open  id={request_id} url={url}");
            }
            NetworkEvent::WebSocketFrame {
                request_id,
                direction,
                opcode,
                payload,
            } => {
                let dir = match direction {
                    FrameDirection::Sent => "sent",
                    FrameDirection::Received => "recv",
                };
                println!("[WS  ] frame {dir} id={request_id} opcode={opcode} payload={payload:?}");
            }
            NetworkEvent::WebSocketClose { request_id } => {
                println!("[WS  ] close id={request_id}");
            }
            NetworkEvent::EventSourceMessage {
                request_id,
                event_name,
                data,
                ..
            } => {
                println!("[SSE ] id={request_id} event={event_name:?} data={data:?}");
            }
            // A delivery-loss boundary — a lagged/reconnected/disconnected
            // transport, a correlation-map eviction, or an undecodable
            // payload. Ignoring this variant is fine (every fully-observed
            // exchange above still arrives); printing it here just shows
            // where a real consumer would decide whether to resync, alert,
            // or restart the monitor. `Disconnected` in particular means
            // this monitor's correlator task has already ended — see
            // `NetworkDeliveryBoundary::Disconnected`.
            NetworkEvent::DeliveryBoundary(boundary) => {
                println!("[GAP ] {boundary:?}");
                if matches!(boundary, NetworkDeliveryBoundary::Disconnected { .. }) {
                    break;
                }
            }
        }
    }

    browser.close().await?;
    Ok(())
}
