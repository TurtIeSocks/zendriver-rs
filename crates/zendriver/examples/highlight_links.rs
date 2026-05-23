//! Inspired by the Python ecosystem's `highlight_links.py` pattern: collect
//! every `<a>` on a page, print its href, and outline it in red via
//! [`Element::evaluate`] so a human watching the headless run can see what
//! was selected.
//!
//! Demonstrates P3 surface: [`Tab::find_all`] + per-element
//! [`Element::attr`] reads + isolated-world JS execution per element.

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let links = tab.find_all().css("a").many_or_empty().await?;
    println!("found {} links", links.len());

    for link in &links {
        let href = link.attr("href").await?.unwrap_or_default();
        let text = link.inner_text().await?;
        println!("  {text:?} -> {href}");

        // Outline the link in red. `evaluate` runs in the isolated world
        // bound to this element handle, so the page can't intercept the
        // style write via a monkeypatched HTMLElement.prototype setter.
        link.evaluate::<()>("el.style.outline = '2px solid red'")
            .await?;
    }

    browser.close().await?;
    Ok(())
}
