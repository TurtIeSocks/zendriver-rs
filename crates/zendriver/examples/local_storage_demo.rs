//! Drive a tab's `window.localStorage` end-to-end through the P4 [`Storage`]
//! API: set three keys, read them all back, clear, then assert empty.
//!
//! Demonstrates:
//!   - [`Tab::local_storage`] (per-tab handle — DOMStorage routes by origin
//!     not tab, but the handle borrows the tab's session for command
//!     scoping).
//!   - [`Storage::set`], [`Storage::get_all`], [`Storage::clear`].
//!
//! DOMStorage requires a real origin — it rejects requests against
//! `about:blank` and `data:` URLs (their origin is the opaque "null"
//! origin). The example navigates to `https://example.com` first so the
//! store has somewhere to live.

use zendriver::Browser;

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let browser = Browser::builder().headless(true).launch().await?;
    let tab = browser.main_tab();
    tab.goto("https://example.com").await?;
    tab.wait_for_load().await?;

    let storage = tab.local_storage();
    storage.set("theme", "dark").await?;
    storage.set("lang", "en").await?;
    storage.set("flag", "enabled").await?;

    let all = storage.get_all().await?;
    println!("after 3 sets, storage holds {} keys:", all.len());
    // HashMap iteration order is not stable — sort for deterministic output.
    let mut entries: Vec<_> = all.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in entries {
        println!("  {k} = {v}");
    }
    assert_eq!(
        all.len(),
        3,
        "expected 3 keys after sets, got {}",
        all.len()
    );

    storage.clear().await?;
    let empty = storage.get_all().await?;
    println!("after clear, storage holds {} keys", empty.len());
    assert!(empty.is_empty(), "storage should be empty after clear");

    browser.close().await?;
    Ok(())
}
