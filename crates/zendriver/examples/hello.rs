//! Phase 1 exit example: launch Chrome, navigate to example.com, find <h1>,
//! print its text.

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let h1 = tab.find().css("h1").one().await?;
    let text = h1.inner_text().await?;
    println!("h1 text: {text}");

    browser.close().await?;
    Ok(())
}
