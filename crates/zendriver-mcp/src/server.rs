//! rmcp server stack + tool router.
//!
//! Every `#[tool]` method on [`ZendriverServer`] is a one-liner that
//! delegates to a free async fn in [`crate::tools`]. The
//! [`#[tool_router(server_handler)]`][tr] form emits the `ServerHandler`
//! impl in one step — no separate `#[tool_handler]` block needed.
//!
//! Keeping the per-tool implementations in `tools/*.rs` (rather than
//! inline here) makes the surface easy to grow: a new tool group adds a
//! new module, a new wrapper here, and lands without touching the others.
//!
//! [tr]: rmcp::tool_router

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::transport::stdio;
use rmcp::{ErrorData, Json, ServiceExt, tool, tool_router};
use tokio::sync::Mutex;

use crate::state::SessionState;
use crate::tools::common::EmptyInput;
use crate::tools::{
    actions, cookies, eval, find, frames, lifecycle, navigation, reads, snapshot, stealth, storage,
    tabs,
};

/// rmcp handler carrying the per-session [`SessionState`].
///
/// Clone is cheap (the only field is an `Arc<Mutex<_>>`).
#[derive(Clone)]
pub struct ZendriverServer {
    pub state: Arc<Mutex<SessionState>>,
}

#[tool_router(server_handler)]
impl ZendriverServer {
    /// Construct a server bound to the given session state.
    pub fn new(state: Arc<Mutex<SessionState>>) -> Self {
        Self { state }
    }

    // ---------- lifecycle ------------------------------------------------

    /// Launch Chrome with stealth defaults. Errors if a browser is
    /// already open in this session — call `browser_close` first.
    #[tool(
        name = "browser_open",
        description = "Launch Chrome with stealth defaults. Errors if a browser is already open in this session — call `browser_close` first."
    )]
    pub async fn browser_open(
        &self,
        Parameters(input): Parameters<lifecycle::OpenInput>,
    ) -> Result<Json<lifecycle::OpenOutput>, ErrorData> {
        lifecycle::open(self.state.clone(), input).await.map(Json)
    }

    /// Close the open browser. Idempotent — no error if no browser is open.
    #[tool(
        name = "browser_close",
        description = "Close the open browser. Idempotent — no error if no browser is open."
    )]
    pub async fn browser_close(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<lifecycle::CloseOutput>, ErrorData> {
        lifecycle::close(self.state.clone(), input).await.map(Json)
    }

    /// Report open browser + current tab + stealth profile.
    #[tool(
        name = "browser_status",
        description = "Report open browser + current tab + stealth profile."
    )]
    pub async fn browser_status(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<lifecycle::StatusOutput>, ErrorData> {
        lifecycle::status(self.state.clone(), input).await.map(Json)
    }

    // ---------- navigation -----------------------------------------------

    /// Navigate the current tab to a URL.
    #[tool(
        name = "browser_goto",
        description = "Navigate the current tab to a URL. `wait_for` selects load / idle / no wait."
    )]
    pub async fn browser_goto(
        &self,
        Parameters(input): Parameters<navigation::GotoInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::goto(self.state.clone(), input).await.map(Json)
    }

    /// Step back one entry in the current tab's history.
    #[tool(
        name = "browser_back",
        description = "Step back one entry in the current tab's history."
    )]
    pub async fn browser_back(
        &self,
        Parameters(input): Parameters<navigation::HistoryInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::back(self.state.clone(), input).await.map(Json)
    }

    /// Step forward one entry in the current tab's history.
    #[tool(
        name = "browser_forward",
        description = "Step forward one entry in the current tab's history."
    )]
    pub async fn browser_forward(
        &self,
        Parameters(input): Parameters<navigation::HistoryInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::forward(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Reload the current tab.
    #[tool(name = "browser_reload", description = "Reload the current tab.")]
    pub async fn browser_reload(
        &self,
        Parameters(input): Parameters<navigation::HistoryInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::reload(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Wait until the current tab's network has been idle for ~500ms,
    /// bounded by `timeout_ms`.
    #[tool(
        name = "browser_wait_for_idle",
        description = "Wait until the current tab's network has been idle for ~500ms, bounded by `timeout_ms`."
    )]
    pub async fn browser_wait_for_idle(
        &self,
        Parameters(input): Parameters<navigation::IdleInput>,
    ) -> Result<Json<navigation::IdleOutput>, ErrorData> {
        navigation::wait_for_idle(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- tabs -----------------------------------------------------

    /// Enumerate all live tabs.
    #[tool(
        name = "browser_tab_list",
        description = "Enumerate all live tabs. Each entry includes the CDP target id, URL, title, and an `is_current` flag relative to the session's focused tab."
    )]
    pub async fn browser_tab_list(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<tabs::TabListOutput>, ErrorData> {
        tabs::list(self.state.clone(), input).await.map(Json)
    }

    /// Open a new tab.
    #[tool(
        name = "browser_tab_new",
        description = "Open a new tab. `url` selects the initial URL (defaults to about:blank). When `activate` is true (the default) the new tab becomes the session's current tab."
    )]
    pub async fn browser_tab_new(
        &self,
        Parameters(input): Parameters<tabs::TabNewInput>,
    ) -> Result<Json<tabs::TabSummary>, ErrorData> {
        tabs::new_tab(self.state.clone(), input).await.map(Json)
    }

    /// Switch the session's current tab.
    #[tool(
        name = "browser_tab_switch",
        description = "Switch the session's current tab to the given `tab_id`. Use `browser_tab_list` to enumerate available ids."
    )]
    pub async fn browser_tab_switch(
        &self,
        Parameters(input): Parameters<tabs::TabSwitchInput>,
    ) -> Result<Json<tabs::TabSummary>, ErrorData> {
        tabs::switch(self.state.clone(), input).await.map(Json)
    }

    /// Close a tab.
    #[tool(
        name = "browser_tab_close",
        description = "Close a tab. When `tab_id` is omitted, the session's current tab is closed; if that was the focused tab, focus falls back to one of the remaining tabs (or `None` if none remain)."
    )]
    pub async fn browser_tab_close(
        &self,
        Parameters(input): Parameters<tabs::TabCloseInput>,
    ) -> Result<Json<tabs::TabCloseOutput>, ErrorData> {
        tabs::close(self.state.clone(), input).await.map(Json)
    }

    /// Bring a tab to the foreground.
    #[tool(
        name = "browser_tab_activate",
        description = "Bring `tab_id` to the foreground in Chrome (sends Target.activateTarget). Also updates the session's current tab so subsequent calls target the foregrounded tab."
    )]
    pub async fn browser_tab_activate(
        &self,
        Parameters(input): Parameters<tabs::TabActivateInput>,
    ) -> Result<Json<tabs::TabActivateOutput>, ErrorData> {
        tabs::activate(self.state.clone(), input).await.map(Json)
    }

    // ---------- frames ---------------------------------------------------

    /// Enumerate all frames on the current tab.
    #[tool(
        name = "browser_frame_list",
        description = "Enumerate all frames on the current tab. Each entry includes the frame id, URL, parent id (None for the main frame), optional `name`, and `is_main` flag."
    )]
    pub async fn browser_frame_list(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<frames::FrameListOutput>, ErrorData> {
        frames::list(self.state.clone(), input).await.map(Json)
    }

    // ---------- stealth --------------------------------------------------

    /// Configure the session's default stealth profile.
    #[tool(
        name = "browser_set_stealth_profile",
        description = "Configure the session's default stealth profile. NOTE: takes effect on the NEXT `browser_open` call; does NOT re-fingerprint an already-open browser. Call `browser_close` + `browser_open` to apply live."
    )]
    pub async fn browser_set_stealth_profile(
        &self,
        Parameters(input): Parameters<stealth::SetStealthProfileInput>,
    ) -> Result<Json<stealth::SetStealthProfileOutput>, ErrorData> {
        stealth::set_stealth_profile(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- find -----------------------------------------------------

    /// Resolve a Selector to one element and return its descriptor.
    #[tool(
        name = "browser_find",
        description = "Resolve a Selector to a single element on the current tab. Returns `{ found: false, element: null }` when no element matches within the selector's timeout (instead of an error) — agents can branch on existence without try/catch."
    )]
    pub async fn browser_find(
        &self,
        Parameters(input): Parameters<find::FindInput>,
    ) -> Result<Json<find::FindOutput>, ErrorData> {
        find::find(self.state.clone(), input).await.map(Json)
    }

    /// Resolve a Selector to ALL matches (up to `limit`).
    #[tool(
        name = "browser_find_all",
        description = "Resolve a Selector to ALL matching elements on the current tab (up to `limit`, default 50). `{ elements: [] }` is returned when nothing matches — never an error."
    )]
    pub async fn browser_find_all(
        &self,
        Parameters(input): Parameters<find::FindAllInput>,
    ) -> Result<Json<find::FindAllOutput>, ErrorData> {
        find::find_all(self.state.clone(), input).await.map(Json)
    }

    // ---------- reads ----------------------------------------------------

    /// Resolve a Selector and report selected state fields.
    #[tool(
        name = "browser_element_state",
        description = "Inspect a single element's state. `include` picks which fields to populate (default `all`). `in_viewport` is reserved for v1 and always returns null. Missing-element returns `{ exists: false }` rather than an error."
    )]
    pub async fn browser_element_state(
        &self,
        Parameters(input): Parameters<reads::ElementStateInput>,
    ) -> Result<Json<reads::ElementState>, ErrorData> {
        reads::element_state(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- actions --------------------------------------------------

    /// Click an element with realistic Bezier-path cursor approach.
    #[tool(
        name = "browser_click",
        description = "Click an element (realistic Bezier-path cursor + full actionability gate). Set `button` to `middle` / `right` for non-primary buttons or `click_count: 2` for a double-click. `return_snapshot: true` includes the post-click trimmed page HTML."
    )]
    pub async fn browser_click(
        &self,
        Parameters(input): Parameters<actions::ClickInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::click(self.state.clone(), input).await.map(Json)
    }

    /// Hover the cursor over an element's bbox center.
    #[tool(
        name = "browser_hover",
        description = "Hover the cursor over an element's bbox center via a realistic Bezier-interpolated mouse path. Common pre-step for revealing dropdown menus / tooltips."
    )]
    pub async fn browser_hover(
        &self,
        Parameters(input): Parameters<actions::HoverInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::hover(self.state.clone(), input).await.map(Json)
    }

    /// Type text into an element with realistic per-character timing.
    #[tool(
        name = "browser_type",
        description = "Type `text` into an element with realistic per-character timing (occasional typos + thinking pauses per the active stealth profile). When `clear_first: true`, the element's value is reset before typing — useful for replacing pre-filled inputs."
    )]
    pub async fn browser_type(
        &self,
        Parameters(input): Parameters<actions::TypeInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::type_text(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Focus an element + dispatch a single keystroke.
    #[tool(
        name = "browser_press",
        description = "Focus an element + dispatch a single keystroke. `key` accepts a special-key name (Enter, Tab, Escape, Backspace, Delete, ArrowUp/Down/Left/Right, Space, Home, End, PageUp, PageDown, F1..F12, etc., case-insensitive) OR a single character (typed as `Key::Char`)."
    )]
    pub async fn browser_press(
        &self,
        Parameters(input): Parameters<actions::PressInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::press(self.state.clone(), input).await.map(Json)
    }

    /// Set an element's `value` directly + fire `input`/`change` events.
    #[tool(
        name = "browser_set_value",
        description = "Set an element's `value` directly + fire bubbled `input` and `change` events. Faster than `browser_type` when keystroke realism doesn't matter, but still routes through the event handlers React-style controlled inputs listen on."
    )]
    pub async fn browser_set_value(
        &self,
        Parameters(input): Parameters<actions::SetValueInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::set_value(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Clear an element's `value` and fire a bubbled `input` event.
    #[tool(
        name = "browser_clear",
        description = "Clear an element's `value` by assigning `''` and firing a bubbled `input` event. Omits `change` event + focus + Backspace sequence — for contenteditable / non-`<input>` clearing semantics, use `browser_type` with a leading select-all + Delete."
    )]
    pub async fn browser_clear(
        &self,
        Parameters(input): Parameters<actions::ClearInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::clear(self.state.clone(), input).await.map(Json)
    }

    /// Move keyboard focus to an element.
    #[tool(
        name = "browser_focus",
        description = "Move keyboard focus to an element by calling `el.focus()`. No snapshot field — focus has no visual side effect for the agent to inspect."
    )]
    pub async fn browser_focus(
        &self,
        Parameters(input): Parameters<actions::FocusInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        actions::focus(self.state.clone(), input).await.map(Json)
    }

    /// Scroll an element into the center of its scroll container.
    #[tool(
        name = "browser_scroll_into_view",
        description = "Scroll an element into the center of its scroll container (`block: 'center', behavior: 'instant'`). Synchronous — the post-scroll bbox is final by the time the call returns."
    )]
    pub async fn browser_scroll_into_view(
        &self,
        Parameters(input): Parameters<actions::ScrollInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        actions::scroll_into_view(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Attach files to an `<input type=\"file\">` element.
    #[tool(
        name = "browser_upload",
        description = "Attach files to an `<input type=\"file\">` element via CDP `DOM.setFileInputFiles`. Bypasses the OS file picker; paths must exist on the host running the MCP server (not the client's machine)."
    )]
    pub async fn browser_upload(
        &self,
        Parameters(input): Parameters<actions::UploadInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        actions::upload(self.state.clone(), input).await.map(Json)
    }

    // ---------- snapshot -------------------------------------------------

    /// Return the current page's HTML (trimmed by default).
    #[tool(
        name = "browser_html",
        description = "Return the current page's HTML as a text content block. With `selector` set, returns that element's `innerHTML`. With `frame_id` set, returns that frame's `document.documentElement.outerHTML`. `selector` and `frame_id` are mutually exclusive — use the selector's own `frame_id` field to scope element lookup to a sub-frame. `trim: true` (default) strips `<script>` / `<style>` blocks and collapses whitespace."
    )]
    pub async fn browser_html(
        &self,
        Parameters(input): Parameters<snapshot::HtmlInput>,
    ) -> Result<String, ErrorData> {
        snapshot::html(self.state.clone(), input).await
    }

    /// Capture a screenshot of the current tab.
    #[tool(
        name = "browser_screenshot",
        description = "Capture a screenshot of the current tab and return it as an inline `image` content block (base64-encoded). `format` selects PNG / JPEG / WebP (default PNG). `full_page: true` captures the entire scrollable document; `selector` clips to that element's bounding box (overrides `full_page`). `omit_background: true` is honored on PNG / WebP only. `quality` (1..=100) applies to JPEG. `save_path` writes the bytes to disk on the MCP server host in addition to returning the inline image. The structured result also exposes `{ format, byte_len, saved_path? }` for callers that want metadata without decoding the image block."
    )]
    pub async fn browser_screenshot(
        &self,
        Parameters(input): Parameters<snapshot::ScreenshotInput>,
    ) -> Result<CallToolResult, ErrorData> {
        snapshot::screenshot(self.state.clone(), input).await
    }

    // ---------- eval -----------------------------------------------------

    /// Evaluate a JavaScript expression in the page's isolated world.
    #[tool(
        name = "browser_evaluate",
        description = "Evaluate `expression` in the page's ISOLATED world (page globals NOT visible — preserves stealth fingerprint shims). The expression must be an expression (not a statement block); for multi-line logic wrap in an IIFE: `(() => { /* ... */ return result; })()`. Returns the value as JSON; `undefined` → `null`. With `frame_id`, evaluates inside that frame instead of the tab's main frame. `await_promise` (default `true`) is currently observational — the lib always awaits promises."
    )]
    pub async fn browser_evaluate(
        &self,
        Parameters(input): Parameters<eval::EvalInput>,
    ) -> Result<Json<eval::EvalOutput>, ErrorData> {
        eval::evaluate(self.state.clone(), input).await.map(Json)
    }

    /// Evaluate a JavaScript expression in the page's main world.
    #[tool(
        name = "browser_evaluate_main",
        description = "Evaluate `expression` in the page's MAIN world. Page globals ARE visible — and the page can observe the call, which BREAKS STEALTH ISOLATION if the page is fingerprinting evaluator origins. Prefer `browser_evaluate` for anything that doesn't strictly require page globals. Same args + return shape as `browser_evaluate`."
    )]
    pub async fn browser_evaluate_main(
        &self,
        Parameters(input): Parameters<eval::EvalInput>,
    ) -> Result<Json<eval::EvalOutput>, ErrorData> {
        eval::evaluate_main(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- cookies --------------------------------------------------

    /// Fetch the browser's cookies.
    #[tool(
        name = "browser_cookies_get",
        description = "Fetch the browser's cookies. With `url` set, returns only cookies that would be sent for that URL (CDP `Network.getCookies`); otherwise returns every cookie in the store (`Storage.getCookies`). `name` (optional) post-filters by exact-match cookie name. Returns `{ cookies: [...] }`."
    )]
    pub async fn browser_cookies_get(
        &self,
        Parameters(input): Parameters<cookies::CookiesGetInput>,
    ) -> Result<Json<cookies::CookiesGetOutput>, ErrorData> {
        cookies::cookies_get(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Set many cookies in one CDP round-trip.
    #[tool(
        name = "browser_cookies_set",
        description = "Set many cookies in one CDP round-trip (`Storage.setCookies`). Each cookie carries the usual `name / value / domain / path / expires / http_only / secure / same_site / url` fields; `url`, when present, lets CDP infer `domain` + `path` + `secure`. Existing cookies matching `(name, domain, path)` are overwritten."
    )]
    pub async fn browser_cookies_set(
        &self,
        Parameters(input): Parameters<cookies::CookiesSetInput>,
    ) -> Result<Json<cookies::CookiesSetOutput>, ErrorData> {
        cookies::cookies_set(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Delete cookies by name (optionally narrowed by domain / path).
    #[tool(
        name = "browser_cookies_delete",
        description = "Delete cookies matching `name` plus optional `domain` / `path` narrowers (`Network.deleteCookies`). When `domain` and `path` are omitted, every cookie with the given name is removed across all domains and paths. Missing cookies are silently ignored (CDP returns no match count)."
    )]
    pub async fn browser_cookies_delete(
        &self,
        Parameters(input): Parameters<cookies::CookiesDeleteInput>,
    ) -> Result<Json<cookies::CookiesDeleteOutput>, ErrorData> {
        cookies::cookies_delete(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Clear the entire browser cookie store.
    #[tool(
        name = "browser_cookies_clear",
        description = "Clear the entire browser cookie store via `Storage.clearCookies`. No filters — for targeted deletion use `browser_cookies_delete`."
    )]
    pub async fn browser_cookies_clear(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<cookies::CookiesClearOutput>, ErrorData> {
        cookies::cookies_clear(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Persist or restore the browser's cookies via on-disk JSON.
    #[tool(
        name = "browser_cookies_persist",
        description = "Persist cookies to disk (`direction: save`) or restore from disk (`direction: load`). Format is pretty-printed `serde_json` of the cookie array — same shape `browser_cookies_get` returns. `path` is on the MCP server host, not the client's machine. Returns `{ count, direction }`."
    )]
    pub async fn browser_cookies_persist(
        &self,
        Parameters(input): Parameters<cookies::CookiesPersistInput>,
    ) -> Result<Json<cookies::CookiesPersistOutput>, ErrorData> {
        cookies::cookies_persist(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- storage --------------------------------------------------

    /// Read entries from local- or session-storage.
    #[tool(
        name = "browser_storage_get",
        description = "Read entries from `local` or `session` storage on the current tab's origin. With `key` set, returns `{ key: value }` for that one key (empty `values` map if the key is absent). Without `key`, returns every entry. `values` is sorted lexicographically for stable agent diffs."
    )]
    pub async fn browser_storage_get(
        &self,
        Parameters(input): Parameters<storage::StorageGetInput>,
    ) -> Result<Json<storage::StorageGetOutput>, ErrorData> {
        storage::storage_get(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Insert or replace one storage entry.
    #[tool(
        name = "browser_storage_set",
        description = "Set `key` to `value` in `local` or `session` storage on the current tab's origin (`DOMStorage.setDOMStorageItem`). Value is treated as opaque text by Chrome — stringify non-string values on the caller side."
    )]
    pub async fn browser_storage_set(
        &self,
        Parameters(input): Parameters<storage::StorageSetInput>,
    ) -> Result<Json<storage::StorageSetOutput>, ErrorData> {
        storage::storage_set(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Remove one storage entry by key.
    #[tool(
        name = "browser_storage_delete",
        description = "Remove `key` from `local` or `session` storage on the current tab's origin (`DOMStorage.removeDOMStorageItem`). Missing keys are silently ignored (matches the Storage API `removeItem` contract)."
    )]
    pub async fn browser_storage_delete(
        &self,
        Parameters(input): Parameters<storage::StorageDeleteInput>,
    ) -> Result<Json<storage::StorageDeleteOutput>, ErrorData> {
        storage::storage_delete(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Empty the chosen storage area for the current tab's origin.
    #[tool(
        name = "browser_storage_clear",
        description = "Empty `local` or `session` storage for the current tab's origin (`DOMStorage.clear`). Equivalent to calling `localStorage.clear()` / `sessionStorage.clear()` from page JS."
    )]
    pub async fn browser_storage_clear(
        &self,
        Parameters(input): Parameters<storage::StorageClearInput>,
    ) -> Result<Json<storage::StorageClearOutput>, ErrorData> {
        storage::storage_clear(self.state.clone(), input)
            .await
            .map(Json)
    }
}

/// Build a fresh server handler bound to the given state.
pub fn build_handler(state: Arc<Mutex<SessionState>>) -> ZendriverServer {
    ZendriverServer::new(state)
}

/// Run the MCP server over stdio until the peer disconnects.
pub async fn run_stdio(state: Arc<Mutex<SessionState>>) -> Result<(), Box<dyn std::error::Error>> {
    let handler = build_handler(state);
    let service = handler.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StealthProfileChoice;

    #[tokio::test]
    async fn browser_status_with_no_browser_reports_closed() {
        let state = Arc::new(Mutex::new(SessionState::new()));
        let server = build_handler(state);
        let out = server
            .browser_status(Parameters(EmptyInput {}))
            .await
            .expect("status call");
        let body = out.0;
        assert!(!body.open);
        assert_eq!(body.tab_count, 0);
        assert!(body.current_tab.is_none());
        assert_eq!(body.profile, StealthProfileChoice::Auto);
    }
}
