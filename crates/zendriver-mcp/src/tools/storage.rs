//! DOM storage handlers — `browser_storage_get / _set / _delete / _clear`.
//!
//! Every tool takes a [`StorageKind`] discriminator (`local` or `session`)
//! and routes to either [`zendriver::Tab::local_storage`] or
//! [`zendriver::Tab::session_storage`] on the session's current tab. CDP's
//! `DOMStorage` domain is per-origin, so a tab navigation between calls
//! switches the effective scope — the lib re-derives the origin on each
//! call (one cheap CDP round-trip), so handlers don't need to do anything
//! special to keep the origin coherent.
//!
//! Output for `_get` is a [`BTreeMap`] keyed by string — sorted output
//! gives agents a stable diff-friendly listing even though CDP itself
//! returns entries in `HashMap` order.

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::state::SessionState;
use crate::tools::common::current_tab;

// ---------- shared types --------------------------------------------------

/// Which DOM storage area to target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// `window.localStorage` (persists across tab restarts).
    Local,
    /// `window.sessionStorage` (cleared on tab close).
    Session,
}

/// Pick the right [`zendriver::Storage`] handle off the supplied tab.
///
/// Both accessors return cheap-to-clone `Arc`-backed handles, so we
/// construct one per call rather than caching.
fn pick_storage(tab: &zendriver::Tab, kind: StorageKind) -> zendriver::Storage {
    match kind {
        StorageKind::Local => tab.local_storage(),
        StorageKind::Session => tab.session_storage(),
    }
}

// ---------- browser_storage_get -------------------------------------------

/// Input for `browser_storage_get`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorageGetInput {
    /// Which storage area to read.
    pub kind: StorageKind,
    /// When set, returns only `{ key: value }` for that one key (empty map
    /// if the key is absent). When unset, returns the full storage area.
    #[serde(default)]
    pub key: Option<String>,
}

/// Output of `browser_storage_get`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StorageGetOutput {
    /// Storage entries, keyed lexicographically. Empty when the key is
    /// missing or the area is empty.
    pub values: BTreeMap<String, String>,
}

/// Read one key or the whole storage area for the session's current tab.
pub async fn storage_get(
    state: Arc<Mutex<SessionState>>,
    input: StorageGetInput,
) -> Result<StorageGetOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let storage = pick_storage(&tab, input.kind);
    let values = match input.key {
        Some(k) => {
            let v = storage
                .get(&k)
                .await
                .map_err(|e| map_error(McpServerError::from(e)))?;
            match v {
                Some(value) => BTreeMap::from([(k, value)]),
                None => BTreeMap::new(),
            }
        }
        None => storage
            .get_all()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?
            .into_iter()
            .collect(),
    };
    Ok(StorageGetOutput { values })
}

// ---------- browser_storage_set -------------------------------------------

/// Input for `browser_storage_set`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorageSetInput {
    /// Which storage area to write.
    pub kind: StorageKind,
    /// Key to set.
    pub key: String,
    /// Value to associate with `key`. Treated as opaque text by Chrome.
    pub value: String,
}

/// Output of `browser_storage_set`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StorageSetOutput {
    /// Always `true` when the call succeeded.
    pub ok: bool,
}

/// Insert or replace one key in the chosen storage area.
pub async fn storage_set(
    state: Arc<Mutex<SessionState>>,
    input: StorageSetInput,
) -> Result<StorageSetOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let storage = pick_storage(&tab, input.kind);
    storage
        .set(&input.key, &input.value)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(StorageSetOutput { ok: true })
}

// ---------- browser_storage_delete ----------------------------------------

/// Input for `browser_storage_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorageDeleteInput {
    /// Which storage area to mutate.
    pub kind: StorageKind,
    /// Key to remove. Missing keys are silently ignored (matches the
    /// Storage API `removeItem` contract).
    pub key: String,
}

/// Output of `browser_storage_delete`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StorageDeleteOutput {
    /// Always `true` when the call succeeded (no match-count signal from CDP).
    pub deleted: bool,
}

/// Remove one key from the chosen storage area.
pub async fn storage_delete(
    state: Arc<Mutex<SessionState>>,
    input: StorageDeleteInput,
) -> Result<StorageDeleteOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let storage = pick_storage(&tab, input.kind);
    // The lib calls this `remove`, not `delete`, to match the Storage API.
    storage
        .remove(&input.key)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(StorageDeleteOutput { deleted: true })
}

// ---------- browser_storage_clear -----------------------------------------

/// Input for `browser_storage_clear`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorageClearInput {
    /// Which storage area to empty.
    pub kind: StorageKind,
}

/// Output of `browser_storage_clear`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StorageClearOutput {
    /// Always `true` when the call succeeded.
    pub ok: bool,
}

/// Empty the chosen storage area for the current tab's origin.
pub async fn storage_clear(
    state: Arc<Mutex<SessionState>>,
    input: StorageClearInput,
) -> Result<StorageClearOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let storage = pick_storage(&tab, input.kind);
    storage
        .clear()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(StorageClearOutput { ok: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    #[tokio::test]
    async fn storage_get_with_no_browser_suggests_browser_open() {
        let err = storage_get(
            fresh(),
            StorageGetInput {
                kind: StorageKind::Local,
                key: None,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn storage_set_with_no_browser_suggests_browser_open() {
        let err = storage_set(
            fresh(),
            StorageSetInput {
                kind: StorageKind::Local,
                key: "k".into(),
                value: "v".into(),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn storage_delete_with_no_browser_suggests_browser_open() {
        let err = storage_delete(
            fresh(),
            StorageDeleteInput {
                kind: StorageKind::Session,
                key: "k".into(),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn storage_clear_with_no_browser_suggests_browser_open() {
        let err = storage_clear(
            fresh(),
            StorageClearInput {
                kind: StorageKind::Local,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[test]
    fn storage_kind_round_trips_serde_snake_case() {
        let local: StorageKind =
            serde_json::from_value(serde_json::json!("local")).expect("parse local");
        assert_eq!(local, StorageKind::Local);
        let session: StorageKind =
            serde_json::from_value(serde_json::json!("session")).expect("parse session");
        assert_eq!(session, StorageKind::Session);
        assert_eq!(
            serde_json::to_value(StorageKind::Local).unwrap(),
            serde_json::json!("local")
        );
        assert_eq!(
            serde_json::to_value(StorageKind::Session).unwrap(),
            serde_json::json!("session")
        );
    }
}
