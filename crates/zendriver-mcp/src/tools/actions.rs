//! Element action handlers — `browser_click`, `_hover`, `_type`, `_press`,
//! `_set_value`, `_clear`, `_focus`, `_scroll_into_view`, `_upload`.
//!
//! Every handler shares the same skeleton:
//!
//! 1. Lock the session, fetch the current tab via
//!    [`crate::tools::common::current_tab`].
//! 2. Resolve the selector to an [`zendriver::Element`] via
//!    [`crate::tools::find::resolve`].
//! 3. Invoke the corresponding `Element::*` method (with options when the
//!    tool accepts them).
//! 4. Optionally collect a trimmed HTML snapshot of the post-action page
//!    state when the input set `return_snapshot: true`.
//!
//! The shared [`ActionOutput`] struct carries `ok: true` plus the optional
//! snapshot. Non-visual tools (`browser_focus` / `_scroll_into_view` /
//! `_upload`) intentionally don't accept `return_snapshot` — they have no
//! observable page-rendering side effect for the agent to inspect, and
//! avoiding the eval round-trip keeps them cheap.
//!
//! ## Key string → `zendriver::Key` mapping
//!
//! [`parse_key`] accepts named special keys (case-insensitive: "Enter",
//! "enter", "ENTER" all work) AND single ASCII characters (typed as
//! `Key::Char(c)`). Unknown names surface as `invalid_params` listing the
//! accepted special-key names — surfacing typos to the agent rather than
//! silently mis-firing as a `Char` literal.
//!
//! ## MouseButton mapping
//!
//! [`MouseButtonArg`] mirrors the three buttons agents typically dispatch
//! (`Left / Middle / Right`). Back/forward thumb buttons exist on
//! [`zendriver::MouseButton`] but are intentionally not exposed over the
//! wire — they're rarely useful and add an enum-variant churn cost we
//! don't need yet.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::{ClickOptions, Key, KeySequence, MouseButton, SpecialKey};

use crate::errors::{McpServerError, map_error};
use crate::selectors::Selector;
use crate::snapshot::html_trim;
use crate::state::SessionState;
use crate::tools::common::{ModifierArg, current_tab, modifiers_to_bits};
use crate::tools::find::resolve;

// ---------- shared output ------------------------------------------------

/// Common output of every action tool that accepts `return_snapshot`.
///
/// `ok` is always `true` on a `Result::Ok`; errors come back as
/// [`rmcp::ErrorData`], not an `ok: false`. `snapshot` is populated only
/// when the caller opted in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ActionOutput {
    /// Always `true` for successful action returns; failures surface as
    /// `Err(ErrorData)` instead of an `ok: false` body.
    pub ok: bool,
    /// Trimmed rendered HTML of the page after the action completes
    /// (drops `<script>` / `<style>` blocks + collapses whitespace).
    /// Populated only when `return_snapshot: true` is on the input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

/// `{ ok: true }` reply for non-visual tools that don't accept a snapshot
/// (focus, scroll_into_view, upload). Distinct from [`ActionOutput`] so
/// the schema doesn't carry a permanently-absent `snapshot` field.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AckOutput {
    /// Always `true` for successful returns.
    pub ok: bool,
}

/// Collect a trimmed snapshot of the current rendered HTML.
///
/// Mirrors the helper in [`crate::tools::navigation`] — uses
/// `document.documentElement.outerHTML` (rather than CDP's
/// `Page.captureSnapshot`) so the result reflects post-script DOM mutations.
async fn snapshot_now(tab: &zendriver::Tab) -> Result<String, ErrorData> {
    let html: String = tab
        .evaluate_main("document.documentElement.outerHTML")
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(html_trim::trim(&html))
}

/// Build an [`ActionOutput`] from a tab + an optional snapshot flag.
async fn ok_with_snapshot(
    tab: &zendriver::Tab,
    return_snapshot: bool,
) -> Result<ActionOutput, ErrorData> {
    let snapshot = if return_snapshot {
        Some(snapshot_now(tab).await?)
    } else {
        None
    };
    Ok(ActionOutput { ok: true, snapshot })
}

// ---------- MouseButton + Key wire shapes --------------------------------

/// MCP-layer mouse button enum, mapped to [`zendriver::MouseButton`].
///
/// Only the three commonly-dispatched buttons; thumb buttons (Back /
/// Forward) are deliberately omitted to keep the schema small.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButtonArg {
    /// Primary button (default).
    #[default]
    Left,
    /// Middle button (scroll wheel click).
    Middle,
    /// Secondary button.
    Right,
}

impl From<MouseButtonArg> for MouseButton {
    fn from(b: MouseButtonArg) -> Self {
        match b {
            MouseButtonArg::Left => MouseButton::Left,
            MouseButtonArg::Middle => MouseButton::Middle,
            MouseButtonArg::Right => MouseButton::Right,
        }
    }
}

/// Map a wire key string onto a [`zendriver::Key`].
///
/// Accepts (case-insensitive) every variant of
/// [`zendriver::SpecialKey`] by its CamelCase name, OR a single ASCII
/// character which becomes [`Key::Char(c)`][zendriver::Key::Char].
/// Unknown multi-character strings surface as `invalid_params` listing
/// every accepted special-key name.
pub fn parse_key(s: &str) -> Result<Key, ErrorData> {
    // Single-char fast path: any single Unicode scalar becomes Char(c).
    let mut chars = s.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(Key::Char(c));
    }
    let special = match s.to_ascii_lowercase().as_str() {
        "enter" | "return" => SpecialKey::Enter,
        "tab" => SpecialKey::Tab,
        "escape" | "esc" => SpecialKey::Escape,
        "backspace" => SpecialKey::Backspace,
        "delete" | "del" => SpecialKey::Delete,
        "space" => SpecialKey::Space,
        "arrowup" | "up" => SpecialKey::ArrowUp,
        "arrowdown" | "down" => SpecialKey::ArrowDown,
        "arrowleft" | "left" => SpecialKey::ArrowLeft,
        "arrowright" | "right" => SpecialKey::ArrowRight,
        "home" => SpecialKey::Home,
        "end" => SpecialKey::End,
        "pageup" => SpecialKey::PageUp,
        "pagedown" => SpecialKey::PageDown,
        "insert" | "ins" => SpecialKey::Insert,
        "capslock" => SpecialKey::CapsLock,
        "numlock" => SpecialKey::NumLock,
        "scrolllock" => SpecialKey::ScrollLock,
        "printscreen" => SpecialKey::PrintScreen,
        "pause" => SpecialKey::Pause,
        "contextmenu" => SpecialKey::ContextMenu,
        "f1" => SpecialKey::F1,
        "f2" => SpecialKey::F2,
        "f3" => SpecialKey::F3,
        "f4" => SpecialKey::F4,
        "f5" => SpecialKey::F5,
        "f6" => SpecialKey::F6,
        "f7" => SpecialKey::F7,
        "f8" => SpecialKey::F8,
        "f9" => SpecialKey::F9,
        "f10" => SpecialKey::F10,
        "f11" => SpecialKey::F11,
        "f12" => SpecialKey::F12,
        _ => {
            return Err(ErrorData::invalid_params(
                format!(
                    "Unknown key `{s}`. Accepted special keys: Enter, Tab, Escape, Backspace, Delete, Space, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, Home, End, PageUp, PageDown, Insert, CapsLock, NumLock, ScrollLock, PrintScreen, Pause, ContextMenu, F1..F12. Single characters (e.g. `a`, `?`) are typed as `Key::Char`."
                ),
                None,
            ));
        }
    };
    Ok(Key::Special(special))
}

// ---------- browser_click ------------------------------------------------

/// Input for `browser_click`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClickInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Which mouse button to dispatch. Default: `left`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButtonArg>,
    /// `clickCount` for the CDP dispatch (set `2` for a double-click in
    /// a single call). Default: `1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_count: Option<u32>,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Click an element with realistic Bezier-path cursor approach + the full
/// actionability gate.
///
/// Always routes through [`zendriver::Element::click_with`] (even with
/// no overrides) so the per-call options struct stays the single source
/// of truth.
pub async fn click(
    state: Arc<Mutex<SessionState>>,
    input: ClickInput,
) -> Result<ActionOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    let opts = ClickOptions {
        button: input.button.unwrap_or_default().into(),
        click_count: input.click_count.unwrap_or(1),
        ..Default::default()
    };
    el.click_with(opts)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_hover ------------------------------------------------

/// Input for `browser_hover`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HoverInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Hover the cursor over an element's bbox center (realistic Bezier path).
pub async fn hover(
    state: Arc<Mutex<SessionState>>,
    input: HoverInput,
) -> Result<ActionOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.hover()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_type -------------------------------------------------

/// Input for `browser_type`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Text to type, character-by-character with realistic timing.
    pub text: String,
    /// When `true`, call `clear()` on the element before typing.
    /// Useful for replacing the contents of a pre-filled input. Default: `false`.
    #[serde(default)]
    pub clear_first: bool,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Type `text` into an element with realistic per-character timing.
///
/// When `clear_first: true`, the element's `value` is reset to `''` (via
/// `Element::clear`) before typing — useful for replacing the contents
/// of a pre-filled input without an explicit two-call sequence.
pub async fn type_text(
    state: Arc<Mutex<SessionState>>,
    input: TypeInput,
) -> Result<ActionOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    if input.clear_first {
        el.clear()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    }
    el.type_text(&input.text)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_press ------------------------------------------------

/// Input for `browser_press`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PressInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Named special key (e.g. "Enter", "Tab", "ArrowUp") OR a single
    /// character (e.g. "a"). Special-key names are case-insensitive.
    pub key: String,
    /// Modifier keys to hold while pressing `key` (a chord, e.g.
    /// `["ctrl"]` + `key: "a"` for select-all). Default none.
    #[serde(default)]
    pub modifiers: Vec<ModifierArg>,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Focus an element + dispatch a single keystroke.
///
/// `key` accepts either a special-key name (Enter, Tab, etc.) or a single
/// character — see [`parse_key`] for the full accept list.
pub async fn press(
    state: Arc<Mutex<SessionState>>,
    input: PressInput,
) -> Result<ActionOutput, ErrorData> {
    let key = parse_key(&input.key)?;
    let mods = modifiers_to_bits(&input.modifiers);
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    if mods.is_empty() {
        el.press(key)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    } else {
        el.press_with(key, mods)
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
    }
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_key_sequence -----------------------------------------

/// One step in a [`KeySequenceInput`]: either literal text or a key press
/// (optionally a modifier chord). Exactly one of `text` / `key` must be set.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyStep {
    /// Literal text to type (grapheme-by-grapheme). Mutually exclusive with
    /// `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Key to press — a special-key name or single character (see
    /// `browser_press`). Mutually exclusive with `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Modifiers held while pressing `key` (ignored for `text`).
    #[serde(default)]
    pub modifiers: Vec<ModifierArg>,
}

/// Input for `browser_key_sequence`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeySequenceInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Ordered steps to dispatch: a mix of typed text and (chorded) key
    /// presses. Example: `[{ "key": "a", "modifiers": ["ctrl"] }, { "text":
    /// "replacement" }]` selects-all then types over it.
    pub sequence: Vec<KeyStep>,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Focus an element + dispatch a mixed text / key-chord sequence in order.
pub async fn key_sequence(
    state: Arc<Mutex<SessionState>>,
    input: KeySequenceInput,
) -> Result<ActionOutput, ErrorData> {
    // Build the lib sequence up front so a bad key name fails before we touch
    // the browser.
    let mut seq = KeySequence::new();
    for step in &input.sequence {
        match (&step.text, &step.key) {
            (Some(text), None) => seq = seq.text(text.clone()),
            (None, Some(key_str)) => {
                let key = parse_key(key_str)?;
                let mods = modifiers_to_bits(&step.modifiers);
                if mods.is_empty() {
                    match key {
                        Key::Special(sk) => seq = seq.key(sk),
                        Key::Char(_) => seq = seq.chord(key, mods),
                    }
                } else {
                    seq = seq.chord(key, mods);
                }
            }
            _ => {
                return Err(ErrorData::invalid_params(
                    "each sequence step must set exactly one of `text` or `key`".to_string(),
                    None,
                ));
            }
        }
    }
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.type_keys(seq)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_set_value --------------------------------------------

/// Input for `browser_set_value`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetValueInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// New value for the element's `.value` property.
    pub value: String,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Set an element's `value` directly + fire bubbled `input` + `change`
/// events.
///
/// Faster than [`type_text`] when the caller doesn't care about
/// keystroke-by-keystroke realism — bypasses keydown/keyup but still
/// fires the events React-style controlled inputs listen on.
pub async fn set_value(
    state: Arc<Mutex<SessionState>>,
    input: SetValueInput,
) -> Result<ActionOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.set_value(&input.value)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_clear ------------------------------------------------

/// Input for `browser_clear`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// When `true`, include the trimmed page HTML in the response.
    #[serde(default)]
    pub return_snapshot: bool,
}

/// Clear an element's `value` and fire a bubbled `input` event.
pub async fn clear(
    state: Arc<Mutex<SessionState>>,
    input: ClearInput,
) -> Result<ActionOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.clear()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    ok_with_snapshot(&tab, input.return_snapshot).await
}

// ---------- browser_focus ------------------------------------------------

/// Input for `browser_focus`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocusInput {
    #[serde(flatten)]
    pub selector: Selector,
}

/// Move keyboard focus to an element.
///
/// No snapshot field — focus has no visual side effect for the agent to
/// inspect, and skipping the eval round-trip keeps the call cheap.
pub async fn focus(
    state: Arc<Mutex<SessionState>>,
    input: FocusInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.focus()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

// ---------- browser_scroll_into_view ------------------------------------

/// Input for `browser_scroll_into_view`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollInput {
    #[serde(flatten)]
    pub selector: Selector,
}

/// Scroll an element into the center of its scroll container.
///
/// No snapshot — the layout-shift side effect is best inspected via a
/// follow-up `browser_html` / `browser_element_state` call if the agent
/// cares about it.
pub async fn scroll_into_view(
    state: Arc<Mutex<SessionState>>,
    input: ScrollInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.scroll_into_view()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

// ---------- browser_upload -----------------------------------------------

/// Input for `browser_upload`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Absolute paths to attach to the `<input type="file">` element.
    pub paths: Vec<String>,
}

/// Attach files to an `<input type="file">` via CDP's
/// `DOM.setFileInputFiles`.
///
/// Bypasses the OS file picker. Paths must point at the file's location
/// on the host running the MCP server (not the client's machine) — CDP
/// reads the files server-side and wires them straight into the input's
/// `FileList`.
pub async fn upload(
    state: Arc<Mutex<SessionState>>,
    input: UploadInput,
) -> Result<AckOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = resolve(&tab, &input.selector).await?;
    el.upload_files(&input.paths)
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    Ok(AckOutput { ok: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    /// Build a minimal css-only [`Selector`] for the no-browser error tests.
    fn css(s: &str) -> Selector {
        Selector {
            css: Some(s.into()),
            xpath: None,
            text: None,
            text_exact: None,
            text_regex: None,
            role: None,
            role_name: None,
            nth: None,
            visible_only: true,
            timeout_ms: 5000,
            frame_id: None,
        }
    }

    /// Convenience: assert `err` carries the `browser_open` suggestion.
    fn assert_suggests_browser_open(err: &ErrorData) {
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn click_with_no_browser_suggests_browser_open() {
        let err = click(
            fresh(),
            ClickInput {
                selector: css("button"),
                button: None,
                click_count: None,
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn hover_with_no_browser_suggests_browser_open() {
        let err = hover(
            fresh(),
            HoverInput {
                selector: css("a"),
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn type_with_no_browser_suggests_browser_open() {
        let err = type_text(
            fresh(),
            TypeInput {
                selector: css("input"),
                text: "hello".into(),
                clear_first: false,
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn press_with_no_browser_suggests_browser_open() {
        // The parse_key path runs BEFORE the state lock, so we must pass a
        // valid key here — otherwise we'd be testing the key-parse error,
        // not the missing-browser one.
        let err = press(
            fresh(),
            PressInput {
                selector: css("input"),
                key: "Enter".into(),
                modifiers: Vec::new(),
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn set_value_with_no_browser_suggests_browser_open() {
        let err = set_value(
            fresh(),
            SetValueInput {
                selector: css("input"),
                value: "rust async".into(),
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn clear_with_no_browser_suggests_browser_open() {
        let err = clear(
            fresh(),
            ClearInput {
                selector: css("input"),
                return_snapshot: false,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn focus_with_no_browser_suggests_browser_open() {
        let err = focus(
            fresh(),
            FocusInput {
                selector: css("input"),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn scroll_into_view_with_no_browser_suggests_browser_open() {
        let err = scroll_into_view(
            fresh(),
            ScrollInput {
                selector: css("footer"),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    #[tokio::test]
    async fn upload_with_no_browser_suggests_browser_open() {
        let err = upload(
            fresh(),
            UploadInput {
                selector: css("input[type=file]"),
                paths: vec!["/tmp/a.txt".into()],
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert_suggests_browser_open(&err);
    }

    // ---- parse_key -----------------------------------------------------

    #[test]
    fn parse_key_accepts_canonical_special_names() {
        assert_eq!(parse_key("Enter").unwrap(), Key::Special(SpecialKey::Enter));
        assert_eq!(parse_key("Tab").unwrap(), Key::Special(SpecialKey::Tab));
        assert_eq!(
            parse_key("Backspace").unwrap(),
            Key::Special(SpecialKey::Backspace)
        );
        assert_eq!(
            parse_key("ArrowUp").unwrap(),
            Key::Special(SpecialKey::ArrowUp)
        );
        assert_eq!(parse_key("F5").unwrap(), Key::Special(SpecialKey::F5));
    }

    #[test]
    fn parse_key_is_case_insensitive_for_special_names() {
        assert_eq!(parse_key("enter").unwrap(), Key::Special(SpecialKey::Enter));
        assert_eq!(parse_key("ENTER").unwrap(), Key::Special(SpecialKey::Enter));
        assert_eq!(
            parse_key("arrowdown").unwrap(),
            Key::Special(SpecialKey::ArrowDown)
        );
        assert_eq!(parse_key("ESC").unwrap(), Key::Special(SpecialKey::Escape));
    }

    #[test]
    fn parse_key_treats_single_char_as_char_variant() {
        assert_eq!(parse_key("a").unwrap(), Key::Char('a'));
        assert_eq!(parse_key("?").unwrap(), Key::Char('?'));
        // Single Unicode scalar should also route to Char.
        assert_eq!(parse_key("é").unwrap(), Key::Char('é'));
    }

    #[test]
    fn parse_key_rejects_unknown_special_with_accepted_list() {
        let err = parse_key("Zomg").expect_err("unknown key must error");
        assert!(
            err.message.contains("Unknown key `Zomg`"),
            "msg: {}",
            err.message
        );
        // The error must list valid alternatives so an agent can recover
        // without external docs — sample a few well-known names.
        assert!(err.message.contains("Enter"), "msg: {}", err.message);
        assert!(err.message.contains("Backspace"), "msg: {}", err.message);
        assert!(err.message.contains("F1..F12"), "msg: {}", err.message);
    }

    #[test]
    fn parse_key_accepts_arrow_aliases_and_return_alias() {
        // Friendlier aliases so agents don't have to remember the exact
        // CamelCase form.
        assert_eq!(
            parse_key("return").unwrap(),
            Key::Special(SpecialKey::Enter)
        );
        assert_eq!(parse_key("up").unwrap(), Key::Special(SpecialKey::ArrowUp));
        assert_eq!(
            parse_key("down").unwrap(),
            Key::Special(SpecialKey::ArrowDown)
        );
        assert_eq!(
            parse_key("left").unwrap(),
            Key::Special(SpecialKey::ArrowLeft)
        );
        assert_eq!(
            parse_key("right").unwrap(),
            Key::Special(SpecialKey::ArrowRight)
        );
    }

    // ---- MouseButtonArg ------------------------------------------------

    #[test]
    fn mouse_button_arg_maps_to_zendriver_variants() {
        assert_eq!(MouseButton::from(MouseButtonArg::Left), MouseButton::Left);
        assert_eq!(
            MouseButton::from(MouseButtonArg::Middle),
            MouseButton::Middle
        );
        assert_eq!(MouseButton::from(MouseButtonArg::Right), MouseButton::Right);
    }
}
