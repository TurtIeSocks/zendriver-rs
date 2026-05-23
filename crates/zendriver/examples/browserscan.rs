//! Port of `zendriver/examples/browserscan.py`.
//!
//! Visit browserscan's bot-detection page and save a full-viewport
//! screenshot to `browserscan.png` in the current working directory.
//!
//! Python uses `page.save_screenshot("browserscan.png")` which both captures
//! and writes. zendriver-rs's [`Tab::screenshot`] returns raw PNG bytes;
//! the caller is responsible for writing them. This matches the rest of
//! the Rust API ("return bytes, don't touch the filesystem").

use std::fs;

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://www.browserscan.net/bot-detection")
        .await?;
    tab.wait_for_load().await?;

    let png = tab.screenshot().await?;
    fs::write("browserscan.png", &png).expect("write browserscan.png");
    println!("wrote {} bytes to browserscan.png", png.len());

    browser.close().await?;
    Ok(())
}
