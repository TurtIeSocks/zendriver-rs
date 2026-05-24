//! Cloudflare Turnstile bypass for zendriver.
//!
//! Drives the Turnstile checkbox click flow by detecting the challenge
//! iframe in shadow DOM, dispatching a raw mouse click, and polling for
//! the resulting clearance token.

pub mod bypass;
pub mod click;
pub mod detection;
pub mod error;

pub use bypass::{CloudflareBypass, ClearanceOutcome};
pub use error::CloudflareError;
