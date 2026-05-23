//! Keyboard types + dispatch (dispatch impl lands in Task 7).

use bitflags::bitflags;

/// A single key dispatch target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Special(SpecialKey),
}

/// Named non-character keys for `Element::press`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Insert,
    CapsLock,
    NumLock,
    ScrollLock,
    PrintScreen,
    Pause,
    ContextMenu,
}

impl SpecialKey {
    /// Maps to CDP `Input.dispatchKeyEvent` fields (code, key, windowsVirtualKeyCode).
    #[must_use]
    pub fn to_cdp(self) -> (&'static str, &'static str, i32) {
        match self {
            SpecialKey::Enter => ("Enter", "Enter", 13),
            SpecialKey::Tab => ("Tab", "Tab", 9),
            SpecialKey::Escape => ("Escape", "Escape", 27),
            SpecialKey::Backspace => ("Backspace", "Backspace", 8),
            SpecialKey::Delete => ("Delete", "Delete", 46),
            SpecialKey::Space => ("Space", " ", 32),
            SpecialKey::ArrowUp => ("ArrowUp", "ArrowUp", 38),
            SpecialKey::ArrowDown => ("ArrowDown", "ArrowDown", 40),
            SpecialKey::ArrowLeft => ("ArrowLeft", "ArrowLeft", 37),
            SpecialKey::ArrowRight => ("ArrowRight", "ArrowRight", 39),
            SpecialKey::Home => ("Home", "Home", 36),
            SpecialKey::End => ("End", "End", 35),
            SpecialKey::PageUp => ("PageUp", "PageUp", 33),
            SpecialKey::PageDown => ("PageDown", "PageDown", 34),
            SpecialKey::F1 => ("F1", "F1", 112),
            SpecialKey::F2 => ("F2", "F2", 113),
            SpecialKey::F3 => ("F3", "F3", 114),
            SpecialKey::F4 => ("F4", "F4", 115),
            SpecialKey::F5 => ("F5", "F5", 116),
            SpecialKey::F6 => ("F6", "F6", 117),
            SpecialKey::F7 => ("F7", "F7", 118),
            SpecialKey::F8 => ("F8", "F8", 119),
            SpecialKey::F9 => ("F9", "F9", 120),
            SpecialKey::F10 => ("F10", "F10", 121),
            SpecialKey::F11 => ("F11", "F11", 122),
            SpecialKey::F12 => ("F12", "F12", 123),
            SpecialKey::Insert => ("Insert", "Insert", 45),
            SpecialKey::CapsLock => ("CapsLock", "CapsLock", 20),
            SpecialKey::NumLock => ("NumLock", "NumLock", 144),
            SpecialKey::ScrollLock => ("ScrollLock", "ScrollLock", 145),
            SpecialKey::PrintScreen => ("PrintScreen", "PrintScreen", 44),
            SpecialKey::Pause => ("Pause", "Pause", 19),
            SpecialKey::ContextMenu => ("ContextMenu", "ContextMenu", 93),
        }
    }
}

bitflags! {
    /// Composable keyboard modifier bits. Matches CDP modifier-bits encoding.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct KeyModifiers: u8 {
        const ALT     = 0b0001;
        const CTRL    = 0b0010;
        const META    = 0b0100;
        const SHIFT   = 0b1000;
    }
}

impl KeyModifiers {
    /// Encode as the integer modifier bitmask CDP expects.
    #[must_use]
    pub fn cdp_bits(self) -> i32 {
        i32::from(self.bits())
    }
}

/// Returns a plausible nearby QWERTY key for `c`, or None for non-alphanumeric.
/// Used by realistic typing to inject occasional typos.
pub(crate) fn neighbor_key(c: char, rng: &mut impl rand::Rng) -> Option<char> {
    use rand::seq::SliceRandom;
    let lower = c.to_ascii_lowercase();
    let neighbors: &[char] = match lower {
        'q' => &['w', 'a', 's'],
        'w' => &['q', 'e', 'a', 's', 'd'],
        'e' => &['w', 'r', 's', 'd', 'f'],
        'r' => &['e', 't', 'd', 'f', 'g'],
        't' => &['r', 'y', 'f', 'g', 'h'],
        'y' => &['t', 'u', 'g', 'h', 'j'],
        'u' => &['y', 'i', 'h', 'j', 'k'],
        'i' => &['u', 'o', 'j', 'k', 'l'],
        'o' => &['i', 'p', 'k', 'l'],
        'p' => &['o', 'l'],
        'a' => &['q', 'w', 's', 'z'],
        's' => &['a', 'd', 'w', 'e', 'z', 'x'],
        'd' => &['s', 'f', 'e', 'r', 'x', 'c'],
        'f' => &['d', 'g', 'r', 't', 'c', 'v'],
        'g' => &['f', 'h', 't', 'y', 'v', 'b'],
        'h' => &['g', 'j', 'y', 'u', 'b', 'n'],
        'j' => &['h', 'k', 'u', 'i', 'n', 'm'],
        'k' => &['j', 'l', 'i', 'o', 'm'],
        'l' => &['k', 'o', 'p'],
        'z' => &['a', 's', 'x'],
        'x' => &['z', 'c', 's', 'd'],
        'c' => &['x', 'v', 'd', 'f'],
        'v' => &['c', 'b', 'f', 'g'],
        'b' => &['v', 'n', 'g', 'h'],
        'n' => &['b', 'm', 'h', 'j'],
        'm' => &['n', 'j', 'k'],
        _ => return None,
    };
    let pick = neighbors.choose(rng)?;
    if c.is_ascii_uppercase() {
        Some(pick.to_ascii_uppercase())
    } else {
        Some(*pick)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn modifiers_compose_with_bitor() {
        let m = KeyModifiers::CTRL | KeyModifiers::SHIFT;
        assert!(m.contains(KeyModifiers::CTRL));
        assert!(m.contains(KeyModifiers::SHIFT));
        assert!(!m.contains(KeyModifiers::ALT));
    }

    #[test]
    fn modifiers_cdp_bits_match_encoding() {
        assert_eq!(KeyModifiers::ALT.cdp_bits(), 1);
        assert_eq!(KeyModifiers::CTRL.cdp_bits(), 2);
        assert_eq!(KeyModifiers::META.cdp_bits(), 4);
        assert_eq!(KeyModifiers::SHIFT.cdp_bits(), 8);
        assert_eq!((KeyModifiers::CTRL | KeyModifiers::SHIFT).cdp_bits(), 10);
    }

    #[test]
    fn special_key_enter_maps_to_cdp_13() {
        let (code, key, vk) = SpecialKey::Enter.to_cdp();
        assert_eq!(code, "Enter");
        assert_eq!(key, "Enter");
        assert_eq!(vk, 13);
    }

    #[test]
    fn neighbor_key_returns_nearby_for_alpha() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let n = neighbor_key('r', &mut rng).expect("r has neighbors");
        assert!(['e', 't', 'd', 'f', 'g'].contains(&n));
    }

    #[test]
    fn neighbor_key_preserves_case() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let n = neighbor_key('R', &mut rng).expect("R has neighbors");
        assert!(n.is_ascii_uppercase());
    }

    #[test]
    fn neighbor_key_returns_none_for_non_alpha() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        assert!(neighbor_key('5', &mut rng).is_none());
        assert!(neighbor_key('!', &mut rng).is_none());
        assert!(neighbor_key(' ', &mut rng).is_none());
    }
}

use std::time::Duration;

use serde_json::json;

use crate::error::Result;
use crate::input::InputController;
use crate::tab::Tab;

/// Dispatch a single character via Input.dispatchKeyEvent (keyDown + keyUp).
pub(crate) async fn dispatch_char(tab: &Tab, c: char, modifier_bits: i32) -> Result<()> {
    let s = c.to_string();
    tab.session()
        .call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown", "text": &s, "key": &s,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
    tab.session()
        .call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp", "text": &s, "key": &s,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
    Ok(())
}

/// Dispatch a named special key (Enter, Tab, etc).
#[allow(dead_code)]
pub(crate) async fn dispatch_special(tab: &Tab, k: SpecialKey, modifier_bits: i32) -> Result<()> {
    let (code, key, vk) = k.to_cdp();
    tab.session()
        .call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "rawKeyDown",
                "code": code, "key": key,
                "windowsVirtualKeyCode": vk,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
    tab.session()
        .call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "code": code, "key": key,
                "windowsVirtualKeyCode": vk,
                "modifiers": modifier_bits,
            }),
        )
        .await?;
    Ok(())
}

/// Type `text` with realistic per-character timing, occasional typos, and
/// inter-word "thinking" pauses pulled from the InputProfile.
#[allow(dead_code)]
pub(crate) async fn type_text_realistic(
    input: &InputController,
    tab: &Tab,
    text: &str,
) -> Result<()> {
    let profile = input.profile.clone();
    for ch in text.chars() {
        let (per_char_delay_ms, mods, do_typo, typo_char, thinking_pause_ms) = {
            let mut s = input.state.lock().await;
            let per_char = if profile.per_char_delay_ms_range.0 == 0
                && profile.per_char_delay_ms_range.1 == 0
            {
                0
            } else {
                rand::Rng::gen_range(
                    &mut s.rng,
                    profile.per_char_delay_ms_range.0..=profile.per_char_delay_ms_range.1,
                )
            };
            let do_typo =
                profile.typo_rate > 0.0 && rand::Rng::gen::<f32>(&mut s.rng) < profile.typo_rate;
            let typo_char = if do_typo {
                neighbor_key(ch, &mut s.rng)
            } else {
                None
            };
            let thinking = if ch == ' '
                && profile.thinking_pause_ms_range.0 > 0
                && rand::Rng::gen::<f32>(&mut s.rng) < 0.05
            {
                rand::Rng::gen_range(
                    &mut s.rng,
                    profile.thinking_pause_ms_range.0..=profile.thinking_pause_ms_range.1,
                )
            } else {
                0
            };
            (
                per_char,
                s.modifiers_held.cdp_bits(),
                do_typo,
                typo_char,
                thinking,
            )
        };
        if per_char_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(per_char_delay_ms as u64)).await;
        }
        if thinking_pause_ms > 0 {
            tokio::time::sleep(Duration::from_millis(thinking_pause_ms as u64)).await;
        }
        if do_typo {
            if let Some(wrong) = typo_char {
                dispatch_char(tab, wrong, mods).await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
                dispatch_special(tab, SpecialKey::Backspace, mods).await?;
            }
        }
        dispatch_char(tab, ch, mods).await?;
    }
    Ok(())
}

/// Type `text` as fast as possible — no delays, no typos.
#[allow(dead_code)]
pub(crate) async fn type_text_raw(input: &InputController, tab: &Tab, text: &str) -> Result<()> {
    let mods = input.state.lock().await.modifiers_held.cdp_bits();
    for ch in text.chars() {
        dispatch_char(tab, ch, mods).await?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod dispatch_tests {
    use super::*;
    use serde_json::Value;
    use zendriver_stealth::InputProfile;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

    #[tokio::test]
    async fn type_text_raw_emits_keydown_keyup_per_char() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());
        let input = InputController::new_with_seed(InputProfile::native(), 42);

        let fut = tokio::spawn({
            let input = input.clone();
            let tab = tab.clone();
            async move { type_text_raw(&input, &tab, "ab").await }
        });

        for ch in ['a', 'a', 'b', 'b'] {
            // 4 events: a-down a-up b-down b-up
            let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
            let last = mock.last_sent();
            let text = last["params"]["text"].as_str().unwrap();
            assert_eq!(text, ch.to_string());
            mock.reply(id, Value::Null).await;
        }
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }

    #[tokio::test]
    async fn dispatch_special_enter_emits_correct_cdp_fields() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new(sess, std::sync::Weak::new());

        let fut = tokio::spawn({
            let tab = tab.clone();
            async move { dispatch_special(&tab, SpecialKey::Enter, 0).await }
        });

        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["type"], "rawKeyDown");
        assert_eq!(last["params"]["key"], "Enter");
        assert_eq!(last["params"]["windowsVirtualKeyCode"], 13);
        mock.reply(id, Value::Null).await;

        let id = mock.expect_cmd("Input.dispatchKeyEvent").await;
        let last = mock.last_sent();
        assert_eq!(last["params"]["type"], "keyUp");
        mock.reply(id, Value::Null).await;
        fut.await.unwrap().unwrap();
        conn.shutdown();
    }
}
