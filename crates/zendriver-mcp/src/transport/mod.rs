//! Transport bootstraps for rmcp.
//!
//! In this slice, stdio wiring lives inline in [`crate::server`] (one
//! `serve(stdio()).await` line — no helper earns its keep yet). The HTTP
//! transport implementation lands in a follow-up dispatch.
