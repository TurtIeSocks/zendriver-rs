# Input

Every input method on [`Element`] comes in two variants:

- **Realistic (default)** — Bezier-interpolated cursor moves for the
  mouse, per-character delays with occasional typos for the keyboard.
  Tuned to defeat behavioral fingerprinters.
- **`_fast`** — single CDP dispatch, no delays, no jitter, no typos.
  Skips the actionability gate. For tests and fast automation flows
  where deterministic timing matters more than realism.

Both flavors route through the same shared [`InputController`] on each
tab, so the OS-level modifier state (Shift, Ctrl, etc.) stays consistent
across realistic and fast paths.

[`Element`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html
[`InputController`]: https://docs.rs/zendriver/latest/zendriver/input/struct.InputController.html

## Realistic vs `_fast`

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let btn = tab.find().css("button").one().await?;

// Realistic: Bezier-path cursor approach, hover, then mousedown/up.
btn.click().await?;

// Fast: single Input.dispatchMouseEvent, no actionability gate.
btn.click_fast().await?;
# Ok(()) }
```

| Method               | Cursor path        | Gate            | Use case                        |
|----------------------|--------------------|-----------------|---------------------------------|
| `click()`            | Bezier             | actionability   | Default. Indistinguishable.    |
| `click_fast()`       | teleport           | skipped         | Tests; trusted automation.     |
| `hover()`            | Bezier             | actionability   | Default. Real cursor approach. |
| `hover_fast()`       | teleport           | skipped         | Tests; trusted automation.     |
| `type_text(s)`       | per-char + delays  | focus gate      | Default. Sub-keystroke timing. |
| `type_text_fast(s)`  | per-char, no delay | focus gate      | Tests; trusted automation.     |

By default, the realism comes from the active [`StealthProfile`]'s
`InputProfile`:

- `StealthProfile::native()` and `::spoofed()` install a realistic
  profile by default — Bezier control points with deterministic-but-
  jittered timing, per-character keyboard delays of 30-200 ms,
  occasional 1-2% typo + correction events.
- `StealthProfile::off()` installs a no-op profile — even realistic
  methods just do the dispatch without realism.

When realism matters but you also want determinism (e.g. snapshots
inside tests), seed the profile with a fixed RNG — see the
[`InputProfile`] rustdoc.

[`StealthProfile`]: https://docs.rs/zendriver/latest/zendriver/stealth/struct.StealthProfile.html
[`InputProfile`]: https://docs.rs/zendriver_stealth/latest/zendriver_stealth/struct.InputProfile.html

### Opt-in: decoupling input timing from stealth

[`BrowserBuilder::input_profile()`] lets you pick the [`InputProfile`]
explicitly, **independent** of `StealthProfile`. This is opt-in only —
it does not change any default. With no `.input_profile(..)` call, timing
still resolves to `InputProfile::native()` (today's zero-overhead
default), whether stealth is on, off, or spoofed.

Use it when you want humanized timing without also turning on stealth's
surface patches (canvas/WebGL/navigator overrides), or when you want
stealth on but deterministic zero-delay input for a test:

```rust,no_run
use zendriver::stealth::{InputProfile, StealthProfile};

# async fn ex() -> zendriver::Result<()> {
// Stealth off (stock Chrome launch), but keep human-paced typing and
// jittery mouse motion — previously impossible, since input timing was
// derived from the stealth profile.
let browser = zendriver::Browser::builder()
    .stealth(StealthProfile::off())
    .input_profile(InputProfile::coherent())
    .launch()
    .await?;
# browser.close().await?;
# Ok(()) }
```

`BrowserBuilder::resolved_input_profile()` returns the effective profile
before launch, for tests/inspection — same pattern as
`resolved_persona()`.

[`BrowserBuilder::input_profile()`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.input_profile

## `ClickOptions` for fine control

Both `click()` and `click_fast()` are wrappers around [`Element::click_with()`],
which takes a [`ClickOptions`] struct for full control:

```rust,no_run
use zendriver::{ClickOptions, MouseButton, KeyModifiers};

# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let row = tab.find().css("tr.contact").one().await?;

// Right-click.
row.click_with(ClickOptions {
    button: MouseButton::Right,
    ..Default::default()
}).await?;

// Ctrl+click (open in new tab).
let link = tab.find().css("a.external").one().await?;
link.click_with(ClickOptions {
    modifiers: KeyModifiers::CTRL,
    ..Default::default()
}).await?;

// Double-click.
let item = tab.find().css(".item").one().await?;
item.click_with(ClickOptions {
    click_count: 2,
    ..Default::default()
}).await?;

// Click at a specific offset inside the element's bbox.
let canvas = tab.find().css("canvas").one().await?;
canvas.click_with(ClickOptions {
    position: Some((100.0, 50.0)),
    ..Default::default()
}).await?;
# Ok(()) }
```

The full `ClickOptions` shape:

| Field         | Type            | Default                  | Meaning                                                |
|---------------|-----------------|--------------------------|--------------------------------------------------------|
| `button`      | `MouseButton`   | `MouseButton::Left`      | Which button to dispatch.                              |
| `modifiers`   | `KeyModifiers`  | `KeyModifiers::empty()`  | Modifier bits held during dispatch.                    |
| `click_count` | `u32`           | `1`                      | `clickCount` for the dispatch (2 = double-click).      |
| `force`       | `bool`          | `false`                  | Skip the actionability gate. Mirrors Playwright.       |
| `realistic`   | `bool`          | `true`                   | Bezier path vs teleport.                               |
| `position`    | `Option<(f64, f64)>` | `None` (bbox center) | Click offset relative to bbox top-left.                |

[`Element::click_with()`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html#method.click_with
[`ClickOptions`]: https://docs.rs/zendriver/latest/zendriver/struct.ClickOptions.html

## Keyboard: `Key`, `KeyModifiers`, `SpecialKey`

For single-key dispatches (Enter, Tab, arrow keys, Ctrl+A, etc.):

```rust,no_run
use zendriver::{Key, KeyModifiers, SpecialKey};

# async fn ex() -> zendriver::Result<()> {
# let browser = zendriver::Browser::builder().launch().await?;
# let tab = browser.main_tab();
let input = tab.find().css("input").one().await?;

// Press Enter (named special key).
input.press(Key::Special(SpecialKey::Enter)).await?;

// Press Tab to move focus.
input.press(Key::Special(SpecialKey::Tab)).await?;

// Ctrl+A (select all).
input.press_with(Key::Char('a'), KeyModifiers::CTRL).await?;

// Ctrl+Shift+End.
input.press_with(
    Key::Special(SpecialKey::End),
    KeyModifiers::CTRL | KeyModifiers::SHIFT,
).await?;
# Ok(()) }
```

[`Key`] is one of:

- `Key::Char(char)` — any typeable character.
- `Key::Special(SpecialKey)` — named non-character key.

[`SpecialKey`] covers Enter, Tab, Escape, Backspace, Delete, Space, all
four arrows, Home, End, PageUp, PageDown, F1-F12, Insert, CapsLock,
NumLock, ScrollLock, PrintScreen, Pause, ContextMenu — the full
non-character keyboard.

[`KeyModifiers`] is a bitflags struct:

- `KeyModifiers::ALT` — Alt (Option on macOS).
- `KeyModifiers::CTRL` — Control.
- `KeyModifiers::META` — Meta (Command on macOS, Windows key on
  Windows).
- `KeyModifiers::SHIFT` — Shift.

Combine with `|`: `KeyModifiers::CTRL | KeyModifiers::SHIFT`.

[`Key`]: https://docs.rs/zendriver/latest/zendriver/enum.Key.html
[`SpecialKey`]: https://docs.rs/zendriver/latest/zendriver/enum.SpecialKey.html
[`KeyModifiers`]: https://docs.rs/zendriver/latest/zendriver/struct.KeyModifiers.html

### `press` vs `press_with`

- [`Element::press(key)`] uses whatever modifiers are currently held by
  the [`InputController`] — useful when you've explicitly tracked
  modifier-held state (e.g. via held-key sequences).
- [`Element::press_with(key, mods)`] passes `mods` straight through to
  the CDP dispatch for this one call, without mutating the controller's
  tracked state — the safer default when you want a single key event
  with specific modifiers.

[`Element::press(key)`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html#method.press
[`Element::press_with(key, mods)`]: https://docs.rs/zendriver/latest/zendriver/struct.Element.html#method.press_with

## End-to-end form fill

```rust,no_run
{{#include ../../../crates/zendriver/examples/form_demo.rs}}
```

Expected output:

```text
user field = "rin", submitted = true
```

The example demonstrates the realistic input surface end-to-end:
per-character typing into two inputs, then a single click on the submit
button. All three calls (`type_text` + `type_text` + `click`) go through
the actionability gate and the realistic-cursor path.

## When to use `_fast` variants

- **Tests** where you only care that the action happened, not how it
  looked to a fingerprinter.
- **Trusted automation pipelines** (internal admin tools, scraping
  flows where you already know stealth isn't being checked).
- **CI** where every saved millisecond per click compounds across
  thousands of runs.
- **Setup steps** (typing a known query into a search box before the
  real interaction starts). Save realism for the moments that matter.

When in doubt, stick with the realistic defaults — the per-call cost is
small (typically 50-300 ms per click; a few ms per typed character) and
it keeps you on the "indistinguishable from a real user" path that the
rest of the stealth machinery is built around.
