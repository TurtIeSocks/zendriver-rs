//! Keyboard types: [`Key`] / [`SpecialKey`] / [`KeyModifiers`].
//!
//! Pass these to [`crate::Element::press`] / [`crate::Element::press_with`]
//! to dispatch single keystrokes.

use bitflags::bitflags;
use unicode_segmentation::UnicodeSegmentation;

/// A single key dispatch target — either a typed character or a named
/// special key.
///
/// # Examples
///
/// ```
/// use zendriver::{Key, SpecialKey};
/// let _ = Key::Char('a');
/// let _ = Key::Special(SpecialKey::Enter);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A typed character.
    Char(char),
    /// A named non-character key (Enter, Tab, F1, etc.).
    Special(SpecialKey),
}

/// Named non-character keys for [`crate::Element::press`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    /// Return / Enter.
    Enter,
    /// Tab.
    Tab,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Delete (forward delete).
    Delete,
    /// Space bar.
    Space,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// F1.
    F1,
    /// F2.
    F2,
    /// F3.
    F3,
    /// F4.
    F4,
    /// F5.
    F5,
    /// F6.
    F6,
    /// F7.
    F7,
    /// F8.
    F8,
    /// F9.
    F9,
    /// F10.
    F10,
    /// F11.
    F11,
    /// F12.
    F12,
    /// Insert.
    Insert,
    /// Caps Lock.
    CapsLock,
    /// Num Lock.
    NumLock,
    /// Scroll Lock.
    ScrollLock,
    /// Print Screen.
    PrintScreen,
    /// Pause / Break.
    Pause,
    /// Context Menu / Application key.
    ContextMenu,
}

impl SpecialKey {
    /// Map to CDP `Input.dispatchKeyEvent` fields.
    ///
    /// Returns `(code, key, windowsVirtualKeyCode)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::SpecialKey;
    /// let (code, key, vk) = SpecialKey::Enter.to_cdp();
    /// assert_eq!(code, "Enter");
    /// assert_eq!(key, "Enter");
    /// assert_eq!(vk, 13);
    /// ```
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
    /// Composable keyboard modifier bits.
    ///
    /// Matches CDP modifier-bits encoding. Combine with `|`:
    ///
    /// ```
    /// use zendriver::KeyModifiers;
    /// let combo = KeyModifiers::CTRL | KeyModifiers::SHIFT;
    /// assert!(combo.contains(KeyModifiers::CTRL));
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct KeyModifiers: u8 {
        /// Alt key (Option on macOS).
        const ALT     = 0b0001;
        /// Control key.
        const CTRL    = 0b0010;
        /// Meta key (Command on macOS, Windows key on Windows).
        const META    = 0b0100;
        /// Shift key.
        const SHIFT   = 0b1000;
    }
}

impl KeyModifiers {
    /// Encode as the integer modifier bitmask CDP expects.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::KeyModifiers;
    /// assert_eq!(KeyModifiers::CTRL.cdp_bits(), 2);
    /// assert_eq!((KeyModifiers::CTRL | KeyModifiers::SHIFT).cdp_bits(), 10);
    /// ```
    #[must_use]
    pub fn cdp_bits(self) -> i32 {
        i32::from(self.bits())
    }
}

/// CDP key-event field bundle for a single printable character.
///
/// Returned by [`char_descriptor`]. `code` is the physical-key code
/// (`"KeyA"`, `"Digit1"`, `"Semicolon"`, ...), `windows_vk` the Windows
/// virtual key-code, and `shift` whether the character requires the Shift
/// modifier to produce (uppercase letters, `!`, `:`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CharDescriptor {
    /// CDP physical-key `code` (e.g. `"KeyA"`).
    pub code: &'static str,
    /// Windows virtual key-code.
    pub windows_vk: i32,
    /// Whether Shift is required to type this character.
    pub shift: bool,
}

/// Resolve a printable character to its CDP key descriptor, mirroring
/// zendriver-py's `core/keys.py` lookup tables.
///
/// Returns `None` for anything that has no single physical-key origin —
/// whitespace (callers route space/tab/newline to the matching
/// [`SpecialKey`]) and every non-ASCII / multi-codepoint / emoji character
/// (callers send those via a `char`-type CDP event instead).
pub(crate) fn char_descriptor(c: char) -> Option<CharDescriptor> {
    // a–z → KeyA..KeyZ, vk = ASCII of the uppercase letter, no shift.
    if c.is_ascii_lowercase() {
        let upper = c.to_ascii_uppercase();
        return Some(CharDescriptor {
            code: key_code_for_letter(upper),
            windows_vk: upper as i32,
            shift: false,
        });
    }
    // A–Z → KeyA..KeyZ, vk = ASCII of the letter, shift.
    if c.is_ascii_uppercase() {
        return Some(CharDescriptor {
            code: key_code_for_letter(c),
            windows_vk: c as i32,
            shift: true,
        });
    }
    // 0–9 → Digit0..Digit9, vk = ASCII of the digit, no shift.
    if c.is_ascii_digit() {
        return Some(CharDescriptor {
            code: digit_code_for(c),
            windows_vk: c as i32,
            shift: false,
        });
    }
    // Shifted digits: ")!@#$%^&*(" — index is the base digit, Shift held.
    if let Some(idx) = NUM_SHIFT.iter().position(|&s| s == c) {
        let base = b'0' + idx as u8;
        return Some(CharDescriptor {
            code: digit_code_for(base as char),
            windows_vk: i32::from(base),
            shift: true,
        });
    }
    // Unshifted punctuation table.
    if let Some(&(code, vk)) = SPECIAL_CHAR_MAP
        .iter()
        .find_map(|(k, v)| (*k == c).then_some(v))
    {
        return Some(CharDescriptor {
            code,
            windows_vk: vk,
            shift: false,
        });
    }
    // Shifted punctuation: reuse the base char's code/vk, Shift held.
    if let Some(base) = SPECIAL_CHAR_SHIFT_MAP
        .iter()
        .find_map(|(k, base)| (*k == c).then_some(*base))
    {
        if let Some(&(code, vk)) = SPECIAL_CHAR_MAP
            .iter()
            .find_map(|(k, v)| (*k == base).then_some(v))
        {
            return Some(CharDescriptor {
                code,
                windows_vk: vk,
                shift: true,
            });
        }
    }
    // Whitespace handled by callers as SpecialKey; everything else (non-ASCII,
    // accents, emoji, multi-codepoint) routes to a `char`-type event.
    None
}

/// `Digit{n}` for an ASCII digit char.
fn digit_code_for(c: char) -> &'static str {
    match c {
        '0' => "Digit0",
        '1' => "Digit1",
        '2' => "Digit2",
        '3' => "Digit3",
        '4' => "Digit4",
        '5' => "Digit5",
        '6' => "Digit6",
        '7' => "Digit7",
        '8' => "Digit8",
        _ => "Digit9",
    }
}

/// `Key{X}` for an uppercase ASCII letter.
fn key_code_for_letter(upper: char) -> &'static str {
    match upper {
        'A' => "KeyA",
        'B' => "KeyB",
        'C' => "KeyC",
        'D' => "KeyD",
        'E' => "KeyE",
        'F' => "KeyF",
        'G' => "KeyG",
        'H' => "KeyH",
        'I' => "KeyI",
        'J' => "KeyJ",
        'K' => "KeyK",
        'L' => "KeyL",
        'M' => "KeyM",
        'N' => "KeyN",
        'O' => "KeyO",
        'P' => "KeyP",
        'Q' => "KeyQ",
        'R' => "KeyR",
        'S' => "KeyS",
        'T' => "KeyT",
        'U' => "KeyU",
        'V' => "KeyV",
        'W' => "KeyW",
        'X' => "KeyX",
        'Y' => "KeyY",
        _ => "KeyZ",
    }
}

/// Shifted-digit row, indexed by base digit (0–9): `index → char`.
const NUM_SHIFT: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];

/// Unshifted punctuation → `(code, windowsVirtualKeyCode)`.
const SPECIAL_CHAR_MAP: [(char, (&str, i32)); 11] = [
    (';', ("Semicolon", 186)),
    ('=', ("Equal", 187)),
    (',', ("Comma", 188)),
    ('-', ("Minus", 189)),
    ('.', ("Period", 190)),
    ('/', ("Slash", 191)),
    ('`', ("Backquote", 192)),
    ('[', ("BracketLeft", 219)),
    ('\\', ("Backslash", 220)),
    (']', ("BracketRight", 221)),
    ('\'', ("Quote", 222)),
];

/// Shifted punctuation → base (unshifted) character whose code/vk it reuses.
const SPECIAL_CHAR_SHIFT_MAP: [(char, char); 11] = [
    (':', ';'),
    ('+', '='),
    ('<', ','),
    ('_', '-'),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
    ('{', '['),
    ('|', '\\'),
    ('}', ']'),
    ('"', '\''),
];

/// How a [`Key`] should be turned into CDP events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyPress {
    /// One `char`-type event (text insertion). Used for emoji / non-ASCII /
    /// multi-codepoint clusters that have no physical-key origin.
    // Constructed by the A2b public-API layer (`type_keys` / grapheme path).
    #[allow(dead_code)]
    Char,
    /// A full keyDown→keyUp pair (wrapped in modifier keyDown/keyUp events
    /// when modifiers are active).
    DownAndUp,
}

/// A single `Input.dispatchKeyEvent` payload, built by [`key_events`].
///
/// Serialized verbatim into the CDP call. `Option` fields are omitted from
/// the wire payload when `None` (a `char` event carries no `code`/vk; a
/// special-key event carries no `text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyEventPayload {
    /// CDP event type: `keyDown` / `keyUp` / `rawKeyDown` / `char`.
    pub event_type: &'static str,
    /// Active modifier bitmask at dispatch time.
    pub modifiers: i32,
    /// Inserted/typed text, if any.
    pub text: Option<String>,
    /// DOM `key` value, if any.
    pub key: Option<String>,
    /// Physical-key `code`, if any.
    pub code: Option<&'static str>,
    /// Windows virtual key-code, if any.
    pub windows_vk: Option<i32>,
}

impl KeyEventPayload {
    /// Render to the CDP `Input.dispatchKeyEvent` `params` object.
    pub fn to_cdp_params(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), self.event_type.into());
        obj.insert("modifiers".into(), self.modifiers.into());
        if let Some(text) = &self.text {
            obj.insert("text".into(), text.as_str().into());
        }
        if let Some(key) = &self.key {
            obj.insert("key".into(), key.as_str().into());
        }
        if let Some(code) = self.code {
            obj.insert("code".into(), code.into());
        }
        if let Some(vk) = self.windows_vk {
            obj.insert("windowsVirtualKeyCode".into(), vk.into());
            obj.insert("nativeVirtualKeyCode".into(), vk.into());
        }
        serde_json::Value::Object(obj)
    }
}

/// CDP descriptor for a modifier key when synthesized as a real keystroke.
/// Returns `(code, key, windowsVirtualKeyCode)`.
fn modifier_cdp(m: KeyModifiers) -> (&'static str, &'static str, i32) {
    match m {
        KeyModifiers::SHIFT => ("ShiftLeft", "Shift", 16),
        KeyModifiers::CTRL => ("ControlLeft", "Control", 17),
        KeyModifiers::ALT => ("AltLeft", "Alt", 18),
        KeyModifiers::META => ("MetaLeft", "Meta", 91),
        _ => unreachable!("modifier_cdp called with a non-single modifier"),
    }
}

/// Active modifiers in conventional press order: Alt, Ctrl, Meta, Shift.
fn ordered_modifiers(mods: KeyModifiers) -> impl Iterator<Item = KeyModifiers> {
    [
        KeyModifiers::ALT,
        KeyModifiers::CTRL,
        KeyModifiers::META,
        KeyModifiers::SHIFT,
    ]
    .into_iter()
    .filter(move |m| mods.contains(*m))
}

/// Build the ordered CDP key-event payloads for a single [`Key`] press.
///
/// - `KeyPress::Char`, or a `Key::Char` whose [`char_descriptor`] is `None`
///   (emoji / non-ASCII), emits a single `char`-type event carrying the
///   character as both `text` and `key`.
/// - `KeyPress::DownAndUp` on a printable `Key::Char` resolves the effective
///   modifiers (`mods` plus Shift if the descriptor requires it) and emits,
///   in conventional order: a keyDown for each active modifier (accumulating
///   the modifier bitmask) → the main keyDown → the main keyUp → modifier
///   keyUps in reverse.
/// - `KeyPress::DownAndUp` on a `Key::Special` does the same modifier wrap,
///   with the main events a `rawKeyDown`/`keyUp` pair built from
///   [`SpecialKey::to_cdp`] (no `text`).
pub(crate) fn key_events(key: Key, mods: KeyModifiers, kind: KeyPress) -> Vec<KeyEventPayload> {
    // char-event path: explicit Char kind, or a Char with no descriptor.
    let force_char = matches!(kind, KeyPress::Char);
    if let Key::Char(c) = key {
        let Some(d) = char_descriptor(c).filter(|_| !force_char) else {
            // Explicit Char kind, or emoji / non-ASCII with no descriptor.
            let s = c.to_string();
            return vec![KeyEventPayload {
                event_type: "char",
                modifiers: mods.cdp_bits(),
                text: Some(s.clone()),
                key: Some(s),
                code: None,
                windows_vk: None,
            }];
        };
        let effective = if d.shift {
            mods | KeyModifiers::SHIFT
        } else {
            mods
        };
        let main_down = KeyEventPayload {
            event_type: "keyDown",
            modifiers: effective.cdp_bits(),
            text: Some(c.to_string()),
            key: Some(c.to_string()),
            code: Some(d.code),
            windows_vk: Some(d.windows_vk),
        };
        let main_up = KeyEventPayload {
            event_type: "keyUp",
            ..main_down.clone()
        };
        return wrap_with_modifiers(effective, main_down, main_up);
    }

    // Special key.
    let Key::Special(k) = key else {
        unreachable!("Key::Char handled above");
    };
    if force_char {
        // char-event for a special key: send its key string as text (space,
        // enter→"\r", tab→"\t" handled by the caller mapping; here we use the
        // CDP `key`). Rarely used, but keep the contract total.
        let (_, key_str, _) = k.to_cdp();
        return vec![KeyEventPayload {
            event_type: "char",
            modifiers: mods.cdp_bits(),
            text: Some(key_str.to_string()),
            key: Some(key_str.to_string()),
            code: None,
            windows_vk: None,
        }];
    }
    let (code, key_str, vk) = k.to_cdp();
    let main_down = KeyEventPayload {
        event_type: "rawKeyDown",
        modifiers: mods.cdp_bits(),
        text: None,
        key: Some(key_str.to_string()),
        code: Some(code),
        windows_vk: Some(vk),
    };
    let main_up = KeyEventPayload {
        event_type: "keyUp",
        ..main_down.clone()
    };
    wrap_with_modifiers(mods, main_down, main_up)
}

/// Wrap a main keyDown/keyUp pair with modifier keyDown (accumulating bits,
/// conventional order) before and modifier keyUp (reverse, clearing bits)
/// after. `effective` is the full modifier set to synthesize.
fn wrap_with_modifiers(
    effective: KeyModifiers,
    main_down: KeyEventPayload,
    main_up: KeyEventPayload,
) -> Vec<KeyEventPayload> {
    let active: Vec<KeyModifiers> = ordered_modifiers(effective).collect();
    let mut events = Vec::with_capacity(active.len() * 2 + 2);

    // Modifier keyDowns, accumulating the bitmask as each is pressed.
    let mut acc = KeyModifiers::empty();
    for &m in &active {
        acc |= m;
        let (code, key_str, vk) = modifier_cdp(m);
        events.push(KeyEventPayload {
            event_type: "keyDown",
            modifiers: acc.cdp_bits(),
            text: None,
            key: Some(key_str.to_string()),
            code: Some(code),
            windows_vk: Some(vk),
        });
    }

    events.push(main_down);
    events.push(main_up);

    // Modifier keyUps in reverse, clearing the bitmask as each releases.
    for &m in active.iter().rev() {
        acc &= !m;
        let (code, key_str, vk) = modifier_cdp(m);
        events.push(KeyEventPayload {
            event_type: "keyUp",
            modifiers: acc.cdp_bits(),
            text: None,
            key: Some(key_str.to_string()),
            code: Some(code),
            windows_vk: Some(vk),
        });
    }

    events
}

/// Map a whitespace character to the [`SpecialKey`] that produces it.
///
/// Space → `Space`, tab → `Tab`, newline / carriage-return → `Enter`. These
/// are the three whitespace forms `char_descriptor` deliberately returns
/// `None` for, so typing paths route them here instead of a `char` event.
pub(crate) fn whitespace_special(c: char) -> Option<SpecialKey> {
    match c {
        ' ' => Some(SpecialKey::Space),
        '\t' => Some(SpecialKey::Tab),
        '\n' | '\r' => Some(SpecialKey::Enter),
        _ => None,
    }
}

/// Build the CDP events for one grapheme cluster, with `mods` held.
///
/// - A single `char` with a [`char_descriptor`] → `DownAndUp` (shift / code /
///   vk synthesis).
/// - A single whitespace `char` → the matching [`SpecialKey`] `DownAndUp`.
/// - Anything else (emoji with modifiers, combining sequences, CJK, accents)
///   → one `char`-type event carrying the whole cluster as text.
pub(crate) fn cluster_events(cluster: &str, mods: KeyModifiers) -> Vec<KeyEventPayload> {
    let mut chars = cluster.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        // Exactly one char in the cluster.
        if char_descriptor(c).is_some() {
            return key_events(Key::Char(c), mods, KeyPress::DownAndUp);
        }
        if let Some(special) = whitespace_special(c) {
            return key_events(Key::Special(special), mods, KeyPress::DownAndUp);
        }
    }
    // Multi-codepoint cluster, or a single char with no descriptor → char event.
    vec![KeyEventPayload {
        event_type: "char",
        modifiers: mods.cdp_bits(),
        text: Some(cluster.to_string()),
        key: Some(cluster.to_string()),
        code: None,
        windows_vk: None,
    }]
}

/// One step in a [`KeySequence`].
#[derive(Debug, Clone)]
enum KeyStep {
    /// Literal text, typed grapheme-by-grapheme like
    /// [`crate::Element::type_text`].
    Text(String),
    /// A single named key, pressed and released.
    Key(SpecialKey),
    /// A key held with modifier(s) — a chord like Ctrl+A.
    Chord(Key, KeyModifiers),
}

/// An ordered mixed sequence of typed text, special-key presses, and modifier
/// chords — parity with zendriver-py's `from_mixed_input`.
///
/// Build with the chainable methods, then dispatch via
/// [`crate::Element::type_keys`]. Steps flatten to CDP key events in the order
/// they were added.
///
/// # Examples
///
/// ```
/// use zendriver::{Key, KeyModifiers, KeySequence, SpecialKey};
/// let seq = KeySequence::new()
///     .text("Hello ")
///     .key(SpecialKey::Enter)
///     .chord(Key::Char('a'), KeyModifiers::CTRL); // Ctrl+A
/// # let _ = seq;
/// ```
#[derive(Debug, Clone, Default)]
pub struct KeySequence {
    steps: Vec<KeyStep>,
}

impl KeySequence {
    /// Start an empty sequence.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append literal text (typed grapheme-by-grapheme, with shift / emoji
    /// handling identical to [`crate::Element::type_text`]).
    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.steps.push(KeyStep::Text(text.into()));
        self
    }

    /// Append a single special-key press (down + up).
    #[must_use]
    pub fn key(mut self, key: SpecialKey) -> Self {
        self.steps.push(KeyStep::Key(key));
        self
    }

    /// Append a modifier chord — `key` pressed while `mods` are held, with
    /// real modifier keyDown/keyUp wrapper events.
    #[must_use]
    pub fn chord(mut self, key: Key, mods: KeyModifiers) -> Self {
        self.steps.push(KeyStep::Chord(key, mods));
        self
    }

    /// Flatten the whole sequence to ordered CDP key-event payloads.
    pub(crate) fn to_events(&self) -> Vec<KeyEventPayload> {
        let mut out = Vec::new();
        for step in &self.steps {
            match step {
                KeyStep::Text(text) => {
                    for cluster in UnicodeSegmentation::graphemes(text.as_str(), true) {
                        out.extend(cluster_events(cluster, KeyModifiers::empty()));
                    }
                }
                KeyStep::Key(k) => {
                    out.extend(key_events(
                        Key::Special(*k),
                        KeyModifiers::empty(),
                        KeyPress::DownAndUp,
                    ));
                }
                KeyStep::Chord(key, mods) => {
                    out.extend(key_events(*key, *mods, KeyPress::DownAndUp));
                }
            }
        }
        out
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

    // --- char_descriptor table ---

    #[test]
    fn char_descriptor_lowercase_letter_no_shift() {
        let d = char_descriptor('a').expect("a is printable");
        assert_eq!(d.code, "KeyA");
        assert_eq!(d.windows_vk, 65);
        assert!(!d.shift);
    }

    #[test]
    fn char_descriptor_uppercase_letter_shift() {
        let d = char_descriptor('A').expect("A is printable");
        assert_eq!(d.code, "KeyA");
        assert_eq!(d.windows_vk, 65);
        assert!(d.shift);
        // Lowercase and uppercase share code + vk; only shift differs.
        let lower = char_descriptor('a').unwrap();
        assert_eq!(d.code, lower.code);
        assert_eq!(d.windows_vk, lower.windows_vk);
    }

    #[test]
    fn char_descriptor_digit_no_shift() {
        let d = char_descriptor('1').expect("1 is printable");
        assert_eq!(d.code, "Digit1");
        assert_eq!(d.windows_vk, 49);
        assert!(!d.shift);
    }

    #[test]
    fn char_descriptor_shifted_digit_uses_base_digit() {
        // '!' is shift+1.
        let d = char_descriptor('!').expect("! is printable");
        assert_eq!(d.code, "Digit1");
        assert_eq!(d.windows_vk, 49);
        assert!(d.shift);
        // '@' is shift+2.
        let at = char_descriptor('@').expect("@ is printable");
        assert_eq!(at.code, "Digit2");
        assert_eq!(at.windows_vk, 50);
        assert!(at.shift);
    }

    #[test]
    fn char_descriptor_unshifted_punctuation() {
        let d = char_descriptor(';').expect("; is printable");
        assert_eq!(d.code, "Semicolon");
        assert_eq!(d.windows_vk, 186);
        assert!(!d.shift);
    }

    #[test]
    fn char_descriptor_shifted_punctuation_reuses_base() {
        // ':' is shift+';' → reuses Semicolon/186 with shift.
        let d = char_descriptor(':').expect(": is printable");
        assert_eq!(d.code, "Semicolon");
        assert_eq!(d.windows_vk, 186);
        assert!(d.shift);
    }

    #[test]
    fn char_descriptor_emoji_and_non_ascii_are_none() {
        assert!(char_descriptor('🚀').is_none());
        assert!(char_descriptor('é').is_none());
        assert!(char_descriptor('中').is_none());
    }

    #[test]
    fn char_descriptor_whitespace_is_none() {
        // Space / tab / newline are routed to SpecialKey by callers.
        assert!(char_descriptor(' ').is_none());
        assert!(char_descriptor('\t').is_none());
        assert!(char_descriptor('\n').is_none());
    }

    // --- key_events builder ---

    #[test]
    fn key_events_uppercase_emits_shift_wrap() {
        let events = key_events(Key::Char('A'), KeyModifiers::empty(), KeyPress::DownAndUp);
        // Shift-down, A-down, A-up, Shift-up.
        assert_eq!(events.len(), 4);

        assert_eq!(events[0].event_type, "keyDown");
        assert_eq!(events[0].code, Some("ShiftLeft"));
        assert_eq!(events[0].windows_vk, Some(16));
        assert_eq!(events[0].modifiers, KeyModifiers::SHIFT.cdp_bits());

        assert_eq!(events[1].event_type, "keyDown");
        assert_eq!(events[1].code, Some("KeyA"));
        assert_eq!(events[1].windows_vk, Some(65));
        assert_eq!(events[1].key.as_deref(), Some("A"));
        assert_eq!(events[1].text.as_deref(), Some("A"));
        assert_eq!(events[1].modifiers, KeyModifiers::SHIFT.cdp_bits());

        assert_eq!(events[2].event_type, "keyUp");
        assert_eq!(events[2].code, Some("KeyA"));

        assert_eq!(events[3].event_type, "keyUp");
        assert_eq!(events[3].code, Some("ShiftLeft"));
        assert_eq!(events[3].modifiers, 0);
    }

    #[test]
    fn key_events_ctrl_a_emits_control_wrap() {
        let events = key_events(Key::Char('a'), KeyModifiers::CTRL, KeyPress::DownAndUp);
        // Control-down, a-down, a-up, Control-up.
        assert_eq!(events.len(), 4);

        assert_eq!(events[0].event_type, "keyDown");
        assert_eq!(events[0].code, Some("ControlLeft"));
        assert_eq!(events[0].windows_vk, Some(17));
        assert_eq!(events[0].modifiers, KeyModifiers::CTRL.cdp_bits());

        assert_eq!(events[1].event_type, "keyDown");
        assert_eq!(events[1].code, Some("KeyA"));
        assert_eq!(events[1].windows_vk, Some(65));
        assert_eq!(events[1].modifiers, KeyModifiers::CTRL.cdp_bits());

        assert_eq!(events[2].event_type, "keyUp");
        assert_eq!(events[2].code, Some("KeyA"));

        assert_eq!(events[3].event_type, "keyUp");
        assert_eq!(events[3].code, Some("ControlLeft"));
        assert_eq!(events[3].modifiers, 0);
    }

    #[test]
    fn key_events_emoji_char_path_is_single_char_event() {
        let events = key_events(Key::Char('🚀'), KeyModifiers::empty(), KeyPress::DownAndUp);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "char");
        assert_eq!(events[0].text.as_deref(), Some("🚀"));
        assert_eq!(events[0].key.as_deref(), Some("🚀"));
        assert!(events[0].code.is_none());
        assert!(events[0].windows_vk.is_none());
    }

    #[test]
    fn key_events_char_kind_forces_char_event_even_for_ascii() {
        let events = key_events(Key::Char('a'), KeyModifiers::empty(), KeyPress::Char);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "char");
        assert_eq!(events[0].text.as_deref(), Some("a"));
        assert!(events[0].code.is_none());
    }

    #[test]
    fn key_events_special_key_no_modifiers_is_rawkeydown_keyup() {
        let events = key_events(
            Key::Special(SpecialKey::Enter),
            KeyModifiers::empty(),
            KeyPress::DownAndUp,
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "rawKeyDown");
        assert_eq!(events[0].key.as_deref(), Some("Enter"));
        assert_eq!(events[0].windows_vk, Some(13));
        assert!(events[0].text.is_none());
        assert_eq!(events[1].event_type, "keyUp");
    }

    #[test]
    fn key_events_shifted_char_with_extra_modifier_accumulates_bits() {
        // Ctrl + ':' → Ctrl-down, Shift-down (synthesized from descriptor),
        // ':'-down, ':'-up, Shift-up, Ctrl-up. Conventional order is
        // Alt,Ctrl,Meta,Shift, so Ctrl precedes Shift.
        let events = key_events(Key::Char(':'), KeyModifiers::CTRL, KeyPress::DownAndUp);
        assert_eq!(events.len(), 6);
        assert_eq!(events[0].code, Some("ControlLeft"));
        assert_eq!(events[0].modifiers, KeyModifiers::CTRL.cdp_bits());
        assert_eq!(events[1].code, Some("ShiftLeft"));
        assert_eq!(
            events[1].modifiers,
            (KeyModifiers::CTRL | KeyModifiers::SHIFT).cdp_bits()
        );
        assert_eq!(events[2].code, Some("Semicolon"));
        assert_eq!(events[2].event_type, "keyDown");
        assert_eq!(events[3].event_type, "keyUp");
        assert_eq!(events[3].code, Some("Semicolon"));
        assert_eq!(events[4].code, Some("ShiftLeft"));
        assert_eq!(events[4].event_type, "keyUp");
        assert_eq!(events[4].modifiers, KeyModifiers::CTRL.cdp_bits());
        assert_eq!(events[5].code, Some("ControlLeft"));
        assert_eq!(events[5].modifiers, 0);
    }

    // --- to_cdp_params serialization ---

    #[test]
    fn char_event_params_omit_code_and_vk() {
        let p = KeyEventPayload {
            event_type: "char",
            modifiers: 0,
            text: Some("🚀".to_string()),
            key: Some("🚀".to_string()),
            code: None,
            windows_vk: None,
        };
        let v = p.to_cdp_params();
        assert_eq!(v["type"], "char");
        assert_eq!(v["text"], "🚀");
        assert_eq!(v["key"], "🚀");
        assert!(v.get("code").is_none());
        assert!(v.get("windowsVirtualKeyCode").is_none());
    }

    #[test]
    fn keydown_params_include_code_and_both_vk_fields() {
        let events = key_events(Key::Char('A'), KeyModifiers::empty(), KeyPress::DownAndUp);
        // events[1] is the main A keyDown.
        let v = events[1].to_cdp_params();
        assert_eq!(v["type"], "keyDown");
        assert_eq!(v["code"], "KeyA");
        assert_eq!(v["windowsVirtualKeyCode"], 65);
        assert_eq!(v["nativeVirtualKeyCode"], 65);
        assert_eq!(v["text"], "A");
    }

    // --- cluster_events (the per-grapheme path type_text uses) ---

    fn flatten_text(text: &str) -> Vec<KeyEventPayload> {
        UnicodeSegmentation::graphemes(text, true)
            .flat_map(|c| cluster_events(c, KeyModifiers::empty()))
            .collect()
    }

    #[test]
    fn cluster_events_for_aa_bang_emits_expected_sequence() {
        // "Aa!" → [Shift,A,A,Shift] + [a,a] + [Shift,'!',? ,Shift]
        // i.e. Shift-down,A-down,A-up,Shift-up, a-down,a-up,
        //      Shift-down,1-down('!'),1-up,Shift-up.
        let events = flatten_text("Aa!");
        let kinds: Vec<(&str, Option<&str>)> =
            events.iter().map(|e| (e.event_type, e.code)).collect();
        assert_eq!(
            kinds,
            vec![
                ("keyDown", Some("ShiftLeft")),
                ("keyDown", Some("KeyA")),
                ("keyUp", Some("KeyA")),
                ("keyUp", Some("ShiftLeft")),
                ("keyDown", Some("KeyA")),
                ("keyUp", Some("KeyA")),
                ("keyDown", Some("ShiftLeft")),
                ("keyDown", Some("Digit1")),
                ("keyUp", Some("Digit1")),
                ("keyUp", Some("ShiftLeft")),
            ]
        );
        // The '!' main key carries text "!" (not "1").
        assert_eq!(events[7].text.as_deref(), Some("!"));
        assert_eq!(events[7].key.as_deref(), Some("!"));
    }

    #[test]
    fn cluster_events_emoji_is_single_char_event() {
        let events = flatten_text("🚀");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "char");
        assert_eq!(events[0].text.as_deref(), Some("🚀"));
    }

    #[test]
    fn cluster_events_multi_codepoint_grapheme_is_one_char_event() {
        // Family emoji = several codepoints joined with ZWJ → one cluster.
        let family = "👨‍👩‍👧";
        let events = flatten_text(family);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "char");
        assert_eq!(events[0].text.as_deref(), Some(family));
    }

    #[test]
    fn cluster_events_space_routes_to_special_key() {
        let events = flatten_text(" ");
        // Space → SpecialKey::Space DownAndUp (rawKeyDown + keyUp).
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "rawKeyDown");
        assert_eq!(events[0].code, Some("Space"));
        assert_eq!(events[0].windows_vk, Some(32));
        assert_eq!(events[1].event_type, "keyUp");
    }

    #[test]
    fn cluster_events_newline_routes_to_enter() {
        let events = flatten_text("\n");
        assert_eq!(events[0].code, Some("Enter"));
        assert_eq!(events[0].windows_vk, Some(13));
    }

    // --- KeySequence flattening ---

    #[test]
    fn key_sequence_flattens_steps_in_order() {
        let seq = KeySequence::new()
            .text("hi")
            .key(SpecialKey::Enter)
            .chord(Key::Char('a'), KeyModifiers::CTRL);
        let events = seq.to_events();

        // "hi": h-down,h-up,i-down,i-up (both lowercase, no shift).
        assert_eq!(events[0].event_type, "keyDown");
        assert_eq!(events[0].code, Some("KeyH"));
        assert_eq!(events[1].event_type, "keyUp");
        assert_eq!(events[2].code, Some("KeyI"));
        assert_eq!(events[3].event_type, "keyUp");
        // Enter: rawKeyDown + keyUp.
        assert_eq!(events[4].event_type, "rawKeyDown");
        assert_eq!(events[4].code, Some("Enter"));
        assert_eq!(events[5].event_type, "keyUp");
        assert_eq!(events[5].code, Some("Enter"));
        // Ctrl+a: Control-down, a-down, a-up, Control-up.
        assert_eq!(events[6].code, Some("ControlLeft"));
        assert_eq!(events[6].event_type, "keyDown");
        assert_eq!(events[7].code, Some("KeyA"));
        assert_eq!(events[7].modifiers, KeyModifiers::CTRL.cdp_bits());
        assert_eq!(events[8].event_type, "keyUp");
        assert_eq!(events[8].code, Some("KeyA"));
        assert_eq!(events[9].code, Some("ControlLeft"));
        assert_eq!(events[9].event_type, "keyUp");
        assert_eq!(events.len(), 10);
    }

    #[test]
    fn key_sequence_text_with_emoji_uses_char_event() {
        let seq = KeySequence::new().text("a🚀");
        let events = seq.to_events();
        // a → down/up (2), 🚀 → char (1).
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].code, Some("KeyA"));
        assert_eq!(events[2].event_type, "char");
        assert_eq!(events[2].text.as_deref(), Some("🚀"));
    }
}

use std::time::Duration;

use crate::error::Result;
use crate::input::InputController;
use crate::tab::Tab;

/// Dispatch a pre-built sequence of CDP key events in order.
pub(crate) async fn dispatch_key_events(tab: &Tab, events: &[KeyEventPayload]) -> Result<()> {
    for ev in events {
        tab.session()
            .call("Input.dispatchKeyEvent", ev.to_cdp_params())
            .await?;
    }
    Ok(())
}

/// Dispatch raw text as a single `char`-type event.
///
/// Used for multi-codepoint grapheme clusters (emoji with ZWJ/skin-tone
/// modifiers, combining sequences) that have no single physical-key origin.
// Consumed by the A2b public-API layer (`type_text` grapheme path).
#[allow(dead_code)]
pub(crate) async fn dispatch_char_text(tab: &Tab, text: &str) -> Result<()> {
    let payload = KeyEventPayload {
        event_type: "char",
        modifiers: 0,
        text: Some(text.to_string()),
        key: Some(text.to_string()),
        code: None,
        windows_vk: None,
    };
    dispatch_key_events(tab, std::slice::from_ref(&payload)).await
}

/// Dispatch a single character as a full keyDown/keyUp pair (with shift /
/// code / vk synthesis via [`key_events`]).
///
/// `modifier_bits` are the caller's already-held modifiers; emoji /
/// non-ASCII chars fall back to a `char`-type event automatically.
pub(crate) async fn dispatch_char(tab: &Tab, c: char, modifier_bits: i32) -> Result<()> {
    let mods = KeyModifiers::from_bits_truncate(modifier_bits as u8);
    let events = key_events(Key::Char(c), mods, KeyPress::DownAndUp);
    dispatch_key_events(tab, &events).await
}

/// Dispatch a named special key (Enter, Tab, etc) as a rawKeyDown/keyUp pair.
#[allow(dead_code)]
pub(crate) async fn dispatch_special(tab: &Tab, k: SpecialKey, modifier_bits: i32) -> Result<()> {
    let mods = KeyModifiers::from_bits_truncate(modifier_bits as u8);
    let events = key_events(Key::Special(k), mods, KeyPress::DownAndUp);
    dispatch_key_events(tab, &events).await
}

/// Type `text` with realistic per-character timing, occasional typos, and
/// inter-word "thinking" pauses pulled from the InputProfile.
///
/// Text is segmented into grapheme clusters; each cluster dispatches via
/// [`cluster_events`] (so uppercase / symbols synthesize Shift, emoji / CJK
/// route to a `char` event, and space / tab / newline become the matching
/// [`SpecialKey`]). The per-cluster delay / typo / thinking-pause wrapper is
/// preserved; typos still apply only to ASCII letters (via [`neighbor_key`]).
#[allow(dead_code)]
pub(crate) async fn type_text_realistic(
    input: &InputController,
    tab: &Tab,
    text: &str,
) -> Result<()> {
    let profile = input.profile.clone();
    for cluster in UnicodeSegmentation::graphemes(text, true) {
        // Leading char drives the typo / thinking-pause heuristics; the actual
        // keystroke uses the whole cluster (handles multi-codepoint emoji).
        let lead = cluster.chars().next().unwrap_or('\0');
        let (per_char_delay_ms, held_mods, do_typo, typo_char, thinking_pause_ms) = {
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
                profile.typo_rate > 0.0 && rand::Rng::r#gen::<f32>(&mut s.rng) < profile.typo_rate;
            let typo_char = if do_typo {
                neighbor_key(lead, &mut s.rng)
            } else {
                None
            };
            let thinking = if lead == ' '
                && profile.thinking_pause_ms_range.0 > 0
                && rand::Rng::r#gen::<f32>(&mut s.rng) < 0.05
            {
                rand::Rng::gen_range(
                    &mut s.rng,
                    profile.thinking_pause_ms_range.0..=profile.thinking_pause_ms_range.1,
                )
            } else {
                0
            };
            (per_char, s.modifiers_held, do_typo, typo_char, thinking)
        };
        if per_char_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(per_char_delay_ms as u64)).await;
        }
        if thinking_pause_ms > 0 {
            tokio::time::sleep(Duration::from_millis(thinking_pause_ms as u64)).await;
        }
        if do_typo {
            if let Some(wrong) = typo_char {
                dispatch_char(tab, wrong, held_mods.cdp_bits()).await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
                dispatch_special(tab, SpecialKey::Backspace, held_mods.cdp_bits()).await?;
            }
        }
        dispatch_key_events(tab, &cluster_events(cluster, held_mods)).await?;
    }
    Ok(())
}

/// Type `text` as fast as possible — no delays, no typos.
///
/// Segments into grapheme clusters and dispatches each via
/// [`cluster_events`], so the full unicode / shift / special-key handling of
/// the realistic path applies without any timing jitter.
#[allow(dead_code)]
pub(crate) async fn type_text_fast(input: &InputController, tab: &Tab, text: &str) -> Result<()> {
    let held_mods = input.state.lock().await.modifiers_held;
    for cluster in UnicodeSegmentation::graphemes(text, true) {
        dispatch_key_events(tab, &cluster_events(cluster, held_mods)).await?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod dispatch_tests {
    use super::*;
    use serde_json::Value;
    use zendriver_stealth::InputProfile;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn type_text_fast_emits_keydown_keyup_per_char() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let input = InputController::new_with_seed(InputProfile::native(), 42);

        let fut = tokio::spawn({
            let input = input.clone();
            let tab = tab.clone();
            async move { type_text_fast(&input, &tab, "ab").await }
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
        let tab = Tab::new_for_test(sess);

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
