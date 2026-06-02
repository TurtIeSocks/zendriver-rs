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
pub mod eval;
#[cfg(feature = "expect")]
pub mod expect;
#[cfg(feature = "fetcher")]
pub mod fetcher;
pub mod find;
pub mod frames;
#[cfg(feature = "imperva")]
pub mod imperva;
#[cfg(feature = "interception")]
pub mod intercept;
pub mod lifecycle;
pub mod navigation;
pub mod reads;
pub mod snapshot;
pub mod stealth;
pub mod storage;
pub mod tabs;
