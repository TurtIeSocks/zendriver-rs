//! [`Element`] actions: `click` / `hover` / `focus` / `scroll_into_view` /
//! `set_value` / `set_text` / `clear` / `upload_files` / `flash` /
//! `highlight_overlay`.
//!
//! Each action wraps its CDP dispatch sequence in an internal "refresh on
//! stale" wrapper so a stale handle (post-navigation, post-React-rerender)
//! transparently re-resolves once and retries.
//!
//! `hover` / `hover_fast`:
//!   1. `scroll_into_view` — bring the element into the viewport so its
//!      bbox center is a real, dispatchable coordinate.
//!   2. Actionability gate (`visible + stable + receives_pointer`) — avoids
//!      mid-transition hover races and overlay occlusion. Pointer events
//!      are dispatched at the geometric center, so the element must be the
//!      actual hit-test target there. `enabled` is left off — hover doesn't
//!      activate the element, so disabled controls still accept mouseover.
//!   3. Compute bbox center (`x + width / 2`, `y + height / 2`).
//!   4. Dispatch the mouse-move via the shared [`crate::input::InputController`].
//!      `hover` uses a realistic Bezier path; `hover_fast` uses a single
//!      teleport dispatch for test/automation paths.
//!
//! `focus`: actionability gate (visible + enabled — no pointer or stability
//! requirement; focus routes through the focused element, not the cursor's
//! position), then `el.focus()` in the main world.
//!
//! `scroll_into_view`: no actionability gate (this *is* the visibility
//! prereq other actions wait for). Calls `el.scrollIntoView({ block:
//! 'center', behavior: 'instant' })`. `block: 'center'` matches Playwright
//! (avoids sticky headers/footers obscuring the element after the scroll);
//! `behavior: 'instant'` skips animation so the post-scroll bbox is final
//! by the time the next CDP call runs.

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::input::keyboard::{Key, KeyModifiers, SpecialKey};
use crate::input::mouse::{self, MouseButton};
use crate::query::actionability::{self, ActionabilityCheck};

/// Default deadline for the actionability gate before each action. Matches
/// the value the spec calls out for P3; per-call override land in P4 when
/// the per-action options structs grow.
const DEFAULT_ACTIONABILITY_TIMEOUT: Duration = Duration::from_secs(5);

/// JS body that assigns `arguments[1].value` through the *native* prototype
/// value-setter and fires bubbled `input` + `change` events.
///
/// React installs a per-instance `_valueTracker` whose own `value` setter
/// shadows the prototype's. A direct `this.value = v` goes through that
/// tracker, which updates its cached value and the DOM together — so the
/// `input` event it then sees reports "no change" and React skips `onChange`,
/// reverting the field on the next render. Calling the *prototype's*
/// `value` setter (resolved via `Object.getOwnPropertyDescriptor`) bypasses
/// the tracker, leaving its cached value stale so React fires `onChange`
/// correctly. Falls back to a plain assignment when the descriptor has no
/// setter (non-input elements). Resolves the `<textarea>` prototype for
/// textareas and the `<input>` prototype otherwise.
const NATIVE_VALUE_SETTER_JS: &str = "function(el, v){ \
        const proto = this instanceof HTMLTextAreaElement \
            ? HTMLTextAreaElement.prototype \
            : HTMLInputElement.prototype; \
        const d = Object.getOwnPropertyDescriptor(proto, 'value'); \
        if (d && d.set) { d.set.call(this, v); } else { this.value = v; } \
        this.dispatchEvent(new Event('input', {bubbles: true})); \
        this.dispatchEvent(new Event('change', {bubbles: true})); \
    }";

/// Extra Backspace presses [`Element::clear_by_deleting`] issues beyond the
/// reported value length — covers off-by-one reporting and IME composition
/// tails so a near-empty field still ends up fully cleared.
const CLEAR_BY_DELETING_SLACK: usize = 2;

/// Hard ceiling on Backspace presses in [`Element::clear_by_deleting`].
/// Bounds the keystroke loop so a pathological / lying `value.length` can't
/// spin it unboundedly.
const CLEAR_BY_DELETING_MAX: usize = 4096;

/// Per-call knobs for [`Element::click_with`].
///
/// `Default` matches the behavior of [`Element::click`]: a left, single,
/// realistic click at the element's bbox center with no modifiers held and
/// full actionability gating. Override fields individually for richer
/// dispatches (right-click, modifier-held click, raw teleport, etc.).
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// use zendriver::{ClickOptions, MouseButton};
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// let el = tab.find().css("button").one().await?;
/// el.click_with(ClickOptions {
///     button: MouseButton::Right,
///     click_count: 2,
///     ..Default::default()
/// }).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ClickOptions {
    /// Which mouse button to dispatch. [`MouseButton::Left`] by default.
    pub button: MouseButton,
    /// Modifier keys held during the dispatch. Empty by default.
    pub modifiers: KeyModifiers,
    /// `clickCount` for the CDP dispatch. `1` by default; set `2` for a
    /// double-click in a single `click_with` call.
    pub click_count: u32,
    /// Skip the actionability gate when true. Use sparingly — bypasses the
    /// visibility/stability/pointer checks. Mirrors Playwright's
    /// `force: true`.
    pub force: bool,
    /// Bezier-interpolated cursor path (`true`) vs single teleport
    /// dispatch (`false`). `true` by default; the `click_fast` shortcut
    /// flips this for deterministic test paths.
    pub realistic: bool,
    /// Click position relative to the element's bbox top-left
    /// (`(dx, dy)`). `None` clicks at the bbox center.
    pub position: Option<(f64, f64)>,
}

impl Default for ClickOptions {
    fn default() -> Self {
        Self {
            button: MouseButton::Left,
            modifiers: KeyModifiers::empty(),
            click_count: 1,
            force: false,
            realistic: true,
            position: None,
        }
    }
}

impl Element {
    /// Click this element with realistic defaults.
    ///
    /// Left button, single click, Bezier-path cursor approach, and the full
    /// actionability gate. Equivalent to
    /// `click_with(ClickOptions::default())`. For right-click / modifier-held
    /// / double-click / raw-teleport variations, use [`Element::click_with`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::NotActionable`] when the actionability gate
    /// times out, [`ZendriverError::ElementStale`] when the handle can't be
    /// refreshed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().css("button#submit").one().await?;
    /// btn.click().await?;
    /// # Ok(()) }
    /// ```
    pub async fn click(&self) -> Result<()> {
        self.click_with(ClickOptions::default()).await
    }

    /// Click this element with a deterministic raw teleport.
    ///
    /// Skips the Bezier interpolation [`Element::click`] uses and bypasses
    /// the actionability gate. Equivalent to
    /// `click_with(ClickOptions { realistic: false, force: true, ..Default::default() })`.
    /// Intended for test paths and fast automation flows where realism
    /// and per-action gating get in the way.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().css("button").one().await?;
    /// btn.click_fast().await?;
    /// # Ok(()) }
    /// ```
    pub async fn click_fast(&self) -> Result<()> {
        self.click_with(ClickOptions {
            realistic: false,
            force: true,
            ..Default::default()
        })
        .await
    }

    /// Click this element with explicit [`ClickOptions`].
    ///
    /// See module docs for the dispatch sequence — same shape as `hover`
    /// (scroll → gate → bbox math → pointer dispatch) but emits the
    /// `mousePressed` + `mouseReleased` pair after the cursor arrives.
    /// `opts.force` skips the actionability gate; `opts.position` shifts
    /// the click point off bbox-center.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::{ClickOptions, MouseButton};
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let row = tab.find().css("tr.contact").one().await?;
    /// row.click_with(ClickOptions {
    ///     button: MouseButton::Right,
    ///     ..Default::default()
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn click_with(&self, opts: ClickOptions) -> Result<()> {
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            if !opts.force {
                actionability::wait_actionable(
                    self,
                    ActionabilityCheck::FULL,
                    DEFAULT_ACTIONABILITY_TIMEOUT,
                )
                .await?;
            }
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let (tx, ty) = match opts.position {
                Some((dx, dy)) => (bbox.x + dx, bbox.y + dy),
                None => (bbox.x + bbox.width / 2.0, bbox.y + bbox.height / 2.0),
            };
            let input = self.inner.tab.input().clone();
            mouse::click_at(
                &input,
                &self.inner.tab,
                tx,
                ty,
                opts.button,
                opts.click_count,
                opts.realistic,
            )
            .await
        })
        .await
    }

    /// Hover the cursor over this element's bbox center.
    ///
    /// Uses a realistic Bezier-interpolated mouse path. See module docs for
    /// the full sequence (`scroll_into_view` → actionability gate → bbox
    /// center → dispatch). Use [`Element::hover_fast`] when the cursor path
    /// doesn't matter.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let nav = tab.find().css("nav.dropdown").one().await?;
    /// nav.hover().await?;
    /// # Ok(()) }
    /// ```
    pub async fn hover(&self) -> Result<()> {
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            actionability::wait_actionable(
                self,
                ActionabilityCheck {
                    visible: true,
                    stable: true,
                    enabled: false,
                    receives_pointer: true,
                },
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let cx = bbox.x + bbox.width / 2.0;
            let cy = bbox.y + bbox.height / 2.0;
            let input = self.inner.tab.input().clone();
            mouse::move_realistic(&input, &self.inner.tab, cx, cy).await
        })
        .await
    }

    /// Hover the cursor over this element's bbox center via a single
    /// teleport.
    ///
    /// Skips the Bezier interpolation [`Element::hover`] does — same
    /// actionability gate + bbox math, but no human-pointer modeling.
    /// Intended for paths where deterministic timing matters more than
    /// realism.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css("button").one().await?;
    /// el.hover_fast().await?;
    /// # Ok(()) }
    /// ```
    pub async fn hover_fast(&self) -> Result<()> {
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            actionability::wait_actionable(
                self,
                ActionabilityCheck {
                    visible: true,
                    stable: true,
                    enabled: false,
                    receives_pointer: true,
                },
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let bbox = self
                .bounding_box()
                .await?
                .ok_or_else(|| ZendriverError::Navigation("element has no bounding box".into()))?;
            let cx = bbox.x + bbox.width / 2.0;
            let cy = bbox.y + bbox.height / 2.0;
            let input = self.inner.tab.input().clone();
            mouse::move_raw(&input, &self.inner.tab, cx, cy).await
        })
        .await
    }

    /// Move keyboard focus to this element by calling `el.focus()`.
    ///
    /// Gated by an actionability check (visible + enabled) so disabled
    /// controls + hidden elements surface a [`ZendriverError::NotActionable`]
    /// error rather than silently no-op on the page side. Reused by
    /// [`Element::type_text`] / [`Element::press`] — they focus first so
    /// keystrokes reach this element.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input[name=email]").one().await?;
    /// input.focus().await?;
    /// # Ok(()) }
    /// ```
    pub async fn focus(&self) -> Result<()> {
        self.with_refresh(|| async move {
            actionability::wait_actionable(
                self,
                ActionabilityCheck::TEXT_INPUT,
                DEFAULT_ACTIONABILITY_TIMEOUT,
            )
            .await?;
            let _ = self
                .call_on_main("function(){ this.focus(); }", json!([]))
                .await?;
            Ok(())
        })
        .await
    }

    /// Scroll this element into view, centered in its scroll container.
    ///
    /// Synchronous (`behavior: 'instant'`) so the post-scroll bbox is final
    /// by the time the next CDP call (e.g. `bounding_box`) runs — important
    /// because subsequent action steps assume the layout is settled.
    ///
    /// No actionability gate: this method IS the visibility prerequisite
    /// for the other actions; gating it on visibility would deadlock.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let footer = tab.find().css("footer").one().await?;
    /// footer.scroll_into_view().await?;
    /// # Ok(()) }
    /// ```
    pub async fn scroll_into_view(&self) -> Result<()> {
        self.with_refresh(|| async move {
            let _ = self
                .call_on_main(
                    "function(){ this.scrollIntoView({block:'center',behavior:'instant'}); }",
                    json!([]),
                )
                .await?;
            Ok(())
        })
        .await
    }

    /// Set this element's `value` + fire bubbled `input` and `change` events.
    ///
    /// Assigns through the element's *native* prototype value-setter rather
    /// than a direct `this.value =` so React's per-instance `_valueTracker`
    /// can't silently revert the change on the next render. See
    /// [`NATIVE_VALUE_SETTER_JS`] for the full rationale. Bypasses
    /// keydown/keyup (use [`Element::type_text`] for a real keystroke
    /// sequence) but the bubbled `input` + `change` events let React-style
    /// controlled inputs see the update through their onChange handlers.
    ///
    /// No actionability gate — `set_value` is the fast-path for tests +
    /// automation flows that don't care about visibility/enabledness. If
    /// you need the gate, focus the element first then call this.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input[name=q]").one().await?;
    /// input.set_value("rust async").await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_value(&self, value: impl AsRef<str>) -> Result<()> {
        let value = value.as_ref().to_string();
        self.with_refresh(|| {
            let value = value.clone();
            async move {
                let _ = self
                    .call_on_main(NATIVE_VALUE_SETTER_JS, json!([{ "value": value }]))
                    .await?;
                Ok(())
            }
        })
        .await
    }

    /// Clear this element's `value` by setting it to the empty string + firing
    /// bubbled `input` and `change` events.
    ///
    /// Routes through the same native prototype value-setter as
    /// [`Element::set_value`] (see [`NATIVE_VALUE_SETTER_JS`]) so React
    /// controlled inputs don't revert the clear on the next render. For inputs
    /// that ignore a programmatic value-set entirely (custom keystroke
    /// handling), use [`Element::clear_by_deleting`]; for contenteditable /
    /// non-`<input>` clearing, use [`Element::type_text`].
    ///
    /// No actionability gate — same rationale as [`Element::set_value`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input").one().await?;
    /// input.clear().await?;
    /// # Ok(()) }
    /// ```
    pub async fn clear(&self) -> Result<()> {
        self.with_refresh(|| async move {
            let _ = self
                .call_on_main(NATIVE_VALUE_SETTER_JS, json!([{ "value": "" }]))
                .await?;
            Ok(())
        })
        .await
    }

    /// Replace this element's text content with `value`.
    ///
    /// Sets `this.textContent = value` in the main world. Ports nodriver's
    /// `Element.set_text` (element.py:771), which writes the first text-child
    /// node's value via `DOM.setNodeValue`; assigning `textContent` is the
    /// simpler equivalent with the same observable result — the element's
    /// rendered text is replaced (and any child element nodes are dropped,
    /// matching `textContent` assignment semantics).
    ///
    /// Unlike [`Element::set_value`] this targets the element's *text*, not a
    /// form control's `value`, and fires no `input` / `change` events (a
    /// `textContent` write isn't a user edit). For `<input>` / `<textarea>`
    /// value edits use [`Element::set_value`] / [`Element::type_text`].
    ///
    /// No actionability gate — same fast-path rationale as
    /// [`Element::set_value`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let heading = tab.find().css("h1").one().await?;
    /// heading.set_text("New title").await?;
    /// # Ok(()) }
    /// ```
    pub async fn set_text(&self, value: impl AsRef<str>) -> Result<()> {
        let value = value.as_ref().to_string();
        self.with_refresh(|| {
            let value = value.clone();
            async move {
                let _ = self
                    .call_on_main(
                        "function(v){ this.textContent = v; }",
                        json!([{ "value": value }]),
                    )
                    .await?;
                Ok(())
            }
        })
        .await
    }

    /// Clear this element by keystroke: focus, select-all, then press
    /// Backspace until the field empties.
    ///
    /// A real-keystroke fallback for inputs that ignore a programmatic
    /// value-set (e.g. fields with custom `onKeyDown` handling that
    /// [`Element::clear`] can't drive). Sequence:
    ///
    /// 1. [`Element::focus`] the element.
    /// 2. Select-all chord — `Cmd+A` on macOS, `Ctrl+A` elsewhere — so the
    ///    whole value is selected before deletion.
    /// 3. Read the current `value.length`, then press
    ///    [`SpecialKey::Backspace`] that many times plus a small slack
    ///    ([`CLEAR_BY_DELETING_SLACK`]), bounded by [`CLEAR_BY_DELETING_MAX`].
    ///
    /// Deletes backward (Backspace) only — never forward-Delete — because
    /// `VK_DELETE` at caret position 0 is treated as a backward delete on some
    /// VM environments, which would no-op and spin forever.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input.react-controlled").one().await?;
    /// input.clear_by_deleting().await?;
    /// # Ok(()) }
    /// ```
    pub async fn clear_by_deleting(&self) -> Result<()> {
        self.with_refresh(|| async move {
            self.focus().await?;
            // Select-all so the field's whole value is highlighted; Backspace
            // then clears the selection in one stroke before falling back to
            // per-character deletion for any stragglers.
            let select_all = if cfg!(target_os = "macos") {
                KeyModifiers::META
            } else {
                KeyModifiers::CTRL
            };
            self.press_with(Key::Char('a'), select_all).await?;

            let len: usize = {
                let res = self
                    .call_on_main("function(){ return (this.value || '').length; }", json!([]))
                    .await?;
                res.get("value")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0)
            };
            let presses = len
                .saturating_add(CLEAR_BY_DELETING_SLACK)
                .min(CLEAR_BY_DELETING_MAX);
            for _ in 0..presses {
                self.press(Key::Special(SpecialKey::Backspace)).await?;
            }
            Ok(())
        })
        .await
    }

    /// Attach files to this `<input type="file">` element via
    /// `DOM.setFileInputFiles`.
    ///
    /// Bypasses the OS file picker entirely — CDP wires the paths straight
    /// into the input's `FileList`, and the page sees a normal `change`
    /// event from the input.
    ///
    /// Scope is direct `<input type="file">` only; routing through a hidden
    /// input clicked by a label / button wrapper is the page's
    /// responsibility. Paths are passed as their lossy `to_string_lossy()`
    /// representation, matching CDP's UTF-8 string contract.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let file_input = tab.find().css("input[type=file]").one().await?;
    /// file_input.upload_files(&["/tmp/photo.jpg"]).await?;
    /// # Ok(()) }
    /// ```
    pub async fn upload_files<P: AsRef<Path>>(&self, paths: &[P]) -> Result<()> {
        let files: Vec<String> = paths
            .iter()
            .map(|p| p.as_ref().to_string_lossy().into_owned())
            .collect();
        self.with_refresh(|| {
            let files = files.clone();
            async move {
                let backend_node_id = self.backend_node_id_cloned().await?;
                let _ = self
                    .inner
                    .tab
                    .call(
                        "DOM.setFileInputFiles",
                        json!({
                            "files": files,
                            "backendNodeId": backend_node_id,
                        }),
                    )
                    .await?;
                Ok(())
            }
        })
        .await
    }

    /// Flash a transient colored dot at this element's bbox center.
    ///
    /// Scrolls the element into view, computes its bbox center, then injects
    /// a small absolutely-positioned `<div>` there that removes itself after
    /// `duration`. A debugging aid for eyeballing where pointer actions land
    /// against a headful Chrome — no input state changes, not for production.
    ///
    /// Ports nodriver's `Element.flash` (element.py:913): nodriver resolves
    /// the node, takes its position, and calls `Tab.flash_point(*center)`
    /// (its richer self-animating overlay is commented out and its `duration`
    /// argument unused). This mirrors the same dot-at-center behavior but
    /// honors `duration` for the self-removal timer rather than hardcoding it,
    /// matching the sibling [`crate::Tab::flash_point`] dispatch shape.
    ///
    /// No-ops gracefully (returns `Ok`) when the element has no box (e.g.
    /// `display: none`), since there is nothing to point at.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # use std::time::Duration;
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().css("button").one().await?;
    /// btn.flash(Duration::from_millis(500)).await?;
    /// # Ok(()) }
    /// ```
    pub async fn flash(&self, duration: Duration) -> Result<()> {
        let ms = duration.as_millis();
        self.with_refresh(|| async move {
            self.scroll_into_view().await?;
            let bbox = match self.bounding_box().await? {
                Some(b) => b,
                // No box to point at — nothing to flash.
                None => return Ok(()),
            };
            let cx = bbox.x + bbox.width / 2.0;
            let cy = bbox.y + bbox.height / 2.0;
            // Inject the dot in the main world with the element bound (house
            // pattern); the args carry the viewport-center coords + lifetime.
            // The dot is `position:fixed` so the coords are viewport-relative,
            // matching `bounding_box`'s frame and `Tab::flash_point`.
            let js = "function(cx, cy, ms){ \
                    const d = document.createElement('div'); \
                    d.style.cssText = 'position:fixed;left:'+cx+'px;top:'+cy+'px;\
width:10px;height:10px;margin:-5px 0 0 -5px;border-radius:50%;background:red;\
z-index:2147483647;pointer-events:none;opacity:0.85;'; \
                    document.body.appendChild(d); \
                    setTimeout(() => d.remove(), ms); \
                }";
            let _ = self
                .call_on_main(
                    js,
                    json!([{ "value": cx }, { "value": cy }, { "value": ms }]),
                )
                .await?;
            Ok(())
        })
        .await
    }

    /// Highlight this element DevTools-inspector style.
    ///
    /// Enables the Overlay domain, then dispatches `Overlay.highlightNode`
    /// with this element's backend node id and a default
    /// [`HighlightConfig`](https://chromedevtools.github.io/devtools-protocol/tot/Overlay/#type-HighlightConfig)
    /// (content/padding/border/margin boxes + info tooltip), painting the
    /// familiar colored box-model overlay over the element.
    ///
    /// Ports nodriver's `Element.highlight_overlay` (element.py:1001).
    /// nodriver toggles the highlight on repeat calls via an internal
    /// `_is_highlighted` flag; this is the show-only half — the overlay
    /// persists until a navigation, an `Overlay.hideHighlight`, or another
    /// highlight replaces it. A debug-visualization helper, not a production
    /// path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css(".target").one().await?;
    /// el.highlight_overlay().await?;
    /// # Ok(()) }
    /// ```
    pub async fn highlight_overlay(&self) -> Result<()> {
        self.with_refresh(|| async move {
            let backend_node_id = self.backend_node_id_cloned().await?;
            // Overlay.highlightNode is a no-op unless the Overlay domain is
            // enabled; enable it first (idempotent).
            let _ = self.inner.tab.call("Overlay.enable", json!({})).await?;
            let _ = self
                .inner
                .tab
                .call(
                    "Overlay.highlightNode",
                    json!({
                        "backendNodeId": backend_node_id,
                        "highlightConfig": {
                            "showInfo": true,
                            "showExtensionLines": true,
                            "showStyles": true,
                            "contentColor": { "r": 111, "g": 168, "b": 220, "a": 0.66 },
                            "paddingColor": { "r": 147, "g": 196, "b": 125, "a": 0.55 },
                            "borderColor":  { "r": 255, "g": 229, "b": 153, "a": 0.66 },
                            "marginColor":  { "r": 246, "g": 178, "b": 107, "a": 0.66 },
                        },
                    }),
                )
                .await?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tab::Tab;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn hover_dispatches_input_dispatchmouseevent_with_type_mousemoved() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        // `Tab::new_for_test` seeds an `InputController` from the native
        // profile (fast: 10 px/ms ⇒ 0.5 ms segment delay; zero jitter ⇒
        // stable Bezier output) with deterministic seed 42 — pinning the
        // RNG path in case a future profile tweak adds entropy.
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.hover().await }
        });

        // Step 1: scroll_into_view → Runtime.callFunctionOn.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains("scrollIntoView")
        );
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: actionability gate runs check_visible first.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;
        // check_stable (gate order: visible → enabled → stable → receives_pointer;
        // enabled is disabled for hover, so stable is next).
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;
        // check_receives_pointer.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(
            id,
            json!({ "result": { "value": true, "type": "boolean" } }),
        )
        .await;

        // Step 3: bounding_box → DOM.getBoxModel.
        let id = mock.expect_cmd("DOM.getBoxModel").await;
        mock.reply(
            id,
            json!({
                "model": {
                    "content": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "padding": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "border":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "margin":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "width":  100,
                    "height": 50
                }
            }),
        )
        .await;

        // Step 4: mouse move — Bezier path emits N=9..=61
        // `Input.dispatchMouseEvent { type: mouseMoved }` calls. Drain
        // each one, asserting type=mouseMoved along the way; stop once
        // the future completes (no more dispatches arrive within the
        // window).
        let mut saw_mouse_moved = false;
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    let kind = sent["params"]["type"].as_str().unwrap_or("");
                    assert_eq!(
                        kind, "mouseMoved",
                        "hover should only emit mouseMoved events"
                    );
                    saw_mouse_moved = true;
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        let res = fut.await.unwrap();
        res.unwrap();
        assert!(
            saw_mouse_moved,
            "expected at least one Input.dispatchMouseEvent with type=mouseMoved"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn click_dispatches_mousemoved_then_mousepressed_then_mousereleased() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        // `Tab::new_for_test` seeds the native input profile (fast, zero
        // jitter) with deterministic seed 42. Bezier path still emits N
        // mouseMoved frames, then exactly one mousePressed + one
        // mouseReleased — the per-tab `InputController` lives on `TabInner`
        // since P4 T0 (no `Browser::input()` dance any more).
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 99, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.click().await }
        });

        // Step 1: scroll_into_view → Runtime.callFunctionOn.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: actionability gate (FULL = visible → enabled → stable →
        // receives_pointer); reply true to each.
        for _ in 0..4 {
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(
                id,
                json!({ "result": { "value": true, "type": "boolean" } }),
            )
            .await;
        }

        // Step 3: bounding_box → DOM.getBoxModel.
        let id = mock.expect_cmd("DOM.getBoxModel").await;
        mock.reply(
            id,
            json!({
                "model": {
                    "content": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "padding": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "border":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "margin":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "width":  100,
                    "height": 50
                }
            }),
        )
        .await;

        // Step 4: dispatch sequence. Bezier emits N mouseMoved frames, then
        // exactly one mousePressed + one mouseReleased. Walk all
        // `Input.dispatchMouseEvent` calls and assert ordering: every
        // mouseMoved precedes mousePressed, which precedes mouseReleased.
        let mut saw_pressed = false;
        let mut saw_released = false;
        let mut last_kind = String::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(500),
                mock.expect_cmd("Input.dispatchMouseEvent"),
            )
            .await;
            match next {
                Ok(id) => {
                    let sent = mock.last_sent();
                    let kind = sent["params"]["type"].as_str().unwrap_or("").to_string();
                    match kind.as_str() {
                        "mouseMoved" => {
                            assert!(
                                !saw_pressed && !saw_released,
                                "mouseMoved arrived after mousePressed/Released"
                            );
                        }
                        "mousePressed" => {
                            assert!(!saw_pressed, "duplicate mousePressed");
                            assert!(!saw_released, "mousePressed after mouseReleased");
                            saw_pressed = true;
                        }
                        "mouseReleased" => {
                            assert!(saw_pressed, "mouseReleased before mousePressed");
                            assert!(!saw_released, "duplicate mouseReleased");
                            saw_released = true;
                        }
                        other => panic!("unexpected dispatch type: {other}"),
                    }
                    last_kind = kind;
                    mock.reply(id, json!({})).await;
                }
                Err(_) => break,
            }
        }

        let res = fut.await.unwrap();
        res.unwrap();
        assert!(saw_pressed, "expected a mousePressed dispatch");
        assert!(saw_released, "expected a mouseReleased dispatch");
        assert_eq!(
            last_kind, "mouseReleased",
            "final dispatch should be mouseReleased"
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn set_value_uses_native_prototype_setter() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 7, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.set_value("hello world").await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        // Assign through the native prototype value-setter so React's
        // instance-level `_valueTracker` doesn't revert the change on the
        // next render (a direct `this.value = v` would).
        assert!(decl.contains("getOwnPropertyDescriptor"));
        assert!(decl.contains(".set.call"));
        // Resolves the right prototype for <textarea> vs <input>.
        assert!(decl.contains("HTMLTextAreaElement"));
        assert!(decl.contains("HTMLInputElement"));
        // Fires both input + change events for React-style listeners.
        assert!(decl.contains("'input'"));
        assert!(decl.contains("'change'"));
        // call_on_main prepends the element {objectId:...}; the user-supplied
        // value lands at arguments[1].
        let args = sent["params"]["arguments"].as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0]["objectId"], "R1");
        assert_eq!(args[1]["value"], "hello world");
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn clear_uses_native_setter_with_empty() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 7, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.clear().await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        // `clear` routes through the same native-setter JS as `set_value`,
        // assigning an empty string.
        assert!(decl.contains("getOwnPropertyDescriptor"));
        assert!(decl.contains(".set.call"));
        assert!(decl.contains("HTMLTextAreaElement"));
        assert!(decl.contains("HTMLInputElement"));
        assert!(decl.contains("'input'"));
        assert!(decl.contains("'change'"));
        // The cleared value is the empty string, passed at arguments[1].
        let args = sent["params"]["arguments"].as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0]["objectId"], "R1");
        assert_eq!(args[1]["value"], "");
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn set_text_assigns_textcontent_with_value() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 7, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.set_text("New title").await }
        });

        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        // Faithful-simpler port of nodriver's DOM.setNodeValue: assign
        // textContent so the element's rendered text is replaced.
        assert!(decl.contains("textContent"));
        // call_on_main prepends the element {objectId:...}; the user-supplied
        // value lands at arguments[1].
        let args = sent["params"]["arguments"].as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0]["objectId"], "R1");
        assert_eq!(args[1]["value"], "New title");
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn clear_by_deleting_focuses_then_selects_then_backspaces() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 7, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.clear_by_deleting().await }
        });

        // Step 1: explicit focus() — actionability gate (visible → enabled)
        // then this.focus().
        for _ in 0..2 {
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(
                id,
                json!({ "result": { "value": true, "type": "boolean" } }),
            )
            .await;
        }
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains("this.focus()")
        );
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: select-all chord via press_with(Key::Char('a'), Ctrl|Meta).
        // press_with focuses first (gate visible → enabled, then this.focus()),
        // then emits the chord. Since A2 (full keyboard parity) press_with
        // dispatches the modifier as REAL wrapper key events, so the chord is
        // four dispatches: modifier keyDown → 'a' keyDown → 'a' keyUp →
        // modifier keyUp. Ctrl on Windows/Linux, Meta on macOS.
        for _ in 0..2 {
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(
                id,
                json!({ "result": { "value": true, "type": "boolean" } }),
            )
            .await;
        }
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;
        let ctrl = i64::from(KeyModifiers::CTRL.cdp_bits());
        let meta = i64::from(KeyModifiers::META.cdp_bits());
        // 1: modifier keyDown (Meta or Control).
        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "keyDown");
        let mod_key = sent["params"]["key"].as_str().unwrap();
        assert!(
            mod_key == "Meta" || mod_key == "Control",
            "select-all chord must wrap with a Meta/Control keyDown, got {mod_key}"
        );
        mock.reply(id, json!({})).await;
        // 2: 'a' keyDown with the modifier bit set.
        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["type"], "keyDown");
        assert_eq!(sent["params"]["key"], "a");
        let mods = sent["params"]["modifiers"].as_i64().unwrap();
        assert!(
            mods == ctrl || mods == meta,
            "select-all 'a' must hold Ctrl or Meta, got {mods}"
        );
        mock.reply(id, json!({})).await;
        // 3: 'a' keyUp.
        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        assert_eq!(mock.last_sent()["params"]["type"], "keyUp");
        mock.reply(id, json!({})).await;
        // 4: modifier keyUp.
        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        assert_eq!(mock.last_sent()["params"]["type"], "keyUp");
        mock.reply(id, json!({})).await;

        // Step 3: read current value length via call_on_main. Reply length 0
        // so only the fixed slack-count of Backspaces follows — keeps the
        // remaining frame sequence deterministic regardless of the value the
        // page reports.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert!(
            sent["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains(".value")
        );
        mock.reply(id, json!({ "result": { "value": 0, "type": "number" } }))
            .await;

        // Step 4: with reported length 0 the impl still presses Backspace a
        // small fixed slack number of times so a near-empty field still gets
        // a couple of deletes. Each press(Backspace) re-focuses (gate:
        // visible → enabled, then this.focus()) then dispatches
        // rawKeyDown + keyUp for the Backspace virtual key. The whole
        // sequence is deterministic; drive every frame explicitly.
        let mut saw_backspace = false;
        for _ in 0..CLEAR_BY_DELETING_SLACK {
            // press(Backspace) focus: 2 gate calls + this.focus().
            for _ in 0..2 {
                let id = mock.expect_cmd("Runtime.callFunctionOn").await;
                mock.reply(id, json!({ "result": { "value": true, "type": "boolean" } }))
                    .await;
            }
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(id, json!({ "result": { "type": "undefined" } }))
                .await;
            // rawKeyDown for Backspace (never forward Delete).
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            let sent = mock.last_sent();
            assert_eq!(sent["params"]["type"], "rawKeyDown");
            assert_eq!(sent["params"]["key"], "Backspace");
            assert_ne!(sent["params"]["key"], "Delete", "must never forward-Delete");
            saw_backspace = true;
            mock.reply(id, json!({})).await;
            // keyUp.
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            assert_eq!(mock.last_sent()["params"]["type"], "keyUp");
            mock.reply(id, json!({})).await;
        }

        let res = fut.await.unwrap();
        res.unwrap();
        assert!(saw_backspace, "expected at least one Backspace keyDown");
        conn.shutdown();
    }

    #[tokio::test]
    async fn upload_files_dispatches_dom_set_file_input_files_with_paths() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 42, "R1".to_string());

        let paths: &[&std::path::Path] = &[
            std::path::Path::new("/tmp/a.txt"),
            std::path::Path::new("/tmp/b.pdf"),
        ];

        let fut = tokio::spawn({
            let e = el.clone();
            let paths: Vec<std::path::PathBuf> = paths.iter().map(|p| p.to_path_buf()).collect();
            async move { e.upload_files(&paths).await }
        });

        let id = mock.expect_cmd("DOM.setFileInputFiles").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["backendNodeId"], 42);
        let files = sent["params"]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], "/tmp/a.txt");
        assert_eq!(files[1], "/tmp/b.pdf");
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn flash_injects_self_removing_overlay_at_bbox_center() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 42, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.flash(Duration::from_millis(500)).await }
        });

        // Step 1: scroll_into_view → Runtime.callFunctionOn.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        assert!(
            mock.last_sent()["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains("scrollIntoView")
        );
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // Step 2: bounding_box → DOM.getBoxModel. Box top-left (10,20), 100x50
        // ⇒ center (60, 45).
        let id = mock.expect_cmd("DOM.getBoxModel").await;
        mock.reply(
            id,
            json!({
                "model": {
                    "content": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "padding": [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "border":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "margin":  [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0],
                    "width":  100,
                    "height": 50
                }
            }),
        )
        .await;

        // Step 3: overlay injected via Runtime.callFunctionOn carrying the
        // dot-building JS (createElement + setTimeout/remove), with the
        // viewport-center coords + duration passed as arguments.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        let decl = sent["params"]["functionDeclaration"].as_str().unwrap();
        assert!(decl.contains("createElement"), "should build a dot element");
        assert!(
            decl.contains("setTimeout") && decl.contains("remove"),
            "should self-remove"
        );
        let args = sent["params"]["arguments"].as_array().unwrap();
        // call_on_main prepends the element {objectId}; then cx, cy, ms.
        assert_eq!(args[0]["objectId"], "R1");
        assert_eq!(args[1]["value"], 60.0); // center x
        assert_eq!(args[2]["value"], 45.0); // center y
        assert_eq!(args[3]["value"], 500); // duration ms
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn highlight_overlay_enables_then_highlights_backend_node() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab, 42, "R1".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.highlight_overlay().await }
        });

        // Step 1: Overlay.enable (highlightNode is a no-op without it).
        let id = mock.expect_cmd("Overlay.enable").await;
        mock.reply(id, json!({})).await;

        // Step 2: Overlay.highlightNode { backendNodeId, highlightConfig }.
        let id = mock.expect_cmd("Overlay.highlightNode").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["backendNodeId"], 42);
        assert!(
            sent["params"]["highlightConfig"].is_object(),
            "should carry a HighlightConfig"
        );
        mock.reply(id, json!({})).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
