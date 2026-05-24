//! Load a page hosting a `srcdoc` iframe, enumerate every [`Frame`] the tab
//! tracks, then run a frame-scoped query inside the child to prove that
//! `Frame::find` resolves against the iframe's own document — not the
//! parent.
//!
//! Demonstrates:
//!   - [`Tab::frames`] (snapshot of main + every attached child).
//!   - [`Frame::is_main`] / [`Frame::id`] / [`Frame::url`] (frame metadata).
//!   - [`Frame::find`] scoped to the iframe's document context.
//!
//! Uses `srcdoc` so the iframe stays same-origin and routes through the
//! standard same-session frame path (no OOPIF needed for a self-contained
//! example).
//!
//! `Page.frameAttached` is delivered asynchronously after navigation
//! completes — the loop below polls the registry briefly until the child
//! frame shows up.

use std::time::{Duration, Instant};

use zendriver::Browser;

const PAGE_HTML: &str = "data:text/html,\
<!doctype html><html><body>\
<h1>parent</h1>\
<iframe id='f' srcdoc=\"<button id='b'>hello from iframe</button>\"></iframe>\
</body></html>";

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto(PAGE_HTML).await?;
    tab.wait_for_load().await?;

    // Wait for the child frame to register — polled because the
    // Page.frameAttached event fires after the parent's load completes.
    let deadline = Instant::now() + Duration::from_secs(5);
    let child = loop {
        let frames = tab.frames().await?;
        println!("frames so far: {}", frames.len());
        for f in &frames {
            println!(
                "  - id={} main={} url={:?}",
                f.id(),
                f.is_main(),
                f.url().await
            );
        }
        if let Some(child) = frames.into_iter().find(|f| !f.is_main()) {
            break child;
        }
        if Instant::now() >= deadline {
            return Err(zendriver::ZendriverError::Timeout(Duration::from_secs(5)));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Frame-scoped query: the button only exists in the iframe's document,
    // so this resolves only because `find` runs against the child's context.
    let btn = child.find().css("#b").one().await?;
    let text = btn.inner_text().await?;
    println!("iframe button text = {text:?}");

    browser.close().await?;
    Ok(())
}
