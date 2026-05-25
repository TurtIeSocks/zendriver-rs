//! `zendriver-mcp` binary entry point.
//!
//! Parses CLI flags, sets up tracing (to stderr — never stdout while in
//! stdio MCP mode), constructs an [`Arc<Mutex<SessionState>>`], and
//! dispatches to the requested transport.

use std::sync::Arc;

use clap::{Parser, ValueEnum};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;
use zendriver_mcp::server;
use zendriver_mcp::state::{SessionState, StealthProfileChoice};

/// CLI surface for `zendriver-mcp`.
#[derive(Debug, Parser)]
#[command(name = "zendriver-mcp", version, about)]
struct Cli {
    /// Bind the streamable HTTP transport on this address
    /// (e.g. `127.0.0.1:8765`). Default: stdio.
    #[arg(long, value_name = "ADDR")]
    http: Option<String>,

    /// Default stealth profile for newly-opened browsers.
    #[arg(long, value_enum, default_value_t = StealthProfileArg::Auto)]
    stealth_profile: StealthProfileArg,

    /// Tracing log filter (see tracing-subscriber EnvFilter syntax).
    #[arg(long, default_value = "info")]
    log: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StealthProfileArg {
    Auto,
    Native,
    SpoofMacos,
    SpoofLinux,
    SpoofWindows,
}

impl From<StealthProfileArg> for StealthProfileChoice {
    fn from(v: StealthProfileArg) -> Self {
        match v {
            StealthProfileArg::Auto => Self::Auto,
            StealthProfileArg::Native => Self::Native,
            StealthProfileArg::SpoofMacos => Self::SpoofMacos,
            StealthProfileArg::SpoofLinux => Self::SpoofLinux,
            StealthProfileArg::SpoofWindows => Self::SpoofWindows,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log))
        // Never log to stdout in stdio MCP mode — that channel is reserved
        // for JSON-RPC frames.
        .with_writer(std::io::stderr)
        .init();

    let state = Arc::new(Mutex::new(SessionState {
        stealth_profile_choice: cli.stealth_profile.into(),
        ..SessionState::new()
    }));

    match cli.http {
        Some(_addr) => {
            // HTTP transport implementation lands in a follow-up dispatch.
            #[allow(clippy::unimplemented)]
            {
                unimplemented!("HTTP transport — next dispatch")
            }
        }
        None => server::run_stdio(state).await?,
    }
    Ok(())
}
