//! [`Element`] keyboard input: `type_text` / `type_text_fast` / `press` /
//! `press_with`.
//!
//! Each method focuses this element first (so keystrokes route to it),
//! then dispatches via the shared [`crate::input::InputController`].
//! `type_text` / `type_text_fast` route through realistic/fast typing
//! helpers, and `press` / `press_with` issue a single keydown/keyup pair.
//!
//! Everything wraps in an internal "refresh on stale" wrapper so a stale
//! handle transparently re-resolves once and retries.
//!
//! `press_with` deliberately does NOT mutate the controller's tracked
//! held-modifier state — passing the modifier bits straight to the
//! dispatch helpers avoids holding the `InputController` mutex across
//! `.await` (which would serialize every CDP call on the Browser) and
//! keeps the held-modifier state from drifting if a dispatch errors
//! mid-flight.

use crate::element::Element;
use crate::error::Result;
use crate::input::keyboard::{self, Key, KeyModifiers, KeyPress, KeySequence};

impl Element {
    /// Focus this element, then type `text` with realistic timing.
    ///
    /// Per-character delays, occasional typos, and "thinking" pauses are
    /// pulled from the active [`crate::input::InputController`]'s
    /// [`zendriver_stealth::InputProfile`].
    ///
    /// Use [`Element::type_text_fast`] when you want a deterministic
    /// keystroke sequence (no delays, no typos) — that path is preferable
    /// for tests + fast automation flows.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input[name=q]").one().await?;
    /// input.type_text("hello world").await?;
    /// # Ok(()) }
    /// ```
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

    /// Focus this element, then dispatch `text` with no delays.
    ///
    /// One character at a time with no typos. Each character becomes a
    /// `keyDown` + `keyUp` pair via `Input.dispatchKeyEvent`.
    ///
    /// Use [`Element::type_text`] when realism matters (driving a page
    /// that's keystroke-timing-sensitive, paths under the `spoofed`
    /// stealth profile, etc.).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input").one().await?;
    /// input.type_text_fast("test").await?;
    /// # Ok(()) }
    /// ```
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
    /// currently-held modifiers.
    ///
    /// Reads held modifiers from the tab's [`crate::input::InputController`].
    /// For dispatches that hold ad-hoc modifiers (e.g. Ctrl+A, Shift+End),
    /// use [`Element::press_with`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::{Key, SpecialKey};
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input").one().await?;
    /// input.press(Key::Special(SpecialKey::Enter)).await?;
    /// # Ok(()) }
    /// ```
    pub async fn press(&self, key: Key) -> Result<()> {
        self.with_refresh(|| async move {
            self.focus().await?;
            let input = self.inner.tab.input().clone();
            let mods = input.state.lock().await.modifiers_held;
            dispatch_key(&self.inner.tab, key, mods).await
        })
        .await
    }

    /// Focus this element, then dispatch a single [`Key`] with `mods`
    /// held during the dispatch.
    ///
    /// The held bits replace (rather than merge with) any modifiers already
    /// tracked in the controller for the duration of this one dispatch —
    /// `mods` is passed straight through to the CDP `modifiers` field; the
    /// tracked state isn't mutated. See the module-level docs for rationale.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::{Key, KeyModifiers, SpecialKey};
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input").one().await?;
    /// // Ctrl+A (select all)
    /// input.press_with(Key::Char('a'), KeyModifiers::CTRL).await?;
    /// # Ok(()) }
    /// ```
    pub async fn press_with(&self, key: Key, mods: KeyModifiers) -> Result<()> {
        self.with_refresh(|| async move {
            self.focus().await?;
            dispatch_key(&self.inner.tab, key, mods).await
        })
        .await
    }

    /// Focus this element, then dispatch a mixed [`KeySequence`] in order.
    ///
    /// Parity with zendriver-py's `from_mixed_input`: chain literal text,
    /// special-key presses, and modifier chords, then send them as one
    /// ordered key-event stream. Text segments type grapheme-by-grapheme
    /// (full unicode / shift / emoji handling, same as [`Element::type_text`]).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::{Key, KeyModifiers, KeySequence, SpecialKey};
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let input = tab.find().css("input").one().await?;
    /// input
    ///     .type_keys(
    ///         KeySequence::new()
    ///             .text("hello")
    ///             .key(SpecialKey::Enter)
    ///             .chord(Key::Char('a'), KeyModifiers::CTRL), // Ctrl+A
    ///     )
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn type_keys(&self, seq: KeySequence) -> Result<()> {
        self.with_refresh(|| {
            let seq = seq.clone();
            async move {
                self.focus().await?;
                let events = seq.to_events();
                keyboard::dispatch_key_events(&self.inner.tab, &events).await
            }
        })
        .await
    }
}

/// Single-key dispatch helper — builds the full keyDown/keyUp (plus modifier
/// wrapper) event sequence via [`keyboard::key_events`] and dispatches it.
/// Kept private (consumers call the `press` / `press_with` methods instead).
async fn dispatch_key(tab: &crate::tab::Tab, key: Key, mods: KeyModifiers) -> Result<()> {
    let events = keyboard::key_events(key, mods, KeyPress::DownAndUp);
    keyboard::dispatch_key_events(tab, &events).await
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::keyboard::SpecialKey;
    use crate::tab::Tab;
    use serde_json::{Value, json};
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

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
        assert!(
            sent["params"]["functionDeclaration"]
                .as_str()
                .unwrap()
                .contains("this.focus()")
        );
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

    /// Drain the focus() actionability gate (visible → enabled) plus the
    /// final `this.focus()` call, replying to each.
    async fn drain_focus(mock: &mut MockConnection) {
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
    }

    #[tokio::test]
    async fn press_with_ctrl_a_emits_control_wrapper_events() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 17, "R17".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.press_with(Key::Char('a'), KeyModifiers::CTRL).await }
        });

        drain_focus(&mut mock).await;

        // Ctrl+A: Control-down, a-down, a-up, Control-up. Conventional order
        // releases the main key *before* the modifier, so the a-keyUp still
        // reports Ctrl held (modifiers=2); only the Control-keyUp clears it.
        let expected = [
            ("ControlLeft", "keyDown", 2),
            ("KeyA", "keyDown", 2),
            ("KeyA", "keyUp", 2),
            ("ControlLeft", "keyUp", 0),
        ];
        for (code, kind, mods) in expected {
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            let last = mock.last_sent();
            assert_eq!(last["params"]["code"].as_str().unwrap(), code);
            assert_eq!(last["params"]["type"].as_str().unwrap(), kind);
            assert_eq!(last["params"]["modifiers"].as_i64().unwrap(), mods);
            mock.reply(id, Value::Null).await;
        }

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn type_keys_flattens_text_key_and_chord_in_order() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 17, "R17".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move {
                e.type_keys(
                    KeySequence::new()
                        .text("hi")
                        .key(SpecialKey::Enter)
                        .chord(Key::Char('a'), KeyModifiers::CTRL),
                )
                .await
            }
        });

        drain_focus(&mut mock).await;

        // "hi" → h/i down+up; Enter → rawKeyDown+keyUp; Ctrl+a → Control
        // wrap around a. 10 dispatch events total.
        let expected = [
            ("h", "keyDown"),
            ("h", "keyUp"),
            ("i", "keyDown"),
            ("i", "keyUp"),
            ("Enter", "rawKeyDown"),
            ("Enter", "keyUp"),
            ("Control", "keyDown"),
            ("a", "keyDown"),
            ("a", "keyUp"),
            ("Control", "keyUp"),
        ];
        for (key, kind) in expected {
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            let last = mock.last_sent();
            assert_eq!(last["params"]["key"].as_str().unwrap(), key);
            assert_eq!(last["params"]["type"].as_str().unwrap(), kind);
            mock.reply(id, Value::Null).await;
        }

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn type_text_fast_emoji_emits_single_char_event() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let el = Element::from_jsret(tab.clone(), 17, "R17".to_string());

        let fut = tokio::spawn({
            let e = el.clone();
            async move { e.type_text_fast("🚀").await }
        });

        drain_focus(&mut mock).await;

        // Emoji has no physical-key descriptor → one `char`-type event.
        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["type"].as_str().unwrap(), "char");
        assert_eq!(last["params"]["text"].as_str().unwrap(), "🚀");
        assert!(last["params"].get("code").is_none());
        mock.reply(id, Value::Null).await;

        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
