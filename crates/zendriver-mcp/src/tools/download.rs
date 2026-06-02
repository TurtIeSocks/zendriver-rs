//! Direct-download tools — `browser_download`, `browser_set_download_path`.
//!
//! `browser_download` ports nodriver's page-driven fetch-and-save: it injects
//! a main-world script that `fetch`es the URL through the page's own network
//! context (cookies / referer / same-origin creds) and saves it via Chrome's
//! download behavior. It is **fire-and-forget** — it returns once the fetch is
//! dispatched, not when the file lands. For await/inspect/save-to-path
//! semantics use `browser_expect_register { kind: download, save_to }`.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::actions::AckOutput;
use crate::tools::common::current_tab;

/// Input for `browser_download`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadInput {
    /// URL to fetch + save. Fetched from the page's own network context.
    pub url: String,
    /// Saved file name within the tab's download directory. When omitted, the
    /// name is derived from the URL's last path segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Output of `browser_download`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DownloadOutput {
    /// Always `true` — the fetch was dispatched. The download itself runs
    /// asynchronously in Chrome.
    pub ok: bool,
    /// Echoed source URL.
    pub url: String,
    /// Echoed target filename, when one was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Initiate a page-driven download of `url`.
pub async fn download(
    state: Arc<Mutex<SessionState>>,
    input: DownloadInput,
) -> Result<DownloadOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.download_file(
        input.url.as_str(),
        input.filename.as_deref().map(PathBuf::from),
    )
    .await
    .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(DownloadOutput {
        ok: true,
        url: input.url,
        filename: input.filename,
    })
}

/// Input for `browser_set_download_path`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDownloadPathInput {
    /// Directory (on the MCP host) Chrome should save downloads into. Created
    /// if it does not exist.
    pub path: String,
}

/// Set the directory downloads are saved into for the current tab.
pub async fn set_download_path(
    state: Arc<Mutex<SessionState>>,
    input: SetDownloadPathInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    tab.set_download_path(PathBuf::from(&input.path))
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_with_no_browser_errors() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let err = download(
            state,
            DownloadInput {
                url: "https://example.com/x.bin".into(),
                filename: None,
            },
        )
        .await
        .expect_err("expected BrowserNotOpen");
        assert!(err.message.contains("Browser not open"));
    }
}
