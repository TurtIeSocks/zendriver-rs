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

/// Re-export the shared `UrlMatcher` used by the `expect_*` helpers.
#[cfg(feature = "expect")]
pub use expect::UrlMatcher;

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
