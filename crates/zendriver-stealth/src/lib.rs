//! Anti-detection profiles and patches for `zendriver`.
//!
//! See the [Stealth chapter](https://turtiesocks.github.io/zendriver-rs/stealth.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for profile trade-offs, the sannysoft test matrix, and platform-spoofing
//! recipes.
//!
//! Three stealth modes, exposed via [`StealthProfile`]:
//!
//! - [`StealthProfile::off`] — no stealth (stock Chrome launch).
//! - [`StealthProfile::native`] — launch flags + UA scrub + CDP `Emulation`
//!   overrides. Safe against `Function.prototype.toString` probes; the
//!   default.
//! - [`StealthProfile::spoofed`] — `native` plus a Navigator-prototype JS
//!   bootstrap script that passes the [sannysoft][sannysoft] battery.
//!
//! Most users go through `zendriver`'s `BrowserBuilder::stealth(...)`:
//!
//! ```
//! use zendriver_stealth::{Platform, StealthProfile};
//!
//! let profile = StealthProfile::spoofed()
//!     .platform(Platform::MacIntel)
//!     .locale("en-US")
//!     .timezone("America/Los_Angeles");
//!
//! assert!(profile.bypass_csp_enabled());
//! assert!(!profile.build_flags().is_empty());
//! ```
//!
//! The [`StealthObserver`] type plugs into `zendriver-transport`'s observer
//! chain and applies the resolved [`Fingerprint`] / bootstrap to every newly
//! attached page target before Chrome releases the debugger.
//!
//! [sannysoft]: https://bot.sannysoft.com/

pub mod error;
pub mod fingerprint;
pub mod flags;
pub mod input_profile;
pub mod lang;
#[cfg(feature = "geo")]
pub mod geo;
pub mod observer;
pub mod patches;
pub mod persona;
pub mod profile;
pub mod ua;

// Re-exports added as types land in Tasks 1–13:
pub use error::StealthError;
pub use fingerprint::{Brand, Fingerprint, UserAgentMetadata};
pub use input_profile::InputProfile;
pub use lang::accept_language;
pub use observer::StealthObserver;
pub use profile::{Platform, ProfileKind, StealthProfile};

pub use persona::seed::Seed;
pub use persona::specs::{FontSpec, HardwareSpec, SurfaceCfg, UaSpec, WebglSpec, WebrtcSpec};
pub use persona::surface::{Strategy, Surface, SurfaceKind};
pub use persona::{JsProbe, Persona, PersonaBuilder};
