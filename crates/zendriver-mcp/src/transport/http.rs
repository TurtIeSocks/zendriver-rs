//! Streamable HTTP transport — per-session [`ZendriverServer`] factory
//! mounted on axum.
//!
//! Each incoming MCP session triggers the factory, which mints a fresh
//! [`SessionState`] (and therefore a fresh `Browser` slot). That keeps
//! HTTP clients fully isolated — one client opening a browser does not
//! affect another's tab state.

use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio::sync::Mutex;

use crate::server::ZendriverServer;
use crate::state::{SessionState, StealthProfileChoice};

/// Bind a streamable HTTP MCP server on `addr` and serve until the process
/// is interrupted.
///
/// Each new MCP session is created with its own [`SessionState`] seeded
/// with `default_profile` — clients never share browser state.
pub async fn serve(
    addr: SocketAddr,
    default_profile: StealthProfileChoice,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = StreamableHttpService::new(
        move || {
            // Fresh SessionState per session keeps each client's Browser
            // / tabs isolated. The factory must be infallible — any
            // failure here would surface as an `io::Error` to the client
            // mid-handshake.
            let state = Arc::new(Mutex::new(SessionState {
                stealth_profile_choice: default_profile,
                ..SessionState::new()
            }));
            Ok(ZendriverServer { state })
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "zendriver-mcp HTTP listening on /mcp");
    axum::serve(listener, router).await?;
    Ok(())
}
