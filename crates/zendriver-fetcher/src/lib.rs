//! Chrome for Testing binary downloader.
//!
//! Resolves a [`VersionSpec`] + [`Platform`] pair against the
//! [Chrome for Testing manifest](https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json),
//! downloads the matching zip, extracts it into an atomic cache layout,
//! and hands back a path to the executable.
//!
//! Public entry point is [`Fetcher`]; progress is reported through
//! [`FetcherProgress`] callbacks tagged with a [`FetcherPhase`].

pub mod cache;
pub mod download;
pub mod error;
pub mod extract;
pub mod fetcher;
pub mod manifest;
pub mod platform;
pub mod version;

pub use error::FetcherError;
pub use fetcher::Fetcher;
pub use platform::Platform;
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
