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
//! Most users go through `zendriver`'s `Tab::imperva()` (feature-gated)
//! rather than constructing the bypass directly. The [`ImpervaBypass`]
//! type is the underlying driver.
//!
//! ```no_run
//! # async fn ex(tab: &zendriver_transport::SessionHandle)
//! #   -> Result<(), zendriver_imperva::ImpervaError> {
//! use std::time::Duration;
//! use zendriver_imperva::{ClearanceOutcome, ImpervaBypass};
//!
//! let outcome = ImpervaBypass::new(tab)
//!     .timeout(Duration::from_secs(30))
//!     .wait_for_clearance()
//!     .await?;
//! match outcome {
//!     ClearanceOutcome::TokenAcquired { reese84, .. } => {
//!         println!("token: {reese84}")
//!     }
//!     ClearanceOutcome::ChallengeGone => println!("legacy cleared"),
//!     ClearanceOutcome::AlreadyClear => println!("no challenge present"),
//! }
//! # Ok(()) }
//! ```
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth

pub mod bypass;
pub mod detection;
pub mod error;
mod interception;

pub use bypass::{CaptchaChallenge, CaptchaSolution, ClearanceOutcome, ImpervaBypass};
pub use detection::{
    CaptchaKind, CookieSnapshot, DetectionSnapshot, ImpervaSurface, detect_surface,
};
pub use error::ImpervaError;
