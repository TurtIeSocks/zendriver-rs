//! Real-device persona sources for zendriver-rs.
//!
//! # Features
//! - `pool` — download-on-first-use pool of real-device personas.
//! - `generative` — Bayesian network persona generator (C3, placeholder).
#[cfg(feature = "generative")]
pub mod generative;
#[cfg(feature = "pool")]
pub mod pool;
