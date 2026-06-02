//! Shared helpers used by multiple tool modules.
//!
//! - [`EmptyInput`] — JSON-schema-bearing placeholder for tools that take no
//!   arguments.
//! - [`current_tab`] — resolves the current `zendriver::Tab` from a locked
//!   [`SessionState`]. Returns an owned (cheap-to-clone) handle so callers
//!   don't fight the borrow checker against `Browser::tabs(&self).await`'s
//!   `Vec<Tab>`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::errors::{McpServerError, map_error};
use crate::snapshot::html_trim;
use crate::state::SessionState;

/// Maximum blob size (bytes) returned inline as base64 before the caller is
/// required to pass a `save_path`. 5 MiB keeps a single tool result well
/// under typical MCP transport limits; larger captures (full-page PDFs,
/// downloads) must go to disk.
const MAX_INLINE_BYTES: usize = 5 * 1024 * 1024;

/// Placeholder input struct for tools with no arguments.
///
/// Required so rmcp can synthesize a JSON schema for the tool's args (an
/// absent arg block would yield `null` schema).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

/// Resolve the currently-focused tab.
///
/// Returns an owned [`zendriver::Tab`] — `Tab` is an `Arc`-backed handle so
/// cloning is cheap. The function takes ownership of the search rather than
/// borrowing into the `Vec<Tab>` from `Browser::tabs()` because that vec
/// gets dropped at the end of the function and we can't hand back a borrow.
///
/// # Errors
///
/// - [`McpServerError::BrowserNotOpen`] if `state.browser` is `None`.
/// - [`McpServerError::NoCurrentTab`] if no tab matches
///   `state.current_tab_id` (or `current_tab_id` is unset).
pub async fn current_tab(s: &SessionState) -> Result<zendriver::Tab, ErrorData> {
    let b = s
        .browser
        .as_ref()
        .ok_or_else(|| map_error(McpServerError::BrowserNotOpen))?;
    let id = s
        .current_tab_id
        .as_deref()
        .ok_or_else(|| map_error(McpServerError::NoCurrentTab))?;
    let tabs = b.tabs().await;
    tabs.into_iter()
        .find(|t| t.target_id() == id)
        .ok_or_else(|| map_error(McpServerError::NoCurrentTab))
}

/// Look up a [`zendriver::Frame`] on `tab` by id.
///
/// Surfaces [`ZendriverError::FrameNotFound`] through the standard error
/// pipeline (which adds a `browser_frame_list` suggestion) when no frame
/// matches. Shared by `eval`, `frames`, and `navigation`.
pub async fn lookup_frame(
    tab: &zendriver::Tab,
    frame_id: &str,
) -> Result<zendriver::Frame, ErrorData> {
    let frames = tab
        .frames()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    frames
        .into_iter()
        .find(|f| f.id() == frame_id)
        .ok_or_else(|| {
            map_error(McpServerError::from(
                zendriver::ZendriverError::FrameNotFound(frame_id.to_string()),
            ))
        })
}

/// Collect a trimmed snapshot of the current rendered HTML.
///
/// Uses `document.documentElement.outerHTML` (rather than CDP's
/// `Page.captureSnapshot`) so the result reflects post-script DOM mutations,
/// then drops `<script>` / `<style>` and collapses whitespace. Shared by the
/// tools (`scroll`, `mouse`) whose `return_snapshot` flag wants the same
/// shape `navigation` / `actions` produce.
pub async fn page_snapshot(tab: &zendriver::Tab) -> Result<String, ErrorData> {
    let html: String = tab
        .evaluate_main("document.documentElement.outerHTML")
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(html_trim::trim(&html))
}

/// MCP-layer keyboard-modifier flag, mapped onto [`zendriver::KeyModifiers`].
///
/// Shared by coordinate-mouse clicks (`browser_mouse`) and keyboard chords
/// (`browser_press` / `browser_key_sequence`).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModifierArg {
    /// `Alt` (Option on macOS).
    Alt,
    /// `Control`.
    Ctrl,
    /// `Meta` (Command on macOS, Windows key on Windows).
    Meta,
    /// `Shift`.
    Shift,
}

/// Fold a slice of [`ModifierArg`] into a [`zendriver::KeyModifiers`] bitset.
#[must_use]
pub fn modifiers_to_bits(mods: &[ModifierArg]) -> zendriver::KeyModifiers {
    let mut bits = zendriver::KeyModifiers::empty();
    for m in mods {
        bits |= match m {
            ModifierArg::Alt => zendriver::KeyModifiers::ALT,
            ModifierArg::Ctrl => zendriver::KeyModifiers::CTRL,
            ModifierArg::Meta => zendriver::KeyModifiers::META,
            ModifierArg::Shift => zendriver::KeyModifiers::SHIFT,
        };
    }
    bits
}

/// Structured result for tools that produce a binary blob (PDF, MHTML,
/// downloaded file).
///
/// When the caller passes a `save_path`, the bytes are written to disk on the
/// MCP server host and only `{ saved_path, byte_len }` is returned. Otherwise
/// the bytes are base64-inlined in `base64` (subject to [`MAX_INLINE_BYTES`]).
#[derive(Debug, Serialize, JsonSchema)]
pub struct BlobOutput {
    /// Absolute/!relative path the bytes were written to, when `save_path`
    /// was supplied. Path is on the MCP server host, not the client machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<String>,
    /// Size of the blob in bytes.
    pub byte_len: usize,
    /// Base64-encoded bytes. Populated only when no `save_path` was given and
    /// the blob is within the inline size limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}

/// Build a [`BlobOutput`] from raw bytes + an optional save path.
///
/// # Errors
///
/// - The file write fails (`save_path` set).
/// - The blob exceeds [`MAX_INLINE_BYTES`] and no `save_path` was given (the
///   error message directs the caller to set one).
pub fn blob_output(bytes: &[u8], save_path: Option<String>) -> Result<BlobOutput, ErrorData> {
    let byte_len = bytes.len();
    if let Some(path) = save_path {
        std::fs::write(&path, bytes)
            .map_err(|e| map_error(McpServerError::from(zendriver::ZendriverError::from(e))))?;
        return Ok(BlobOutput {
            saved_path: Some(path),
            byte_len,
            base64: None,
        });
    }
    if byte_len > MAX_INLINE_BYTES {
        return Err(ErrorData::invalid_params(
            format!(
                "Blob is {byte_len} bytes, over the {MAX_INLINE_BYTES}-byte inline limit. Pass `save_path` to write it to disk on the MCP server host instead of inlining it."
            ),
            None,
        ));
    }
    Ok(BlobOutput {
        saved_path: None,
        byte_len,
        base64: Some(BASE64.encode(bytes)),
    })
}
