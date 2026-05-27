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
//! # Limitations
//!
//! This crate observes the page; it does not modify Imperva's response.
//! A [`ClearanceOutcome::TokenAcquired`] result means the JS challenge
//! completed and the `reese84` cookie landed — it does *not* guarantee
//! the next request will be accepted. Imperva runs a second validation
//! pass on every request that ships the token; if its scoring of the
//! fingerprint collected during the challenge falls below threshold,
//! the next request returns a 403 challenge page with `edet=15` /
//! `B15(x,y,z)` (general bot protection). Root causes for that are
//! upstream of this crate:
//!
//! - **Browser stealth gaps.** Missing fingerprint shim — canvas,
//!   audio, WebGL, client hints — leaks "automation" even when the
//!   challenge JS itself runs to completion.
//! - **IP reputation.** Residential / datacenter pools flagged at
//!   the edge return `B15` before the JS challenge runs at all.
//! - **UA-vs-binary drift.** Claiming Chrome 146 from a Chromium 148
//!   binary leaks JS-API behavior inconsistent with the claimed
//!   version.
//!
//! If [`ImpervaBypass::wait_for_clearance`] returns `TokenAcquired` but
//! subsequent requests still hit `edet=15`, look upstream — the
//! fingerprint the browser emitted is the problem, not the clearance
//! detection.
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
pub mod captcha;
pub mod detection;
pub mod error;
mod interception;

pub use bypass::{ClearanceOutcome, ImpervaBypass};
pub use captcha::{CaptchaChallenge, CaptchaSolution};
pub use detection::{
    CaptchaKind, CookieSnapshot, DetectionSnapshot, ImpervaSurface, detect_surface,
};
pub use error::ImpervaError;
