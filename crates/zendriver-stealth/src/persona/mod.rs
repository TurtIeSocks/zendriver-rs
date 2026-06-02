//! Persona: the unified fingerprint configuration.
pub mod seed;
pub mod specs;
pub mod surface;
pub use seed::Seed;
pub use specs::{FontSpec, HardwareSpec, SurfaceCfg, UaSpec, WebglSpec, WebrtcSpec};
