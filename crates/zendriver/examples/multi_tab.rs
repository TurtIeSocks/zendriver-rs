//! Open three tabs at distinct URLs, iterate the [`Browser::tabs`] registry,
//! and print every tab's URL — the canonical P4 multi-tab smoke test.
//!
//! Demonstrates:
//!   - [`Browser::new_tab_at`] (open + navigate in one step).
//!   - [`Browser::tabs`] (snapshot of every live tab the registrar tracks,
//!     including the auto-attached `main_tab`).
//!   - [`Browser::tab_count`] (cheap len read on the same registry).
//!
//! Each opened tab is its own session — closing the [`Browser`] tears them
//! all down via the shared Connection drop path.

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;

    // `main_tab()` is the about:blank tab Chrome auto-opens at launch.
    // Drive it to a real URL so the printout is interesting.
    let main = browser.main_tab();
    main.goto("https://example.com").await?;
    main.wait_for_load().await?;

    // Open two more tabs at distinct origins; each call returns a fully
    // initialised Tab (Page/DOM/Runtime/Network domains enabled, stealth
    // applied, isolated world ready).
    let tab_b = browser
        .new_tab_at("data:text/html,<!doctype html><title>B</title><h1>tab B</h1>")
        .await?;
    tab_b.wait_for_load().await?;

    let tab_c = browser
        .new_tab_at("data:text/html,<!doctype html><title>C</title><h1>tab C</h1>")
        .await?;
    tab_c.wait_for_load().await?;

    // Pull the live snapshot from the registry and walk it.
    let tabs = browser.tabs().await;
    println!("tab_count = {}", browser.tab_count().await);
    for (i, tab) in tabs.iter().enumerate() {
        let url = tab.url().await?;
        let title = tab.title().await?;
        println!("  [{i}] target={} url={url} title={title:?}", tab.target_id());
    }

    browser.close().await?;
    Ok(())
}
