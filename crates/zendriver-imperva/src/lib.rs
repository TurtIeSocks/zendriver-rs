//! Imperva WAF / Incapsula bypass for `zendriver`.
//!
//! See the [Imperva chapter](https://turtiesocks.github.io/zendriver-rs/imperva.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, surface variants, and CAPTCHA-callback recipes.
//!
//! **Stealth required.** Imperva's reese84 sensor is itself a browser
//! fingerprint check. Run with [`BrowserBuilder::stealth`] enabled or this
//! bypass will fail on nearly all real Imperva-protected sites.
//!
//! Public API stub — modules land in subsequent tasks.
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth

pub mod detection;
pub mod error;

pub use detection::{
    CaptchaKind, CookieSnapshot, DetectionSnapshot, ImpervaSurface, detect_surface,
};
pub use error::ImpervaError;
