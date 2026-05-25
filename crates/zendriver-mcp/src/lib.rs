//! Internal library for the `zendriver-mcp` binary.
//!
//! Exposed primarily so integration tests can construct the server stack
//! directly without spawning the binary.

pub mod errors;
pub mod selectors;
pub mod server;
pub mod snapshot;
pub mod state;
pub mod tools;
pub mod transport;
