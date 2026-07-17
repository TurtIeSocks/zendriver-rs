//! Internal CDP transport for `zendriver`: WebSocket I/O, command/response
//! routing, event broadcast.
//!
//! **Use via the `zendriver` crate's re-exports.** This crate is published so
//! the workspace can compile end-to-end but its surface is not covered by the
//! same SemVer guarantees as `zendriver`; expect minor versions to rearrange
//! types here freely. See [`SEMVER.md`] in the repo root for the policy.
//!
//! For a high-level walkthrough of the actor/observer model see the
//! [Architecture chapter](https://turtiesocks.github.io/zendriver-rs/architecture.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/).
//!
//! # What lives here
//!
//! - [`Connection`] — cheap, clonable handle to the actor task. All `Tab`s
//!   and `Element`s hold one.
//! - [`SessionHandle`] — a connection scoped to a particular CDP `sessionId`.
//! - [`CdpCommand`] / [`CdpInbound`] / [`CdpRpcError`] / [`RawEvent`] — wire
//!   types.
//! - [`AccountedRawEvent`] — opt-in loss-accounted alternative to
//!   `RawEvent`, delivered by
//!   [`Connection::subscribe_raw_accounted`](connection::Connection::subscribe_raw_accounted);
//!   reports lag, reconnects, and disconnects explicitly instead of silently
//!   dropping frames.
//! - [`TargetObserver`] — observer trait fired on `Target.attachedToTarget`
//!   before the debugger is released; used by `zendriver-stealth` to install
//!   patches on new pages.
//! - [`CallError`] / [`TransportError`] — error types surfaced via
//!   `zendriver`'s `ZendriverError::Transport` / `Cdp` variants.
//!
//! [`SEMVER.md`]: https://github.com/TurtIeSocks/zendriver-rs/blob/main/SEMVER.md

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
pub use connection::{
    Connection, DEFAULT_CALL_TIMEOUT, connect, connect_with_observers, spawn_actor,
    spawn_actor_with_observers,
};
pub use error::{CallError, TransportError};
pub use frame::{AccountedRawEvent, CdpCommand, CdpInbound, CdpRpcError, RawEvent};
pub use observer::{ObserverError, PausedSession, TargetInfo, TargetObserver};
pub use session::SessionHandle;
