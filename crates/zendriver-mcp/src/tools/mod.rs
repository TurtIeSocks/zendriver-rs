//! Per-category tool handler modules.
//!
//! Each submodule exposes free async fns taking
//! `Arc<Mutex<SessionState>>` + a typed input struct and returning a typed
//! output (or [`rmcp::ErrorData`]). The `#[tool_router(server_handler)]`
//! block in [`crate::server`] hosts thin `#[tool]` wrappers that delegate
//! to these — keeping the schema-bearing structs colocated with their
//! handlers makes it easy to grow new tool categories without bloating
//! `server.rs`.

pub mod actions;
#[cfg(feature = "cloudflare")]
pub mod cloudflare;
pub mod common;
pub mod cookies;
#[cfg(feature = "datadome")]
pub mod datadome;
pub mod download;
pub mod eval;
#[cfg(feature = "expect")]
pub mod expect;
#[cfg(feature = "fetcher")]
pub mod fetcher;
pub mod find;
#[cfg(feature = "fingerprints")]
pub mod fingerprints;
pub mod frames;
#[cfg(feature = "imperva")]
pub mod imperva;
#[cfg(feature = "interception")]
pub mod intercept;
pub mod lifecycle;
#[cfg(feature = "monitor")]
pub mod monitor;
pub mod mouse;
pub mod navigation;
pub mod pdf;
pub mod reads;
pub mod request;
pub mod scroll;
pub mod snapshot;
pub mod stealth;
pub mod storage;
pub mod tabs;
pub mod window;
