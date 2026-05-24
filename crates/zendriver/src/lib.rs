//! zendriver — async, undetectable Chrome automation over CDP.
//!
//! Phase 1 public surface.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod browser;
pub mod cookies;
pub mod element;
pub mod error;
#[cfg(feature = "expect")]
pub mod expect;
pub mod frame;
pub mod input;
pub(crate) mod isolated_world;
pub mod network_idle;
pub mod query;
pub mod screenshot;
pub mod storage;
pub mod tab;
pub mod traits;

pub use browser::{Browser, BrowserBuilder};
pub use cookies::{Cookie, CookieJar, SameSite};
pub use element::actions::ClickOptions;
pub use element::Element;
pub use error::{BrowserError, Result, ZendriverError};
pub use frame::Frame;
pub use input::{Key, KeyModifiers, MouseButton, SpecialKey};
pub use query::{AriaRole, BoundingBox, FindBuilder};
pub use screenshot::{Format, ScreenshotBuilder};
pub use storage::Storage;
pub use tab::Tab;
pub use traits::{Evaluable, Queryable};

// Re-export selected transport types for advanced users.
pub use zendriver_transport::{CallError, Connection, SessionHandle, TransportError};

/// Network interception API re-exports.
///
/// Gated by the `interception` cargo feature. The full surface lives in the
/// `zendriver-interception` sub-crate; these aliases let downstream code
/// reach the types without depending on the sub-crate directly.
#[cfg(feature = "interception")]
pub use zendriver_interception::{
    AbortReason, InterceptBuilder, InterceptHandle, InterceptionError, PausedRequest, RequestInfo,
    RequestOverrides, RequestStage, ResourceType, ResponseInfo,
};

/// Cloudflare Turnstile bypass re-exports.
///
/// Gated by the `cloudflare` cargo feature. The driver lives in the
/// `zendriver-cloudflare` sub-crate; these aliases let downstream code reach
/// the types without depending on the sub-crate directly. Drive via
/// [`Tab::cloudflare`].
#[cfg(feature = "cloudflare")]
pub use zendriver_cloudflare::{ClearanceOutcome, CloudflareBypass, CloudflareError};

/// Re-export the shared `UrlMatcher` used by the `expect_*` helpers.
#[cfg(feature = "expect")]
pub use expect::UrlMatcher;

/// `expect_request` API re-exports.
#[cfg(feature = "expect")]
pub use expect::request::{MatchedRequest, RequestExpectation};

/// `expect_response` API re-exports.
#[cfg(feature = "expect")]
pub use expect::response::{MatchedResponse, ResponseExpectation};

/// `expect_dialog` API re-exports.
#[cfg(feature = "expect")]
pub use expect::dialog::{DialogExpectation, DialogType, MatchedDialog};

/// `expect_download` API re-exports.
#[cfg(feature = "expect")]
pub use expect::download::{
    DownloadExpectation, DownloadProgressState, DownloadState, MatchedDownload,
};

/// Chrome-for-Testing fetcher re-exports.
///
/// Gated by the `fetcher` cargo feature. The driver lives in the
/// `zendriver-fetcher` sub-crate; these aliases let downstream code reach the
/// types without depending on the sub-crate directly. Drive via
/// [`BrowserBuilder::ensure_chrome`] for the common "just download Chrome"
/// case, or instantiate [`Fetcher`] directly for version/channel/cache
/// customization.
#[cfg(feature = "fetcher")]
pub use zendriver_fetcher::{
    Channel, Fetcher, FetcherError, FetcherPhase, FetcherProgress, Platform, VersionSpec,
};

/// Stealth profile + fingerprint configuration re-exported from `zendriver-stealth`.
pub mod stealth {
    pub use zendriver_stealth::{Fingerprint, Platform, StealthProfile, UserAgentMetadata};
}

/// Convenience entry point: launch a Chrome instance with default settings.
///
/// Equivalent to `Browser::builder().launch().await`.
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// let browser = zendriver::start().await?;
/// let tab = browser.main_tab();
/// tab.goto("https://example.com").await?;
/// # Ok(()) }
/// ```
pub async fn start() -> Result<Browser> {
    Browser::builder().launch().await
}

#[cfg(test)]
#[allow(dead_code)]
mod auto_trait_assertions {
    //! Compile-time `Send + Sync` assertions for the public surface.
    //!
    //! If any of these stop compiling, a field was added to the named type
    //! whose auto traits don't cover `Send + Sync` — usually a `Rc` /
    //! `RefCell` / un-`Send`able future. Treat that as a design bug rather
    //! than a relaxation of the bounds: zendriver's whole point is to be
    //! ferried across `tokio::spawn` boundaries.
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_surface_is_send_sync() {
        assert_send_sync::<Browser>();
        assert_send_sync::<BrowserBuilder>();
        assert_send_sync::<Tab>();
        assert_send_sync::<Element>();
        assert_send_sync::<Frame>();
        assert_send_sync::<Storage>();
        assert_send_sync::<CookieJar>();
        assert_send_sync::<Cookie>();
        assert_send_sync::<SameSite>();
        assert_send_sync::<BoundingBox>();
        assert_send_sync::<AriaRole>();
        assert_send_sync::<Format>();
        assert_send_sync::<MouseButton>();
        assert_send_sync::<Key>();
        assert_send_sync::<SpecialKey>();
        assert_send_sync::<KeyModifiers>();
        assert_send_sync::<ClickOptions>();
        assert_send_sync::<ZendriverError>();
        assert_send_sync::<BrowserError>();
    }

    #[cfg(feature = "expect")]
    #[test]
    fn expect_surface_is_send_sync() {
        assert_send_sync::<UrlMatcher>();
        assert_send_sync::<MatchedRequest>();
        assert_send_sync::<RequestExpectation>();
        assert_send_sync::<MatchedResponse>();
        assert_send_sync::<ResponseExpectation>();
        assert_send_sync::<MatchedDialog>();
        assert_send_sync::<DialogExpectation>();
        assert_send_sync::<DialogType>();
        assert_send_sync::<MatchedDownload>();
        assert_send_sync::<DownloadExpectation>();
        assert_send_sync::<DownloadState>();
        assert_send_sync::<DownloadProgressState>();
    }
}
