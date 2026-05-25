//! Transport bootstraps for rmcp.
//!
//! Stdio wiring lives inline in [`crate::server`] (one
//! `serve(stdio()).await` line — no helper earns its keep yet). The HTTP
//! transport mounts an `rmcp` [`StreamableHttpService`] onto axum; see
//! [`http::serve`].
//!
//! [`StreamableHttpService`]: rmcp::transport::streamable_http_server::StreamableHttpService

pub mod http;
