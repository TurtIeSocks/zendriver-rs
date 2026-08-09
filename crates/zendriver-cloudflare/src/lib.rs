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
//! 2. Scroll it into view, re-measure it, and dispatch a raw left-click at
//!    the 15% × 50% offset inside its box (the default Turnstile checkbox
//!    location), retrying a swallowed click a few times.
//! 3. Poll for either the token input gaining a value, or the challenge
//!    ceasing to be actionable — the clicked iframe no longer a valid click
//!    target, or every challenge marker gone. See
//!    [`ClearanceOutcome::ChallengeGone`] for why the first of those is not
//!    proof the gate was passed.
//!
//! **The markers and the click are caller data.** What to look for is
//! [`TurnstileSelectors`], whether and where to click is a [`ClickPolicy`],
//! and the click itself can be replaced wholesale via
//! [`CloudflareBypass::on_click`]. Cloudflare owns all of it and moves it on
//! their schedule; a caller should be able to follow without waiting on a
//! release of this crate.
//!
//! Two things stay fixed. **What counts as clickable** is the crate's rule,
//! not the caller's: a widget qualifies only if it has non-zero size and is
//! not hidden by `visibility` / `display` / `opacity`, and everything below
//! that bar — including a replacement [`on_click`](CloudflareBypass::on_click)
//! — is never reached. **How the widget is brought into view** is likewise
//! fixed, at a centred instant scroll.
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
//!     ClearanceOutcome::TimedOut { saw_challenge } => {
//!         println!("timed out; saw_challenge = {saw_challenge}")
//!     }
//! }
//! # Ok(()) }
//! ```

pub mod bypass;
pub mod click;
pub mod detection;
pub mod error;
mod js;
pub mod options;
#[cfg(test)]
mod testutil;

pub use bypass::{ClearanceOutcome, CloudflareBypass};
pub use detection::BoundingBox;
pub use error::CloudflareError;
pub use options::{ClickPolicy, ClickTarget, TurnstileSelectors};
