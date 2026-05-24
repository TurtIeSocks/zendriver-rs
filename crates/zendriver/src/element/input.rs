//! `Element` keyboard input: `type_text` / `type_text_fast` / `press` /
//! `press_with`.
//!
//! Each method focuses this element first (so keystrokes route to it),
//! then dispatches via the shared [`InputController`] — `type_text` /
//! `type_text_fast` route through [`keyboard::type_text_realistic`] /
//! [`keyboard::type_text_fast`], and `press` / `press_with` issue a single
//! [`keyboard::dispatch_char`] or [`keyboard::dispatch_special`] depending
//! on whether the [`Key`] is a character or a named special key.
//!
//! Everything wraps in [`Element::with_refresh`] so a stale handle
//! transparently re-resolves once and retries.
//!
//! `press_with` deliberately does NOT mutate
//! [`crate::input::InputState::modifiers_held`] — passing the modifier
//! bits straight to the dispatch helpers avoids holding the
//! `InputController` mutex across `.await` (which would serialize every
//! CDP call on the Browser) and keeps the held-modifier state from
//! drifting if a dispatch errors mid-flight. The downside: nested
//! sequences like "hold Shift, click, then press Tab" still need to drive
//! the state mutex directly — that's a P4 concern when the public
//! input-state API lands.
//!
//! Post-P4 T0: `Tab::input()` returns `&Arc<InputController>` directly
//! (the controller lives on `TabInner` now, not the owning Browser).
//! Tests build a `Tab` via [`crate::tab::Tab::new_for_test`], which seeds
//! a deterministic native [`InputController`] (seed `42`).

use crate::element::Element;
use crate::error::Result;
use crate::input::keyboard::{self, Key, KeyModifiers};

impl Element {
    /// Focus this element, then type `text` with realistic per-character
    /// timing, occasional typos, and "thinking" pauses pulled from the
    /// active [`crate::input::InputController`]'s
    /// [`zendriver_stealth::InputProfile`].
    ///
    /// Use [`Element::type_text_fast`] when you want a deterministic
    /// keystroke sequence (no delays, no typos) — that path is preferable
    /// for tests + fast automation flows.
    pub async fn type_text(&self, text: impl AsRef<str>) -> Result<()> {
        let text = text.as_ref().to_string();
        self.with_refresh(|| {
            let text = text.clone();
            async move {
                self.focus().await?;
                let input = self.inner.tab.input().clone();
                keyboard::type_text_realistic(&input, &self.inner.tab, &text).await
            }
        })
        .await
    }

    /// Focus this element, then dispatch `text` one character at a time
    /// with no delays and no typos. Each character becomes a `keyDown` +
    /// `keyUp` pair via `Input.dispatchKeyEvent`.
    ///
    /// Use [`Element::type_text`] when realism matters (driving a page
    /// that's keystroke-timing-sensitive, paths under the `spoofed`
    /// stealth profile, etc.).
    pub async fn type_text_fast(&self, text: impl AsRef<str>) -> Result<()> {
        let text = text.as_ref().to_string();
        self.with_refresh(|| {
            let text = text.clone();
            async move {
                self.focus().await?;
                let input = self.inner.tab.input().clone();
                keyboard::type_text_fast(&input, &self.inner.tab, &text).await
            }
        })
        .await
    }

    /// Focus this element, then dispatch a single [`Key`] with any
    /// currently-held modifiers (read from
    /// [`crate::input::InputState::modifiers_held`]).
    ///
    /// For dispatches that hold ad-hoc modifiers (e.g. Ctrl+A, Shift+End),
    /// use [`Element::press_with`].
    pub async fn press(&self, key: Key) -> Result<()> {
        self.with_refresh(|| async move {
            self.focus().await?;
            let input = self.inner.tab.input().clone();
            let mods = input.state.lock().await.modifiers_held.cdp_bits();
            dispatch_key(&self.inner.tab, key, mods).await
        })
        .await
    }

    /// Focus this element, then dispatch a single [`Key`] with `mods`
    /// held during the dispatch. The held bits replace (rather than
    /// merge with) any modifiers already tracked in
    /// [`crate::input::InputState::modifiers_held`] for the duration of
    /// this one dispatch — `mods` is passed straight through to the CDP
    /// `modifiers` field; the tracked state isn't mutated. See the
    /// module-level docs for the rationale (avoids holding the input
    /// mutex across `.await` + keeps held-modifier state from drifting
    /// on dispatch errors).
    pub async fn press_with(&self, key: Key, mods: KeyModifiers) -> Result<()> {
        self.with_refresh(|| async move {
            self.focus().await?;
            dispatch_key(&self.inner.tab, key, mods.cdp_bits()).await
        })
        .await
    }
}

/// Single-key dispatch helper — routes `Key::Char` to `dispatch_char` and
/// `Key::Special` to `dispatch_special`. Kept private (consumers call
/// the `press` / `press_with` methods instead).
async fn dispatch_key(tab: &crate::tab::Tab, key: Key, modifier_bits: i32) -> Result<()> {
    match key {
        Key::Char(c) => keyboard::dispatch_char(tab, c, modifier_bits).await,
        Key::Special(k) => keyboard::dispatch_special(tab, k, modifier_bits).await,
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tab::Tab;
    use serde_json::{json, Value};
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn type_text_fast_emits_two_dispatchkeyevent_per_char() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        // `Tab::new_for_test` seeds the native input profile with a
        // deterministic seed; the raw type path doesn't sample the RNG
        // (no jitter, no typos), so seed value is irrelevant here.
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 17, "R17".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.type_text_fast("hi").await }
        });

        // focus() runs the actionability gate first: visible → enabled.
        // ActionabilityCheck::TEXT_INPUT skips stable + receives_pointer.
        for _ in 0..2 {
            let id = mock.expect_cmd("Runtime.callFunctionOn").await;
            mock.reply(
                id,
                json!({ "result": { "value": true, "type": "boolean" } }),
            )
            .await;
        }
        // focus() then calls el.focus() — one more Runtime.callFunctionOn.
        let id = mock.expect_cmd("Runtime.callFunctionOn").await;
        let sent = mock.last_sent();
        assert!(sent["params"]["functionDeclaration"]
            .as_str()
            .unwrap()
            .contains("this.focus()"));
        mock.reply(id, json!({ "result": { "type": "undefined" } }))
            .await;

        // type_text_fast "hi" emits 4 Input.dispatchKeyEvent calls in
        // order: h-down, h-up, i-down, i-up.
        let expected = [
            ("h", "keyDown"),
            ("h", "keyUp"),
            ("i", "keyDown"),
            ("i", "keyUp"),
        ];
        for (ch, kind) in expected {
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            let last = mock.last_sent();
            assert_eq!(last["params"]["text"].as_str().unwrap(), ch);
            assert_eq!(last["params"]["type"].as_str().unwrap(), kind);
            mock.reply(id, Value::Null).await;
        }

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
