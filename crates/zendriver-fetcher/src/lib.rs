//! Chromium binary downloader.
//!
//! See the [Fetcher chapter](https://turtiesocks.github.io/zendriver-rs/fetcher.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for cache-layout details and CI integration tips.
//!
//! Resolves a [`Distribution`] + [`VersionSpec`] + [`Platform`] triple against
//! that distribution's index, downloads the matching archive, unpacks it into
//! an atomic cache layout, and hands back a path to the executable.
//!
//! Public entry point is [`Fetcher`]; progress is reported through
//! [`FetcherProgress`] callbacks tagged with a [`FetcherPhase`].
//!
//! ```no_run
//! # async fn ex() -> Result<(), zendriver_fetcher::FetcherError> {
//! use zendriver_fetcher::{Fetcher, VersionSpec};
//!
//! let chrome = Fetcher::new()
//!     .version(VersionSpec::Latest)
//!     .ensure_chrome()
//!     .await?;
//! println!("Chrome ready at {}", chrome.display());
//! # Ok(()) }
//! ```
//!
//! # Distributions
//!
//! [`Distribution::default()`] is [`Distribution::ChromeForTesting`], so the
//! example above fetches exactly what this crate always has. Two further
//! indexes are available, and *only* resolution differs between them —
//! download, integrity check, unpack and cache are one shared path:
//!
//! | [`Distribution`] | Index | Keyed by |
//! |---|---|---|
//! | [`ChromeForTesting`](Distribution::ChromeForTesting) | one [JSON manifest][cft-manifest] | version |
//! | [`UngoogledChromium`](Distribution::UngoogledChromium) | three GitHub repos, one per OS | version (tag prefix) |
//! | [`ChromiumSnapshot`](Distribution::ChromiumSnapshot) | a GCS bucket per platform | **revision** |
//!
//! ```no_run
//! # async fn ex() -> Result<(), zendriver_fetcher::FetcherError> {
//! use zendriver_fetcher::{Distribution, Fetcher, VersionSpec};
//!
//! let chromium = Fetcher::new()
//!     .distribution(Distribution::UngoogledChromium)
//!     .version(VersionSpec::Explicit("151.0.7922.71".into()))
//!     .ensure_chrome()
//!     .await?;
//! # let _ = chromium; Ok(()) }
//! ```
//!
//! Availability is a per-platform question — the three ungoogled repos
//! release independently, and the snapshot bucket has no version index at all
//! — so [`list_builds`] answers it for the platform you actually want instead
//! of assuming parity.
//!
//! [cft-manifest]: https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json

pub mod archive;
pub mod cache;
pub mod distribution;
pub mod download;
pub mod error;
pub mod extract;
pub mod fetcher;
pub mod list;
pub mod manifest;
pub mod platform;
pub mod resolver;
pub mod tls;
pub mod version;

pub use distribution::Distribution;
pub use error::FetcherError;
pub use fetcher::Fetcher;
pub use list::list_builds;
pub use platform::Platform;
pub use resolver::Build;
pub use version::{Channel, VersionSpec};

/// Lifecycle phase of an in-flight fetch.
///
/// Reported via [`FetcherProgress::phase`] so callers can drive a TUI
/// or log stage-by-stage progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetcherPhase {
    /// Resolving version + platform against the CFT manifest.
    Resolving,
    /// Streaming bytes from the CFT CDN.
    Downloading,
    /// Unzipping the downloaded archive.
    Extracting,
    /// Verifying integrity (SHA256, executable bit).
    Verifying,
    /// All work complete; binary available at the returned path.
    Done,
}

/// Progress snapshot emitted by an in-flight fetch.
#[derive(Debug, Clone)]
pub struct FetcherProgress {
    /// Bytes written so far for the current phase.
    pub downloaded: u64,
    /// Total bytes expected for the current phase, when known
    /// (e.g. from the `Content-Length` header during download).
    pub total: Option<u64>,
    /// Current phase.
    pub phase: FetcherPhase,
}
