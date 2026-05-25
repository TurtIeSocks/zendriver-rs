//! Cloudflare Turnstile bypass for `zendriver`.
//!
//! See the [Cloudflare chapter](https://turtiesocks.github.io/zendriver-rs/cloudflare.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for end-to-end usage, timeout tuning, and detection-failure diagnostics.
//!
//! **Stealth recommended.** Cloudflare Turnstile is somewhat forgiving of
//! non-stealth Chrome, but `BrowserBuilder::stealth` significantly raises
//! the clearance success rate.
//!
//! Drives the Turnstile checkbox click flow:
//!
//! 1. Detect the Turnstile iframe via a shadow-DOM-aware walk of the page's
//!    main world.
//! 2. Dispatch a raw left-click at the 15% × 50% offset inside the iframe
//!    bbox (the canonical Turnstile checkbox location).
//! 3. Poll for either the `cf-turnstile-response` input gaining a token, or
//!    the challenge container disappearing entirely.
//!
//! Most users go through `zendriver`'s `Tab::cloudflare()` (feature-gated)
//! rather than constructing the bypass directly. The
//! [`CloudflareBypass`] type is the underlying driver.
//!
//! ```no_run
//! # async fn ex(tab: &zendriver_transport::SessionHandle)
//! #   -> Result<(), zendriver_cloudflare::CloudflareError> {
//! use std::time::Duration;
//! use zendriver_cloudflare::{CloudflareBypass, ClearanceOutcome};
//!
//! let outcome = CloudflareBypass::new(tab)
//!     .wait_for_clearance(Duration::from_secs(30))
//!     .await?;
//! match outcome {
//!     ClearanceOutcome::TokenAcquired(token) => println!("got token: {token}"),
//!     ClearanceOutcome::ChallengeGone => println!("challenge cleared"),
//! }
//! # Ok(()) }
//! ```

pub mod bypass;
pub mod click;
pub mod detection;
pub mod error;

pub use bypass::{ClearanceOutcome, CloudflareBypass};
pub use error::CloudflareError;
