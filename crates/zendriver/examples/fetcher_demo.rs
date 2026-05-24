//! Demonstrates the P5 [`Fetcher`] — Chrome for Testing binary downloader.
//!
//! [`Fetcher::new`] starts a builder; `.version(VersionSpec::Latest)`
//! pins the version selector to the newest Stable build in the CFT
//! manifest. `.on_progress(...)` registers a callback that fires through
//! every phase ([`FetcherPhase::Resolving`] → `Downloading` → `Extracting`
//! → `Verifying` → `Done`).
//!
//! [`Fetcher::ensure_chrome`] resolves the manifest, downloads + extracts
//! into the OS-conventional cache dir on a cache miss, and returns a
//! [`PathBuf`] to a runnable Chrome binary. On a cache hit (binary already
//! extracted under `<cache>/<version>/`), it skips the network entirely
//! and returns the cached path.
//!
//! After resolving the path, you can hand it to a [`Browser`] launch:
//! `Browser::builder().executable(path).launch().await?` — or use the
//! one-line shortcut `Browser::builder().ensure_chrome().await?.launch()`,
//! which wraps this call internally.
//!
//! Requires the `fetcher` cargo feature:
//! `cargo run --example fetcher_demo --features fetcher`.

use zendriver::{Fetcher, VersionSpec};

#[tokio::main]
#[allow(clippy::result_large_err)] // example boundary; users wrap in their own Error
async fn main() -> zendriver::Result<()> {
    tracing_subscriber::fmt::init();

    let path = Fetcher::new()
        .version(VersionSpec::Latest)
        .on_progress(|p| println!("{p:?}"))
        .ensure_chrome()
        .await?;

    println!("chrome binary: {}", path.display());

    Ok(())
}
