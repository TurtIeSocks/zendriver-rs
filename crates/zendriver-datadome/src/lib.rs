//! DataDome anti-bot bypass for `zendriver`.
//!
//! This crate is a sibling to `zendriver-cloudflare` and `zendriver-imperva`.
//! It detects which DataDome surface a tab is currently showing, observes the
//! page until the `datadome` clearance cookie lands (on device-check surfaces
//! the invisible JS interrogation resolves on its own), and escalates a CAPTCHA
//! surface (slider / puzzle / press-hold) to a caller-supplied solver callback.
//!
//! # Limitations
//!
//! A successful clearance means the `datadome` cookie landed in the browser —
//! it does **not** guarantee every subsequent request will be accepted.
//! DataDome runs an invisible device-check that scores the fingerprint the
//! browser emitted during the interstitial. If that score falls below
//! threshold the next request returns a fresh challenge regardless of the
//! cookie value.
//!
//! The most common root cause of a repeated challenge after apparent clearance
//! is the **WebGL / WebGPU fingerprint leak** (issue #20): an un-shimmed
//! `UNMASKED_RENDERER_WEBGL` extension string or `GPUAdapterInfo.device` field
//! exposes the real GPU and flags the session as a headless browser.
//!
//! Run with [`BrowserBuilder::stealth`] enabled to minimize fingerprint
//! surface. Without stealth the bypass will succeed locally but fail on
//! real DataDome-protected sites under scoring.
//!
//! [`BrowserBuilder::stealth`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.stealth

// Modules land in later tasks.
