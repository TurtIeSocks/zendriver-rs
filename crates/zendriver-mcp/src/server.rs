//! rmcp server stack + tool router.
//!
//! Every `#[tool]` method on [`ZendriverServer`] is a one-liner that
//! delegates to a free async fn in [`crate::tools`]. Tools are split
//! across multiple `#[tool_router]` impl blocks: the always-on
//! `base_tool_router`, and one cfg-gated block per feature
//! (`interception`, `expect`, `cloudflare`, `fetcher`). The
//! `combined_tool_router()` helper sums them (`ToolRouter` implements
//! `Add`), and a hand-written `#[tool_handler]` impl wires that sum into
//! [`rmcp::ServerHandler`]. This split is forced by [`tool_router`] not
//! propagating per-method `#[cfg]` attributes — feature-gated tools must
//! live in their own impl block that the macro can either generate or
//! skip wholesale.
//!
//! Keeping the per-tool implementations in `tools/*.rs` (rather than
//! inline here) makes the surface easy to grow: a new tool group adds a
//! new module, a new wrapper here, and lands without touching the others.
//!
//! [`tool_router`]: rmcp::tool_router

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::transport::stdio;
use rmcp::{ErrorData, Json, ServiceExt, tool, tool_handler, tool_router};
use tokio::sync::Mutex;

use crate::state::SessionState;
#[cfg(feature = "cloudflare")]
use crate::tools::cloudflare;
use crate::tools::common::EmptyInput;
#[cfg(feature = "expect")]
use crate::tools::expect;
#[cfg(feature = "fetcher")]
use crate::tools::fetcher;
#[cfg(feature = "fingerprints")]
use crate::tools::fingerprints;
#[cfg(feature = "imperva")]
use crate::tools::imperva;
#[cfg(feature = "interception")]
use crate::tools::intercept;
#[cfg(feature = "monitor")]
use crate::tools::monitor;
use crate::tools::{
    actions, cookies, download, eval, find, frames, lifecycle, mouse, navigation, pdf, reads,
    request, scroll, snapshot, stealth, storage, tabs, window,
};

/// rmcp handler carrying the per-session [`SessionState`].
///
/// Clone is cheap (the only field is an `Arc<Mutex<_>>`).
#[derive(Clone)]
pub struct ZendriverServer {
    pub state: Arc<Mutex<SessionState>>,
}

// Two `#[tool_router]` impl blocks: this one (always present) and the
// `#[cfg(feature = "interception")]` one near the bottom of the file. The
// `tool_router` macro can't see through `#[cfg]` attributes on individual
// `#[tool]` methods, so we split feature-gated tools into a separate impl
// block that the macro can either generate or skip wholesale. The two
// per-block routers are then combined in `combined_tool_router()`, which
// the manual `ServerHandler` impl below uses.
#[tool_router(router = base_tool_router, vis = "pub")]
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
    #[tool(
        name = "browser_reload",
        description = "Reload the current tab. Set `ignore_cache: true` for a hard reload that bypasses the HTTP cache. `return_snapshot: true` includes the trimmed page HTML."
    )]
    pub async fn browser_reload(
        &self,
        Parameters(input): Parameters<navigation::ReloadInput>,
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
        description = "Close a tab. When `tab_id` is omitted, the session's current tab is closed; if that was the focused tab, focus falls back to one of the remaining tabs (or `None` if none remain). v0 limitation: in-flight `browser_expect_*` expectations and `browser_intercept_*` rules registered against the closed tab continue running until manually cancelled or until `browser_close` tears them down (the per-handle registries do not track which tab a handle was bound to)."
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
        description = "Configure the session's default stealth profile + optional fine-grained fingerprint `overrides` (`platform` / `locale` / `timezone` / `memory_gb` / `cpu_count` / `chrome_version` / `user_agent` / `bypass_csp`). Overrides are layered onto the profile and most meaningful with a `spoof_*` profile. NOTE: takes effect on the NEXT `browser_open`; does NOT re-fingerprint an already-open browser. `overrides` replaces any previously-set overrides (omit to clear)."
    )]
    pub async fn browser_set_stealth_profile(
        &self,
        Parameters(input): Parameters<stealth::SetStealthProfileInput>,
    ) -> Result<Json<stealth::SetStealthProfileOutput>, ErrorData> {
        stealth::set_stealth_profile(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Override the current tab's User-Agent at runtime.
    #[tool(
        name = "browser_set_user_agent",
        description = "Override the current tab's User-Agent at runtime (`Emulation.setUserAgentOverride`), with optional `accept_language` and `platform`. Last-write-wins and sends NO userAgentMetadata — under a Spoofed stealth profile this can clobber UA-Client-Hints coherence and INCREASE detectability. Prefer the stealth profile for stealth-sensitive tabs; use this for non-stealth tabs or a deliberate per-tab change."
    )]
    pub async fn browser_set_user_agent(
        &self,
        Parameters(input): Parameters<stealth::SetUserAgentInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        stealth::set_user_agent(self.state.clone(), input)
            .await
            .map(Json)
    }

    // ---------- find -----------------------------------------------------

    /// Resolve a Selector to one element and return its descriptor.
    #[tool(
        name = "browser_find",
        description = "Resolve a Selector to a single element on the current tab. Selector supports single-selector mode (css/xpath/text/text_exact/text_regex/role) or predicate mode (tag + attrs + text* combinable AND-ed filters). Returns `{ found: false, element: null }` when no element matches within the selector's timeout (instead of an error) — agents can branch on existence without try/catch."
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
        description = "Resolve a Selector to ALL matching elements on the current tab (up to `limit`, default 50). Selector supports single-selector mode (css/xpath/text/text_exact/text_regex/role) or predicate mode (tag + attrs + text* combinable AND-ed filters). `{ elements: [] }` is returned when nothing matches — never an error."
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
        description = "Inspect a single element's state. `include` picks which fields to populate (default `all`): `visible`/`enabled`, `bounding_box` (viewport) + `bounding_box_page` (page-absolute), `text`/`attrs`/`inner_html`/`outer_html`. `in_viewport` is reserved for v1 and always returns null. Missing-element returns `{ exists: false }` rather than an error."
    )]
    pub async fn browser_element_state(
        &self,
        Parameters(input): Parameters<reads::ElementStateInput>,
    ) -> Result<Json<reads::ElementState>, ErrorData> {
        reads::element_state(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Harvest anchor URLs (and optionally resource sources) from the page.
    #[tool(
        name = "browser_get_links",
        description = "Collect all anchor (`<a href>`) URLs on the current page. `absolute: true` resolves relative hrefs against the page URL. `include_sources: true` also returns `src`/`href` of linked-resource elements (img, script, link, …) in `sources`. Useful for crawling / link extraction without writing JS."
    )]
    pub async fn browser_get_links(
        &self,
        Parameters(input): Parameters<reads::GetLinksInput>,
    ) -> Result<Json<reads::GetLinksOutput>, ErrorData> {
        reads::get_links(self.state.clone(), input).await.map(Json)
    }

    /// Search every frame's loaded resources for a URL substring.
    #[tool(
        name = "browser_search_resources",
        description = "Search across every frame's loaded resource URLs (`Page.getResourceTree`) for a substring `query`. Returns `{ matches: [{ url, frame_id }] }`. Use to locate a script/XHR/asset by URL fragment, including inside iframes."
    )]
    pub async fn browser_search_resources(
        &self,
        Parameters(input): Parameters<reads::SearchResourcesInput>,
    ) -> Result<Json<reads::SearchResourcesOutput>, ErrorData> {
        reads::search_resources(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Click through Chrome's TLS interstitial on the current tab.
    #[tool(
        name = "browser_bypass_insecure_warning",
        description = "Dismiss Chrome's \"Your connection is not private\" TLS interstitial on the current tab (types the `thisisunsafe` bypass). Use after navigating to a site with a self-signed / invalid certificate when you intend to proceed anyway."
    )]
    pub async fn browser_bypass_insecure_warning(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        navigation::bypass_insecure_warning(self.state.clone(), input)
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
        description = "Focus an element + dispatch a single keystroke. `key` accepts a special-key name (Enter, Tab, Escape, Backspace, Delete, ArrowUp/Down/Left/Right, Space, Home, End, PageUp, PageDown, F1..F12, etc., case-insensitive) OR a single character (typed as `Key::Char`). `modifiers` (e.g. `[\"ctrl\"]`) holds modifier keys for a chord. For multi-step shortcuts use `browser_key_sequence`."
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
        description = "Set an element's `value` directly + fire bubbled `input` and `change` events. Faster than `browser_type` when keystroke realism doesn't matter, but still routes through the event handlers React-style controlled inputs listen on. Set `mode: text` to write `textContent` instead of `.value` (for non-form elements)."
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
        description = "Clear an element's `value` by assigning `''` and firing a bubbled `input` event (default `mode: value`). Set `mode: backspace` to instead focus + select-all + Backspace — emitting the real key events some inputs require."
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
        description = "Return the current page's HTML as a text content block. With `selector` set, returns that element's `innerHTML` (or `outerHTML` when `outer: true`). With `frame_id` set, returns that frame's `document.documentElement.outerHTML`. `selector` and `frame_id` are mutually exclusive — use the selector's own `frame_id` field to scope element lookup to a sub-frame. `trim: true` (default) strips `<script>` / `<style>` blocks and collapses whitespace."
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

    // ---------- request --------------------------------------------------

    /// Make an HTTP request from the browser context.
    #[tool(
        name = "browser_request",
        description = "Make an HTTP request FROM the browser context — inherits the page's cookies/session and (by default) respects CORS like an in-page `fetch`. `method` + `url` required; optional `headers`, and one of `body` (string) or `json` (object → sets body + Content-Type). `bypass_cors: true` routes via the browser's privileged network stack (ignores CORS, GET only). A non-2xx `status` is returned normally (not an error). Returns `{ status, headers, body (utf8-lossy), body_base64 }`. Needs a loaded page; navigate to the target origin first for same-origin calls."
    )]
    pub async fn browser_request(
        &self,
        Parameters(input): Parameters<request::RequestInput>,
    ) -> Result<Json<request::RequestOutput>, ErrorData> {
        request::request(self.state.clone(), input).await.map(Json)
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

    // ---------- scroll / window / pdf / mouse (Tier 1 coverage) ----------

    /// Scroll the page by a signed pixel distance.
    #[tool(
        name = "browser_scroll",
        description = "Scroll the page by a signed pixel distance via `Input.synthesizeScrollGesture`. Intuitive axes: positive `dy` scrolls **down**, positive `dx` scrolls **right** (the tool negates internally for CDP). Optional `speed` (px/sec). Returns the resulting `{ scroll_x, scroll_y }` (and the trimmed HTML when `return_snapshot: true`). Use `browser_scroll_into_view` instead when you want to bring a specific element into view."
    )]
    pub async fn browser_scroll(
        &self,
        Parameters(input): Parameters<scroll::PageScrollInput>,
    ) -> Result<Json<scroll::PageScrollOutput>, ErrorData> {
        scroll::scroll(self.state.clone(), input).await.map(Json)
    }

    /// Read the current OS window bounds + state.
    #[tool(
        name = "browser_get_window",
        description = "Read the current OS window bounds + state (`Browser.getWindowForTarget`). Returns `{ left?, top?, width?, height?, state }` in device-independent pixels; geometry fields are omitted for a minimized window. `state` is one of `normal` / `minimized` / `maximized` / `fullscreen`."
    )]
    pub async fn browser_get_window(
        &self,
        Parameters(_): Parameters<EmptyInput>,
    ) -> Result<Json<window::WindowBoundsDto>, ErrorData> {
        window::get_window(self.state.clone()).await.map(Json)
    }

    /// Resize / reposition / change the state of the OS window.
    #[tool(
        name = "browser_set_window",
        description = "Resize, reposition, or change the state of the OS window hosting the current tab. `mode`: `size` (resize to `width`×`height`, both required), `bounds` (set any subset of `left`/`top`/`width`/`height`/`state`), `maximize`, `minimize`, or `fullscreen`. Returns the resulting bounds. Affects viewport size for screenshots + responsive layouts."
    )]
    pub async fn browser_set_window(
        &self,
        Parameters(input): Parameters<window::SetWindowInput>,
    ) -> Result<Json<window::WindowBoundsDto>, ErrorData> {
        window::set_window(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Export the current page to PDF.
    #[tool(
        name = "browser_pdf",
        description = "Export the current page to PDF (`Page.printToPDF`). All options optional: `landscape`, `print_background`, `scale`, `paper_width`/`paper_height` (inches), `margin_*` (inches), `page_ranges` (e.g. `\"1-3, 5\"`), `prefer_css_page_size`. When `save_path` is set the bytes are written to disk on the MCP host and `{ saved_path, byte_len }` is returned; otherwise the bytes are base64-inlined in `{ byte_len, base64 }` (blobs over 5 MiB require `save_path`)."
    )]
    pub async fn browser_pdf(
        &self,
        Parameters(input): Parameters<pdf::PdfInput>,
    ) -> Result<Json<crate::tools::common::BlobOutput>, ErrorData> {
        pdf::pdf(self.state.clone(), input).await.map(Json)
    }

    /// Capture an MHTML archive of the current page.
    #[tool(
        name = "browser_save_mhtml",
        description = "Capture a single-file MHTML archive of the current page (`Page.captureSnapshot`) — HTML plus inlined subresources. When `save_path` is set the bytes are written to disk on the MCP host; otherwise base64-inlined (same `BlobOutput` shape as `browser_pdf`)."
    )]
    pub async fn browser_save_mhtml(
        &self,
        Parameters(input): Parameters<pdf::SaveMhtmlInput>,
    ) -> Result<Json<crate::tools::common::BlobOutput>, ErrorData> {
        pdf::save_mhtml(self.state.clone(), input).await.map(Json)
    }

    /// Dispatch a coordinate-anchored mouse action.
    #[tool(
        name = "browser_mouse",
        description = "Dispatch a coordinate-anchored pointer action for canvas / drag-and-drop / map / game interactions not reachable via element-targeted tools. `action`: `move` (Bezier path to `x,y`), `click` (at `x,y`, with optional `button` / `click_count` / `modifiers`), or `drag` (press at `x,y`, drag to `to_x,to_y` over `steps`, release). Coordinates are viewport CSS pixels. Returns `{ ok }` (and trimmed HTML when `return_snapshot: true`)."
    )]
    pub async fn browser_mouse(
        &self,
        Parameters(input): Parameters<mouse::MouseInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        mouse::mouse(self.state.clone(), input).await.map(Json)
    }

    // ---------- keyboard chords / downloads / load waits (Tier 2) --------

    /// Dispatch a mixed text / key-chord sequence into an element.
    #[tool(
        name = "browser_key_sequence",
        description = "Focus an element and dispatch an ordered sequence of typed text and (chorded) key presses — the way to send shortcuts like Ctrl+A, Cmd+C, or Tab-between-fields in one call. Each step sets `text` (literal text) OR `key` (a special-key name or single char) with optional `modifiers` (`alt`/`ctrl`/`meta`/`shift`). Example: `[{\"key\":\"a\",\"modifiers\":[\"ctrl\"]},{\"text\":\"new value\"}]`."
    )]
    pub async fn browser_key_sequence(
        &self,
        Parameters(input): Parameters<actions::KeySequenceInput>,
    ) -> Result<Json<actions::ActionOutput>, ErrorData> {
        actions::key_sequence(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Download a URL through the page's own network context.
    #[tool(
        name = "browser_download",
        description = "Download `url` by fetching it from the page's own network context (cookies / referer / same-origin credentials) and saving it via Chrome's download behavior. Optional `filename` sets the saved name (else derived from the URL). Fire-and-forget: returns once the fetch is dispatched, NOT when the file lands — for await/save-to-path use `browser_expect_register { kind: download, save_to }`."
    )]
    pub async fn browser_download(
        &self,
        Parameters(input): Parameters<download::DownloadInput>,
    ) -> Result<Json<download::DownloadOutput>, ErrorData> {
        download::download(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Set the directory downloads are saved into.
    #[tool(
        name = "browser_set_download_path",
        description = "Set the directory (on the MCP host) Chrome saves downloads into for the current tab (`Browser.setDownloadBehavior`). Created if missing. Applies to subsequent `browser_download` calls and any in-page-triggered downloads."
    )]
    pub async fn browser_set_download_path(
        &self,
        Parameters(input): Parameters<download::SetDownloadPathInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        download::set_download_path(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Wait for a document-load milestone on the tab or a frame.
    #[tool(
        name = "browser_wait_for_load",
        description = "Wait for a load milestone. With no args, waits for the tab's `load` event. `ready_state` (`interactive` / `complete`) waits for that `document.readyState`. `frame_id` waits for a specific frame's load instead (ignoring `ready_state`). Distinct from `browser_wait_for_idle`, which waits for network quiet."
    )]
    pub async fn browser_wait_for_load(
        &self,
        Parameters(input): Parameters<navigation::WaitForLoadInput>,
    ) -> Result<Json<navigation::NavOutput>, ErrorData> {
        navigation::wait_for_load(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Navigate a specific frame to a URL.
    #[tool(
        name = "browser_frame_goto",
        description = "Navigate the frame identified by `frame_id` (from `browser_frame_list`) to `url`, then wait for its load. Works on the main frame of an out-of-process iframe; in-process child frames surface the lib's navigation error. For top-level navigation use `browser_goto`."
    )]
    pub async fn browser_frame_goto(
        &self,
        Parameters(input): Parameters<frames::FrameGotoInput>,
    ) -> Result<Json<actions::AckOutput>, ErrorData> {
        frames::frame_goto(self.state.clone(), input)
            .await
            .map(Json)
    }
}

// ---------- interception (gated) ----------------------------------------
//
// Separate impl block so the `tool_router` macro can be cfg-gated as a
// unit. See the comment on `base_tool_router` above for why per-method
// `#[cfg]` doesn't work.

#[cfg(feature = "interception")]
#[tool_router(router = intercept_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Install one network-interception rule on the current tab.
    #[tool(
        name = "browser_intercept_add_rule",
        description = "Install one interception rule on the current tab. `pattern` uses CDP wildcard syntax (`*` / `?`). `action.kind` selects `block` (fail with `BlockedByClient`), `redirect` (URL replacement via `action.to`), `respond` (synthesize a response — `status`, `body`, optional `content_type`, optional `headers`), or `modify_request` (overlay extra `headers` onto the request). Returns `{ rule_id }`. v0: one rule per call — chain multiple `add_rule` calls for multiple rules. Each rule spawns its own actor; tearing it down (`browser_intercept_remove_rule` / `_clear_rules`) stops only that rule."
    )]
    pub async fn browser_intercept_add_rule(
        &self,
        Parameters(input): Parameters<intercept::AddRuleInput>,
    ) -> Result<Json<intercept::AddRuleOutput>, ErrorData> {
        intercept::add_rule(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Remove a previously-installed interception rule.
    #[tool(
        name = "browser_intercept_remove_rule",
        description = "Remove the interception rule identified by `rule_id`. Drops the underlying handle, which cancels the per-rule actor and tears down its `Fetch.enable`. Returns an error if the id is unknown — for an idempotent clear-all use `browser_intercept_clear_rules`."
    )]
    pub async fn browser_intercept_remove_rule(
        &self,
        Parameters(input): Parameters<intercept::RemoveRuleInput>,
    ) -> Result<Json<intercept::RemoveRuleOutput>, ErrorData> {
        intercept::remove_rule(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Enumerate every live interception rule.
    #[tool(
        name = "browser_intercept_list_rules",
        description = "Enumerate every live interception rule. Returns `{ rules: [{ rule_id, pattern, action_kind }] }` sorted by `rule_id` for stable output. Empty list (never an error) when no rules are installed."
    )]
    pub async fn browser_intercept_list_rules(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<intercept::ListRulesOutput>, ErrorData> {
        intercept::list_rules(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Drop every live interception rule on this session.
    #[tool(
        name = "browser_intercept_clear_rules",
        description = "Drop every live interception rule on this session (each handle's `Drop` cancels its actor). Returns `{ cleared: <count> }`. Idempotent — calling on an empty registry returns `{ cleared: 0 }` rather than erroring."
    )]
    pub async fn browser_intercept_clear_rules(
        &self,
        Parameters(input): Parameters<EmptyInput>,
    ) -> Result<Json<intercept::ClearRulesOutput>, ErrorData> {
        intercept::clear_rules(self.state.clone(), input)
            .await
            .map(Json)
    }
}

// ---------- expect (gated) -----------------------------------------------
//
// Same split pattern as the interception block above — the `tool_router`
// macro can't see through per-method `#[cfg]`, so feature-gated tools live
// in their own impl block. The matched-event JSON returned by
// `browser_expect_await` is `serde_json::Value`; the schema for that on the
// wire is `JSON`, which rmcp's `Json` wrapper handles transparently.

#[cfg(feature = "expect")]
#[tool_router(router = expect_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Register a one-shot expectation against the current tab.
    #[tool(
        name = "browser_expect_register",
        description = "Register a one-shot expectation against the current tab. `kind` selects `request` / `response` / `dialog` / `download`. `matcher.url_substr` / `matcher.url_regex` filter request and response by URL (regex wins if both set; default matches every URL). `dialog` and `download` ignore matcher fields entirely. `pre_await_timeout_ms` (default 60_000 = 60s) is the inner timeout applied to the lib's `.timeout(d)` so the user has time to trigger the action between `_register` and `_await`. Drive params (decided here because the matched handle is consumed by the spawned task): `dialog_action` (`accept`/`dismiss`, plus `dialog_prompt_text` for prompts) drives a matched dialog so the page's blocking call returns; `fetch_body: true` inlines the response body as `body_base64`; `save_to: <path>` copies a completed download to the MCP host. Returns `{ expectation_id }` — pass to `browser_expect_await` or `browser_expect_cancel`."
    )]
    pub async fn browser_expect_register(
        &self,
        Parameters(input): Parameters<expect::RegisterInput>,
    ) -> Result<Json<expect::RegisterOutput>, ErrorData> {
        expect::register(self.state.clone(), input).await.map(Json)
    }

    /// Wait for a previously-registered expectation to resolve.
    #[tool(
        name = "browser_expect_await",
        description = "Wait for a previously-registered expectation to resolve. `timeout_ms` (default 30_000 = 30s) is the outer wait on the spawned task's matched-event channel. Returns `{ expectation_id, event }` where `event` is a JSON object whose shape depends on the expectation's `kind`: request/response carry `url` / `headers` / `method` or `status`; dialog carries `dialog_type` / `message` / `default_prompt` / `driven`; download carries `suggested_filename` / `guid` / `download_dir` / `saved_path`. When the expectation was registered with `fetch_body: true`, response events also carry `body_base64` + `body_len`. Dialog drive and download save are requested at register time (see `browser_expect_register`)."
    )]
    pub async fn browser_expect_await(
        &self,
        Parameters(input): Parameters<expect::AwaitInput>,
    ) -> Result<Json<expect::AwaitOutput>, ErrorData> {
        expect::await_expectation(self.state.clone(), input)
            .await
            .map(Json)
    }

    /// Cancel a pending expectation, aborting its spawned task.
    #[tool(
        name = "browser_expect_cancel",
        description = "Cancel a pending expectation. Drops the registry entry and aborts the spawned task so the lib-side `.matched()` future is torn down promptly instead of left to fire its own pre-await timeout. Returns `{ cancelled: true }`. Unknown ids surface `ExpectationNotFound`."
    )]
    pub async fn browser_expect_cancel(
        &self,
        Parameters(input): Parameters<expect::CancelInput>,
    ) -> Result<Json<expect::CancelOutput>, ErrorData> {
        expect::cancel(self.state.clone(), input).await.map(Json)
    }
}

// ---------- cloudflare (gated) -------------------------------------------
//
// Same split pattern as the interception / expect blocks above — the
// `tool_router` macro can't see through per-method `#[cfg]`, so the
// `browser_solve_turnstile` tool lives in its own impl block.

#[cfg(feature = "cloudflare")]
#[tool_router(router = cloudflare_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Drive the Cloudflare Turnstile clearance flow on the current tab.
    #[tool(
        name = "browser_solve_turnstile",
        description = "Drive the Cloudflare Turnstile clearance flow on the current tab. Locates the Turnstile iframe, clicks the checkbox at the canonical 15%/50% bbox offset, then polls every `poll_interval_ms` (default 500ms) until one of three terminal states is reached, bounded by `timeout_ms` (default 30_000 = 30s): `solved` (token captured in `cf-turnstile-response` — returned in `token`), `challenge_gone` (iframe vanished without a token, e.g. clearance cookie shortcut), or `timeout` (deadline elapsed — not an error, the agent can retry or fall back). Errors only on the structural failures: no challenge present at all, CDP call failed, or in-page JS exception."
    )]
    pub async fn browser_solve_turnstile(
        &self,
        Parameters(input): Parameters<cloudflare::SolveInput>,
    ) -> Result<Json<cloudflare::SolveOutput>, ErrorData> {
        cloudflare::solve_turnstile(self.state.clone(), input)
            .await
            .map(Json)
    }
}

// ---------- imperva (gated) ----------------------------------------------
//
// Same split pattern as the cloudflare block — own impl block so the
// `tool_router` macro can cfg-gate the whole thing.

#[cfg(feature = "imperva")]
#[tool_router(router = imperva_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Drive the Imperva / Incapsula clearance flow on the current tab.
    #[tool(
        name = "browser_solve_imperva",
        description = "Drive the Imperva / Incapsula clearance flow on the current tab. Detects the active surface (modern reese84 bot-management, legacy Incapsula, or CAPTCHA escalation) and polls every `poll_interval_ms` until one of four terminal states is reached, bounded by `timeout_ms` (default 30_000 = 30s): `token_acquired` (reese84 cookie captured — returned in `reese84`), `challenge_gone` (markers cleared without a token, e.g. legacy flow), `already_clear` (no surface present at call time — fast path), or `timeout` (deadline elapsed — not an error, retry or fall back). Set `with_interception: true` for the Fetch-domain fast-path. Errors on structural failures: CAPTCHA with no solver, CDP failure, or in-page JS exception. Requires stealth (on by default)."
    )]
    pub async fn browser_solve_imperva(
        &self,
        Parameters(input): Parameters<imperva::SolveImpervaInput>,
    ) -> Result<Json<imperva::SolveImpervaOutput>, ErrorData> {
        imperva::solve_imperva(self.state.clone(), input)
            .await
            .map(Json)
    }
}

// ---------- fetcher (gated) ----------------------------------------------
//
// Same split pattern as the other gated blocks. `browser_install_chrome`
// is the v0 surface; `browser_list_installed_chromes` was dropped per the
// plan's API Reality note (no lib support, and reaching into the cache
// layout by hand was deemed too invasive for v0).

#[cfg(feature = "fetcher")]
#[tool_router(router = fetcher_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Resolve + download (on cache miss) a Chrome-for-Testing binary.
    #[tool(
        name = "browser_install_chrome",
        description = "Resolve, download (on cache miss), and return the path to a runnable Chrome-for-Testing binary on the MCP server host. `version` selects an exact CFT release (e.g. `\"126.0.6478.182\"`) and wins over `channel` when both are set. `channel` (case-insensitive: `stable` / `beta` / `dev` / `canary`) maps to a release channel; only `stable` is wired end-to-end as of v0 — the others surface `UnsupportedPlatform`. Omitting both falls back to the lib's `Latest`. `cache_dir` overrides the OS cache root (`$XDG_CACHE_HOME/zendriver/chrome` on Linux, `~/Library/Caches/zendriver/chrome` on macOS). Returns `{ path, version_requested?, channel_requested? }` — `path` is on the MCP server host, not the client's machine."
    )]
    pub async fn browser_install_chrome(
        &self,
        Parameters(input): Parameters<fetcher::InstallInput>,
    ) -> Result<Json<fetcher::InstallOutput>, ErrorData> {
        fetcher::install_chrome(self.state.clone(), input)
            .await
            .map(Json)
    }
}

// ---------- fingerprints (gated) ----------------------------------------
//
// Same split pattern as the other gated blocks. Not in `default` — must be
// opted into explicitly with `--features fingerprints`.

#[cfg(feature = "fingerprints")]
#[tool_router(router = fingerprints_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Generate a fingerprint Persona from a pool or generative source.
    #[tool(
        name = "browser_fingerprint_generate",
        description = "Generate a realistic fingerprint Persona JSON from a real-device `source`. `generative` synthesizes a coherent persona from the embedded Bayesian network and works offline; `pool` samples a downloaded real-device set (requires the published pool asset — see issue #25; returns an error until the dataset is hosted). Optional `seed` (u64) for reproducibility — omit for a random persona. Returns `{ persona }` — pass it to `browser_open`'s `persona` field (inspect / tweak the JSON first if desired)."
    )]
    pub async fn browser_fingerprint_generate(
        &self,
        Parameters(input): Parameters<fingerprints::GenerateInput>,
    ) -> Result<Json<fingerprints::GenerateOutput>, ErrorData> {
        fingerprints::generate(input).await.map(Json)
    }
}

// ---------- monitor (gated) ---------------------------------------------
//
// Same split pattern as the other gated blocks. In `default`, so these are
// present in a normal build; drop them with `--no-default-features`.

#[cfg(feature = "monitor")]
#[tool_router(router = monitor_tool_router, vis = "pub")]
impl ZendriverServer {
    /// Start a network monitor over the current tab.
    #[tool(
        name = "browser_monitor_start",
        description = "Start a passive network monitor over the current tab and begin buffering observed events (HTTP exchanges, WebSocket open/frame/close, EventSource messages) into a bounded ring. Optional `url_pattern` keeps only events whose URL contains that substring. `capture_bodies: true` fetches each HTTP response body at observe-time (one extra CDP round-trip per exchange) and inlines it — Chrome retains bodies only briefly, so capture-at-observe is the only reliable way to get them. Returns `{ handle }`; poll with `browser_monitor_read` and tear down with `browser_monitor_stop`. The ring holds the most recent events — a slow reader sees a non-zero `dropped` count rather than unbounded memory growth."
    )]
    pub async fn browser_monitor_start(
        &self,
        Parameters(input): Parameters<monitor::StartInput>,
    ) -> Result<Json<monitor::StartOutput>, ErrorData> {
        monitor::start(self.state.clone(), input).await.map(Json)
    }

    /// Drain buffered events from a running monitor.
    #[tool(
        name = "browser_monitor_read",
        description = "Drain buffered events from a running monitor (started via `browser_monitor_start`). `handle` identifies the monitor; optional `max` caps how many events this call returns (omit to drain all buffered). Returns `{ events, dropped }`: `events` are oldest-first and are removed from the buffer (the next read sees only newer ones); `dropped` is the count of events evicted because the ring filled since the previous read (reset to 0 each read) — a non-zero value means read more often. An unknown `handle` is an error."
    )]
    pub async fn browser_monitor_read(
        &self,
        Parameters(input): Parameters<monitor::ReadInput>,
    ) -> Result<Json<monitor::ReadOutput>, ErrorData> {
        monitor::read(self.state.clone(), input).await.map(Json)
    }

    /// Stop a running monitor and drop its buffer.
    #[tool(
        name = "browser_monitor_stop",
        description = "Stop a running monitor: cancel its background drain task, drop its buffer, and remove the handle. `handle` identifies the monitor. Returns `{ stopped }` — `true` if a live monitor was found and stopped, `false` if the handle was unknown (already stopped or never started); stopping is idempotent and never errors. Any events left unread when you stop are discarded — call `browser_monitor_read` first if you need them."
    )]
    pub async fn browser_monitor_stop(
        &self,
        Parameters(input): Parameters<monitor::StopInput>,
    ) -> Result<Json<monitor::StopOutput>, ErrorData> {
        monitor::stop(self.state.clone(), input).await.map(Json)
    }
}

// ---------- combined router + ServerHandler -----------------------------

impl ZendriverServer {
    /// Combine the always-on base router with every feature-gated router
    /// the build was compiled with (`interception`, `expect`,
    /// `cloudflare`, `fetcher`). The `tool_handler` impl below points at
    /// this so a single `ServerHandler::call_tool` / `list_tools` reaches
    /// every tool the build was compiled with.
    pub fn combined_tool_router() -> ToolRouter<Self> {
        let router = Self::base_tool_router();
        #[cfg(feature = "interception")]
        let router = router + Self::intercept_tool_router();
        #[cfg(feature = "expect")]
        let router = router + Self::expect_tool_router();
        #[cfg(feature = "cloudflare")]
        let router = router + Self::cloudflare_tool_router();
        #[cfg(feature = "imperva")]
        let router = router + Self::imperva_tool_router();
        #[cfg(feature = "fetcher")]
        let router = router + Self::fetcher_tool_router();
        #[cfg(feature = "fingerprints")]
        let router = router + Self::fingerprints_tool_router();
        #[cfg(feature = "monitor")]
        let router = router + Self::monitor_tool_router();
        router
    }
}

#[tool_handler(router = Self::combined_tool_router())]
impl rmcp::ServerHandler for ZendriverServer {}

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

/// Run the MCP server over streamable HTTP, bound to `addr`. Each new
/// MCP session gets its own [`SessionState`] (and therefore its own
/// `Browser` slot) — clients do not share browser state.
pub async fn run_http(
    addr: std::net::SocketAddr,
    default_profile: crate::state::StealthProfileChoice,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::transport::http::serve(addr, default_profile).await
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
