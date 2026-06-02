//! Async, undetectable Chrome automation over the Chrome DevTools Protocol.
//!
//! `zendriver` is the high-level browser-automation entry point built on a
//! CDP-over-WebSocket transport. The crate aims to feel like Playwright /
//! Puppeteer for Rust while staying explicit about its CDP underpinnings —
//! every public type maps cleanly to a CDP surface, every action has a
//! single-call escape hatch, and stealth is on by default.
//!
//! See the [user guide / mdBook](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end walkthroughs covering [installation][book-install],
//! [quickstart][book-quickstart], [stealth][book-stealth],
//! [multi-tab][book-multi-tab], [frames][book-frames],
//! [input][book-input], and migration guides from
//! [Playwright][book-mig-playwright] /
//! [zendriver (Python)][book-mig-zendriver] /
//! [nodriver (Python)][book-mig-nodriver].
//!
//! [book-install]: https://turtiesocks.github.io/zendriver-rs/install.html
//! [book-quickstart]: https://turtiesocks.github.io/zendriver-rs/quickstart.html
//! [book-stealth]: https://turtiesocks.github.io/zendriver-rs/stealth.html
//! [book-multi-tab]: https://turtiesocks.github.io/zendriver-rs/multi-tab.html
//! [book-frames]: https://turtiesocks.github.io/zendriver-rs/frames.html
//! [book-input]: https://turtiesocks.github.io/zendriver-rs/input.html
//! [book-mig-playwright]: https://turtiesocks.github.io/zendriver-rs/migration-playwright.html
//! [book-mig-zendriver]: https://turtiesocks.github.io/zendriver-rs/migration-zendriver-python.html
//! [book-mig-nodriver]: https://turtiesocks.github.io/zendriver-rs/migration-nodriver-python.html
//!
//! # Quickstart
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! let browser = zendriver::Browser::builder().launch().await?;
//! let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! let title: String = tab.evaluate_main("document.title").await?;
//! println!("{title}");
//! browser.close().await?;
//! # Ok(()) }
//! ```
//!
//! # Module layout
//!
//! - [`browser`] — process lifecycle ([`Browser`], [`BrowserBuilder`]).
//! - [`tab`] — per-page handle ([`Tab`]); the main interaction surface.
//! - [`frame`] — same-process + out-of-process iframes ([`Frame`]).
//! - [`element`] — DOM handle ([`Element`]) returned by [`query`] helpers.
//! - [`query`] — query builders, ARIA roles, actionability checks.
//! - [`cookies`] / [`storage`] — browser-scope cookie jar + per-tab storage.
//! - [`screenshot`] — screenshot builder (PNG / JPEG / WebP / full-page).
//! - [`input`] — keyboard / mouse / modifier state.
//! - [`error`] — [`ZendriverError`] + [`Result`] alias.
//! - [`traits`] — [`Queryable`] / [`Evaluable`] for code generic over
//!   Tab + Frame + Element.
//! - [`expect`] (feature `expect`) — Playwright-style `expect_*` helpers.
//!
//! # Feature flags
//!
//! | Flag | Crate | Purpose |
//! |------|-------|---------|
//! | `expect` | in-tree | `expect_request` / `expect_response` / `expect_dialog` / `expect_download` |
//! | `interception` | `zendriver-interception` | `Fetch.*`-backed request rewriting / abort |
//! | `cloudflare` | `zendriver-cloudflare` | Cloudflare Turnstile bypass |
//! | `imperva` | `zendriver-imperva` | Imperva WAF / Incapsula bypass |
//! | `fetcher` | `zendriver-fetcher` | Chrome-for-Testing download cache |
//!
//! Each gated module is re-exported here under `#[cfg(feature = "...")]` so
//! downstream code can `use zendriver::AbortReason` etc. without depending on
//! the sub-crate directly.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod browser;
pub mod browser_context;
pub mod cookies;
pub mod element;
pub mod error;
#[cfg(feature = "expect")]
pub mod expect;
pub(crate) mod expert;
pub mod frame;
pub mod input;
pub(crate) mod isolated_world;
pub mod network_idle;
pub mod pdf;
pub mod query;
pub mod screenshot;
pub mod storage;
pub mod tab;
pub mod traits;
pub(crate) mod url_matcher;
pub mod window;

pub use browser::{Browser, BrowserBuilder, Channel, PermissionType};
pub use browser_context::BrowserContext;
pub use cookies::{Cookie, CookieJar, CookiePriority, CookieSourceScheme, SameSite};
pub use element::Element;
pub use element::actions::ClickOptions;
pub use error::{BrowserError, Result, ZendriverError};
pub use frame::Frame;
pub use input::{Key, KeyModifiers, KeySequence, MouseButton, SpecialKey};
pub use pdf::PdfBuilder;
pub use query::{AriaRole, BoundingBox, FindBuilder, PageBox};
pub use screenshot::{Format, ScreenshotBuilder};
pub use storage::Storage;
pub use tab::{
    FrameResourceMatch, ReadyState, ReloadOptions, ScrollOptions, Tab, UserAgentOverride,
};
pub use traits::{Evaluable, Queryable};
pub use window::{WindowBounds, WindowState};

// Fingerprint-spoofing surface: the `Persona` config + per-surface render
// strategy knobs, re-exported from `zendriver-stealth` so callers can wire
// `Browser::builder().persona(...)` / `.surface(...)` without depending on the
// stealth crate directly.
pub use zendriver_stealth::{Persona, PersonaBuilder, Seed, Strategy, Surface};

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
    RequestOverrides, RequestStage, ResourceType, ResponseInfo, ResponseOverrides,
};

/// Cloudflare Turnstile bypass re-exports.
///
/// Gated by the `cloudflare` cargo feature. The driver lives in the
/// `zendriver-cloudflare` sub-crate; these aliases let downstream code reach
/// the types without depending on the sub-crate directly. Drive via
/// [`Tab::cloudflare`].
#[cfg(feature = "cloudflare")]
pub use zendriver_cloudflare::{ClearanceOutcome, CloudflareBypass, CloudflareError};

/// Imperva WAF / Incapsula bypass surface re-exports.
///
/// Gated by the `imperva` cargo feature. The driver lives in the
/// `zendriver-imperva` sub-crate; these aliases let downstream code reach
/// it via the parent crate without an extra dependency. Entry point is
/// [`Tab::imperva`].
#[cfg(feature = "imperva")]
pub use zendriver_imperva::{
    CaptchaChallenge, CaptchaKind, CaptchaSolution, CookieSnapshot, DetectionSnapshot,
    ImpervaBypass, ImpervaError, ImpervaSurface, detect_surface,
};

/// Imperva-bypass clearance outcome (aliased to avoid colliding with the
/// cloudflare crate's `ClearanceOutcome`).
#[cfg(feature = "imperva")]
pub use zendriver_imperva::ClearanceOutcome as ImpervaClearanceOutcome;

/// Re-export the shared `UrlMatcher` used by `expect_*` and `monitor`.
pub use url_matcher::UrlMatcher;

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
///
/// The fetcher's release-channel enum is re-exported as `FetcherChannel` to
/// avoid colliding with the browser-discovery [`Channel`] enum (Chrome /
/// Chromium / Brave / Edge / Auto): the two name different concepts — a
/// Chrome-for-Testing *release* channel (Stable/Beta/Dev/Canary) versus which
/// installed *browser* to launch.
#[cfg(feature = "fetcher")]
pub use zendriver_fetcher::{
    Channel as FetcherChannel, Fetcher, FetcherError, FetcherPhase, FetcherProgress, Platform,
    VersionSpec,
};

/// Stealth profile + fingerprint configuration re-exported from `zendriver-stealth`.
pub mod stealth {
    pub use zendriver_stealth::{
        Fingerprint, Persona, PersonaBuilder, Platform, Seed, StealthProfile, Strategy, Surface,
        UserAgentMetadata,
    };
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
        assert_send_sync::<Channel>();
        assert_send_sync::<PermissionType>();
        assert_send_sync::<Tab>();
        assert_send_sync::<Element>();
        assert_send_sync::<Frame>();
        assert_send_sync::<Storage>();
        assert_send_sync::<CookieJar>();
        assert_send_sync::<Cookie>();
        assert_send_sync::<SameSite>();
        assert_send_sync::<BoundingBox>();
        assert_send_sync::<PageBox>();
        assert_send_sync::<AriaRole>();
        assert_send_sync::<Format>();
        assert_send_sync::<MouseButton>();
        assert_send_sync::<Key>();
        assert_send_sync::<SpecialKey>();
        assert_send_sync::<KeyModifiers>();
        assert_send_sync::<ClickOptions>();
        assert_send_sync::<ReloadOptions>();
        assert_send_sync::<ScrollOptions>();
        assert_send_sync::<UserAgentOverride>();
        assert_send_sync::<ReadyState>();
        assert_send_sync::<FrameResourceMatch>();
        assert_send_sync::<WindowBounds>();
        assert_send_sync::<WindowState>();
        assert_send_sync::<ZendriverError>();
        assert_send_sync::<BrowserError>();
    }

    #[test]
    fn url_matcher_is_send_sync() {
        assert_send_sync::<UrlMatcher>();
    }

    #[cfg(feature = "expect")]
    #[test]
    fn expect_surface_is_send_sync() {
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

    #[cfg(feature = "interception")]
    #[test]
    fn interception_surface_is_send_sync() {
        assert_send_sync::<InterceptBuilder>();
        assert_send_sync::<InterceptHandle>();
        assert_send_sync::<InterceptionError>();
        assert_send_sync::<PausedRequest>();
        assert_send_sync::<RequestInfo>();
        assert_send_sync::<RequestOverrides>();
        assert_send_sync::<ResponseInfo>();
        assert_send_sync::<RequestStage>();
        assert_send_sync::<ResourceType>();
        assert_send_sync::<AbortReason>();
    }

    #[cfg(feature = "cloudflare")]
    #[test]
    fn cloudflare_surface_is_send_sync() {
        assert_send_sync::<CloudflareBypass>();
        assert_send_sync::<CloudflareError>();
        assert_send_sync::<ClearanceOutcome>();
    }

    #[cfg(feature = "imperva")]
    use crate::{
        CaptchaChallenge, CaptchaKind, CaptchaSolution, CookieSnapshot, DetectionSnapshot,
        ImpervaBypass, ImpervaClearanceOutcome, ImpervaError, ImpervaSurface,
    };

    #[cfg(feature = "imperva")]
    #[test]
    fn imperva_surface_is_send_sync() {
        assert_send_sync::<ImpervaBypass<'_>>();
        assert_send_sync::<ImpervaError>();
        assert_send_sync::<ImpervaSurface>();
        assert_send_sync::<CaptchaKind>();
        assert_send_sync::<CaptchaChallenge>();
        assert_send_sync::<CaptchaSolution>();
        assert_send_sync::<CookieSnapshot>();
        assert_send_sync::<DetectionSnapshot>();
        assert_send_sync::<ImpervaClearanceOutcome>();
    }

    #[cfg(feature = "fetcher")]
    #[test]
    fn fetcher_surface_is_send_sync() {
        assert_send_sync::<Fetcher>();
        assert_send_sync::<FetcherError>();
        assert_send_sync::<FetcherPhase>();
        assert_send_sync::<FetcherProgress>();
        assert_send_sync::<Platform>();
        assert_send_sync::<VersionSpec>();
        assert_send_sync::<FetcherChannel>();
    }

    #[test]
    fn stealth_surface_is_send_sync() {
        assert_send_sync::<stealth::Fingerprint>();
        assert_send_sync::<stealth::Platform>();
        assert_send_sync::<stealth::StealthProfile>();
        assert_send_sync::<stealth::UserAgentMetadata>();
    }
}
