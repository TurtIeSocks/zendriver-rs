//! Internal transport layer for zendriver: WebSocket I/O, command/response
//! routing, event broadcast. Not a public API — re-exported selectively via
//! the `zendriver` crate.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod actor;
pub mod connection;
pub mod error;
pub mod frame;
pub mod observer;
pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

// Re-exports added as types land in later Phase 1 tasks:
pub use connection::Connection;
pub use error::{CallError, TransportError};
pub use frame::{CdpCommand, CdpInbound, CdpRpcError, RawEvent};
pub use observer::{ObserverError, PausedSession, TargetInfo, TargetObserver};
pub use session::SessionHandle;
