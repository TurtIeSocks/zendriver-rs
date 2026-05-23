//! Anti-detection profiles and patches for zendriver.

pub mod error;
pub mod fingerprint;
pub mod flags;
pub mod input_profile;
pub mod observer;
pub mod patches;
pub mod profile;
pub mod ua;

// Re-exports added as types land in Tasks 1–13:
pub use error::StealthError;
pub use fingerprint::{Brand, Fingerprint, UserAgentMetadata};
pub use input_profile::InputProfile;
pub use observer::StealthObserver;
pub use profile::{Platform, ProfileKind, StealthProfile};
