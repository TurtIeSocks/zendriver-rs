//! Per-category tool handler modules.
//!
//! Each submodule exposes free async fns taking
//! `Arc<Mutex<SessionState>>` + a typed input struct and returning a typed
//! output (or [`rmcp::ErrorData`]). The `#[tool_router(server_handler)]`
//! block in [`crate::server`] hosts thin `#[tool]` wrappers that delegate
//! to these — keeping the schema-bearing structs colocated with their
//! handlers makes it easy to grow new tool categories without bloating
//! `server.rs`.

pub mod common;
pub mod frames;
pub mod lifecycle;
pub mod navigation;
pub mod stealth;
pub mod tabs;
