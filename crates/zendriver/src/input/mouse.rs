//! Realistic + raw mouse dispatch.

use std::time::Duration;

use serde_json::json;

use crate::error::Result;
use crate::input::InputController;
use crate::input::bezier::BezierPath;
use crate::input::keyboard::KeyModifiers;
use crate::input::pointer_state::MouseButtonSet;
use crate::tab::Tab;

/// Mouse buttons for click dispatch.
///
/// Mirrors the CDP `MouseEvent.button` enum.
///
/// # Examples
///
/// ```
/// use zendriver::MouseButton;
/// assert_eq!(MouseButton::Left.cdp_str(), "left");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary button (left for right-handed users).
    Left,
    /// Middle button (scroll wheel click).
    Middle,
    /// Secondary button (right for right-handed users).
    Right,
    /// "Back" thumb button.
    Back,
    /// "Forward" thumb button.
    Forward,
}

impl MouseButton {
    /// CDP wire string for this button.
    ///
    /// # Examples
    ///
    /// ```
    /// use zendriver::MouseButton;
    /// assert_eq!(MouseButton::Right.cdp_str(), "right");
    /// ```
    #[must_use]
    pub fn cdp_str(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
            MouseButton::Back => "back",
            MouseButton::Forward => "forward",
        }
    }
}

/// The [`MouseButtonSet`] bit for a single button.
///
/// `MouseButtonSet`'s bit values are chosen to match CDP's `buttons` bitmask
/// (1 left, 2 right, 4 middle, 8 back, 16 forward), so `MouseButtonSet::bits`
/// is the wire value directly.
const fn button_bit(button: MouseButton) -> MouseButtonSet {
    match button {
        MouseButton::Left => MouseButtonSet::LEFT,
        MouseButton::Middle => MouseButtonSet::MIDDLE,
        MouseButton::Right => MouseButtonSet::RIGHT,
        MouseButton::Back => MouseButtonSet::BACK,
        MouseButton::Forward => MouseButtonSet::FORWARD,
    }
}

/// Move the cursor from its current position to `(target_x, target_y)` along
/// a Bezier path with realistic per-segment delay. Updates InputController
/// state to the target position on success.
///
/// Carries `buttons` so a move dispatched between a press and a release reads
/// as a drag. A page implementing drag with `mousemove` + `e.buttons` — the
/// standard modern check — sees nothing at all without it.
pub(crate) async fn move_realistic(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
    extra_modifiers: KeyModifiers,
) -> Result<()> {
    // Hold the lock across the full dispatch + state-update sequence so a
    // concurrent `move_*` from another task can't slip in between our last
    // `mouseMoved` and the `pointer_{x,y}` write and leave the cached
    // cursor position out of sync with the page's actual cursor.
    // InputController is per-Tab so this only serializes input on this Tab,
    // which matches Chrome's per-page input model.
    let mut state = input.state.lock().await;
    let start = (state.pointer_x, state.pointer_y);
    let modifier_bits = (state.modifiers_held | extra_modifiers).cdp_bits();
    let buttons = state.buttons_held.bits();
    let path = BezierPath::build(
        start,
        (target_x, target_y),
        input.profile.jitter_amplitude_px,
        &mut state.rng,
    );
    let speed = input.profile.mouse_speed_px_per_ms;
    // `points[0]` is the start position, so dispatching it emits a `mouseMoved`
    // to where the cursor already is. Skip it, and sleep *before* each step so
    // the delay pays for the movement that follows rather than trailing the
    // last one.
    let mut prev = path.points.first().copied().unwrap_or(start);
    for &(x, y) in path.points.iter().skip(1) {
        if speed > 0.0 {
            // Derive the delay from the segment actually emitted. `BezierPath`
            // clamps its sample count to [8, 60], so segment length scales with
            // distance; assuming a fixed 5px step ran long moves ~3x too fast
            // and short ones ~4x too slow, which made `mouse_speed_px_per_ms`
            // honest only for 40-300px journeys.
            let seg = (x - prev.0).hypot(y - prev.1);
            let delay = Duration::from_micros(((seg / speed) * 1000.0) as u64);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        tab.session()
            .call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseMoved", "x": x, "y": y,
                    "modifiers": modifier_bits,
                    "buttons": buttons,
                }),
            )
            .await?;
        prev = (x, y);
    }
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

/// Direct move without interpolation. Single dispatchMouseEvent.
pub(crate) async fn move_raw(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
    extra_modifiers: KeyModifiers,
) -> Result<()> {
    // Same per-Tab serialization rationale as `move_realistic`.
    let mut state = input.state.lock().await;
    let modifier_bits = (state.modifiers_held | extra_modifiers).cdp_bits();
    let buttons = state.buttons_held.bits();
    tab.session()
        .call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved", "x": target_x, "y": target_y,
                "modifiers": modifier_bits,
                "buttons": buttons,
            }),
        )
        .await?;
    state.pointer_x = target_x;
    state.pointer_y = target_y;
    Ok(())
}

/// Dispatch a click at `(target_x, target_y)`.
///
/// Reads `button` / `click_count` / `realistic` / `modifiers` from `opts`;
/// `force` and `position` belong to the element-gating layer and are resolved
/// by the caller before it picks a target coordinate.
///
/// `opts.modifiers` are OR'd with whatever the keyboard currently holds, so a
/// caller-requested Ctrl/Cmd/Shift-click works without the caller having to
/// drive a real key-down first. They ride the approach `mouseMoved` frames too,
/// the way a real browser reports `ctrlKey` on mousemove.
///
/// A `click_count` above 1 emits that many press/release pairs with an
/// increasing `clickCount`, which is what Chrome produces for a real
/// double-click. Collapsing it into one pair carrying `clickCount: 2` is
/// invisible to any page counting `mousedown` events.
pub(crate) async fn click_at(
    input: &InputController,
    tab: &Tab,
    target_x: f64,
    target_y: f64,
    opts: &crate::element::actions::ClickOptions,
) -> Result<()> {
    let (button, click_count, extra_modifiers) = (opts.button, opts.click_count, opts.modifiers);
    if opts.realistic {
        move_realistic(input, tab, target_x, target_y, extra_modifiers).await?;
    } else {
        move_raw(input, tab, target_x, target_y, extra_modifiers).await?;
    }
    let bit = button_bit(button);
    // One fallible section plus one cleanup below, rather than a `Drop` guard.
    // The full rationale — and the cancellation case it does not cover — is on
    // `InputState::buttons_held`; `Tab::mouse_drag` uses the same shape.
    //
    // `we_latched` records whether *this* call is the one that set the bit,
    // so the cleanup below only clears a bit it owns. Without it, a failure
    // anywhere before the first press — the approach `mouseMoved`, say —
    // still runs the cleanup and clears a bit some other task latched.
    //
    // It does not make `click_at` safe to run concurrently with another
    // gesture on the same tab, and does not try to: the in-loop release
    // clears the bit unconditionally, because the frame it emits has to
    // report the button as up. Two tasks driving one tab's mouse are
    // modelling one physical button from two places and will disagree
    // whatever this does. `InputController` is per-`Tab`; keep a gesture on
    // one task.
    //
    // What the guard gives up: the unconditional sweep it replaced also
    // cleaned up a bit stranded by a dropped gesture future (the cancellation
    // case on `InputState::buttons_held`). A click that finds the bit already
    // set now records `we_latched = false`, so if its own press fails the
    // stranded bit survives this call too. Not permanent — the in-loop
    // release clears the bit unconditionally, so the next click that reaches
    // a press self-heals it — but until then this tab's `mouseMoved` frames
    // report `buttons: 1` with no preceding `mousedown`, the impossible
    // stream that field doc calls worse than omitting `buttons`.
    let mut we_latched = false;
    let sequence: Result<()> = async {
        for n in 1..=click_count.max(1) {
            // `buttons` must reflect the set held *after* the press and *after*
            // the release, so take it from the same locked section that mutates
            // it.
            let (modifier_bits, buttons) = {
                let mut s = input.state.lock().await;
                we_latched |= !s.buttons_held.contains(bit);
                s.buttons_held.insert(bit);
                (
                    (s.modifiers_held | extra_modifiers).cdp_bits(),
                    s.buttons_held.bits(),
                )
            };
            tab.session()
                .call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": "mousePressed",
                        "x": target_x, "y": target_y,
                        "button": button.cdp_str(),
                        "clickCount": n,
                        "modifiers": modifier_bits,
                        "buttons": buttons,
                    }),
                )
                .await?;
            let (modifier_bits, buttons) = {
                let mut s = input.state.lock().await;
                s.buttons_held.remove(bit);
                (
                    (s.modifiers_held | extra_modifiers).cdp_bits(),
                    s.buttons_held.bits(),
                )
            };
            tab.session()
                .call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": "mouseReleased",
                        "x": target_x, "y": target_y,
                        "button": button.cdp_str(),
                        "clickCount": n,
                        "modifiers": modifier_bits,
                        "buttons": buttons,
                    }),
                )
                .await?;
        }
        Ok(())
    }
    .await;
    // Single exit for the latched bit, skipped when this call never latched
    // it. A no-op on the happy path too — the last release already cleared it.
    if we_latched {
        input.state.lock().await.buttons_held.remove(bit);
    }
    sequence
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::expect;
    use rand::SeedableRng as _;
    use zendriver_transport::{SessionHandle, testing::MockConnection};

    #[test]
    fn mouse_button_cdp_strings_match_chrome() {
        assert_eq!(MouseButton::Left.cdp_str(), "left");
        assert_eq!(MouseButton::Right.cdp_str(), "right");
        assert_eq!(MouseButton::Middle.cdp_str(), "middle");
        assert_eq!(MouseButton::Back.cdp_str(), "back");
        assert_eq!(MouseButton::Forward.cdp_str(), "forward");
    }
    // Note: dispatch fns are async + need a Tab + MockConnection — exercised in T20 click tests.

    /// Drive one `Input.dispatchMouseEvent` off the mock, assert the frame is
    /// the expected `type`, and return its id so the caller can reply.
    async fn next_dispatch(mock: &mut MockConnection, expected_type: &str) -> u64 {
        let id = expect(mock, "Input.dispatchMouseEvent").await;
        assert_eq!(
            mock.last_sent()["params"]["type"],
            expected_type,
            "unexpected dispatch order: {}",
            mock.last_sent()
        );
        id
    }

    /// An [`InputProfile`] with no jitter (so `BezierPath` is fully
    /// deterministic and its arc length is exactly the straight-line
    /// distance) and a cursor speed of `px_per_ms`.
    fn steady_profile(px_per_ms: f64) -> zendriver_stealth::InputProfile {
        zendriver_stealth::InputProfile {
            mouse_speed_px_per_ms: px_per_ms,
            jitter_amplitude_px: 0.0,
            ..zendriver_stealth::InputProfile::native()
        }
    }

    /// The `(x, y)` of every frame a move emits, asserting each really is a
    /// `mouseMoved`.
    async fn drain_mouse_moves(mock: &mut MockConnection) -> Vec<(f64, f64)> {
        crate::test_support::drain_mouse_dispatches(mock)
            .await
            .iter()
            .map(|p| {
                assert_eq!(p["type"], "mouseMoved", "{p}");
                (
                    p["x"].as_f64().expect("x is a number"),
                    p["y"].as_f64().expect("y is a number"),
                )
            })
            .collect()
    }

    /// A click that found the bit already set never latched it, so its
    /// cleanup must leave it alone.
    ///
    /// The unconditional `remove` this replaced made a failed `click_fast`
    /// clear a LEFT bit an in-flight `mouse_drag` was holding, after which the
    /// drag's remaining `mouseMoved` frames report `buttons: 0` while the page
    /// still believes the button is down — the same incoherent stream the
    /// latch exists to prevent, arrived at from the other side.
    ///
    /// The *press* is what has to fail here, not the approach: a failed
    /// approach returns from `click_at` before the cleanup is reachable at
    /// all, so it cannot tell the two versions apart.
    #[tokio::test]
    async fn click_at_leaves_a_button_it_never_latched_alone() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));

        // Stand in for a concurrent gesture already holding the button.
        tab.input()
            .state
            .lock()
            .await
            .buttons_held
            .insert(MouseButtonSet::LEFT);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                let opts = crate::element::actions::ClickOptions {
                    realistic: false,
                    ..Default::default()
                };
                click_at(t.input(), &t, 10.0, 20.0, &opts).await
            }
        });

        let mv = next_dispatch(&mut mock, "mouseMoved").await;
        mock.reply(mv, serde_json::json!({})).await;
        let press = next_dispatch(&mut mock, "mousePressed").await;
        mock.reply_err(press, -32000, "dispatch rejected").await;

        assert!(
            fut.await.unwrap().is_err(),
            "the failed press must propagate"
        );
        assert!(
            tab.input()
                .state
                .lock()
                .await
                .buttons_held
                .contains(MouseButtonSet::LEFT),
            "click_at cleared a held button it never latched"
        );
        conn.shutdown();
    }

    /// The path's first point is where the cursor already is, so dispatching
    /// it emits a `mouseMoved` to the current position — something a real
    /// cursor never does, and a behavioral tell in a crate whose whole
    /// purpose is not looking synthetic.
    ///
    /// Pins the count against the path rather than a literal, so it stays
    /// honest if `BezierPath`'s sampling changes: n points in, n-1 frames out,
    /// and the one dropped is the start.
    #[tokio::test(start_paused = true)]
    async fn move_realistic_skips_the_path_point_the_cursor_is_already_on() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));
        let input = InputController::new_with_seed(steady_profile(10.0), 42);

        // 600px puts the sample count on `BezierPath`'s upper clamp (60), so
        // the emitted frames are a fixed 60 rather than distance/5.
        let target = (600.0, 0.0);
        let fut = tokio::spawn({
            let (i, t) = (input.clone(), tab.clone());
            async move { move_realistic(&i, &t, target.0, target.1, KeyModifiers::empty()).await }
        });

        let sent = drain_mouse_moves(&mut mock).await;
        fut.await.unwrap().unwrap();

        // Rebuild the same path: zero jitter never draws from the RNG, so
        // this is the identical point list the move walked.
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), target, 0.0, &mut rng);

        assert_eq!(
            sent.len(),
            path.points.len() - 1,
            "one frame per segment, not one per point"
        );
        assert_ne!(
            sent[0],
            (0.0, 0.0),
            "the opening frame must not re-dispatch the cursor's current position"
        );
        assert_eq!(
            sent[0], path.points[1],
            "the first frame is the second point"
        );
        assert_eq!(
            *sent.last().unwrap(),
            target,
            "the last frame is the target"
        );
        conn.shutdown();
    }

    /// Total travel time must come from the path's real segment lengths.
    ///
    /// The predecessor assumed a fixed 5px step, which is only true while
    /// `BezierPath`'s sample count is unclamped — it clamps to [8, 60], so a
    /// 600px move is 60 segments of 10px and the fixed assumption ran it 2x
    /// too fast. 600px at 1px/ms must therefore take ~600ms, not ~300ms,
    /// which is what makes `mouse_speed_px_per_ms` mean what it says.
    #[tokio::test(start_paused = true)]
    async fn move_realistic_paces_the_path_by_its_real_segment_lengths() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));
        let input = InputController::new_with_seed(steady_profile(1.0), 42);

        // Timed inside the task: the drain below ends on `try_expect`'s
        // silence timeout, and under `start_paused` that advances the clock
        // too, so measuring around the drain would fold the terminator into
        // the answer.
        let fut = tokio::spawn({
            let (i, t) = (input.clone(), tab.clone());
            async move {
                let started = tokio::time::Instant::now();
                move_realistic(&i, &t, 600.0, 0.0, KeyModifiers::empty()).await?;
                Ok::<_, crate::error::ZendriverError>(started.elapsed())
            }
        });
        drain_mouse_moves(&mut mock).await;
        let elapsed = fut.await.unwrap().unwrap();

        // Two sources of slack, both bounded and both upward-only against a
        // 600ms floor. `from_micros(... as u64)` truncates below a
        // microsecond, losing at most 60µs across the 60 segments; and
        // tokio's paused clock advances on 1ms ticks, so each sleep rounds up
        // to the next millisecond — at most 60ms in total. The window is
        // still nowhere near the ~300ms the fixed-5px predecessor produced.
        assert!(
            elapsed >= Duration::from_micros(599_940) && elapsed <= Duration::from_millis(660),
            "600px at 1px/ms should take ~600ms, took {elapsed:?}"
        );
        conn.shutdown();
    }

    /// A failed `mousePressed` must not strand the held-button bit. The
    /// InputController is per-Tab and long-lived, so a latched bit makes every
    /// subsequent `mouseMoved` on this tab report a button held with no
    /// preceding mousedown — incoherent pointer telemetry, and strictly worse
    /// than omitting the field.
    #[tokio::test]
    async fn click_at_clears_the_held_bit_when_the_press_fails() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                // `realistic: false` keeps the approach to a single teleport
                // `mouseMoved` — no Bezier path, no per-segment sleeps.
                let opts = crate::element::actions::ClickOptions {
                    realistic: false,
                    ..Default::default()
                };
                click_at(t.input(), &t, 10.0, 20.0, &opts).await
            }
        });

        let mv = next_dispatch(&mut mock, "mouseMoved").await;
        mock.reply(mv, serde_json::json!({})).await;
        let press = next_dispatch(&mut mock, "mousePressed").await;
        mock.reply_err(press, -32000, "dispatch rejected").await;

        assert!(
            fut.await.unwrap().is_err(),
            "a failed mousePressed must propagate"
        );
        assert!(
            tab.input().state.lock().await.buttons_held.is_empty(),
            "click_at leaked a latched button bit after a failed mousePressed"
        );
        conn.shutdown();
    }

    /// `mouse_drag` latches the bit as it builds the press frame, so the
    /// failure that leaks it is any dispatch up to the release — an
    /// interpolated `mouseMoved` is the realistic one.
    ///
    /// The mid-gesture assertion is what gives the closing one its meaning.
    /// `buttons_held` is empty before a drag starts, so `is_empty()` on its
    /// own passes just as well against a `mouse_drag` that never wrote the
    /// field at all — which is how it read on `origin/main`. Observing the
    /// bit set, then cleared, is a before/after pair; only the second half is
    /// a claim about cleanup.
    #[tokio::test]
    async fn mouse_drag_clears_the_held_bit_when_a_move_fails() {
        let (mut mock, conn) = MockConnection::pair();
        let tab = Tab::new_for_test(SessionHandle::new(conn.clone(), "S1"));

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.mouse_drag((10.0, 10.0), (200.0, 10.0), 5).await }
        });

        let press = next_dispatch(&mut mock, "mousePressed").await;
        mock.reply(press, serde_json::json!({})).await;
        // The bit is held from here until the release; fail inside that window.
        // The first `mouseMoved` is dispatched after the latch, so its arrival
        // means the write has already happened.
        let mv = next_dispatch(&mut mock, "mouseMoved").await;
        assert!(
            tab.input()
                .state
                .lock()
                .await
                .buttons_held
                .contains(MouseButtonSet::LEFT),
            "mouse_drag must hold LEFT between its press and its release"
        );
        mock.reply_err(mv, -32000, "dispatch rejected").await;

        assert!(
            fut.await.unwrap().is_err(),
            "a failed mouseMoved must propagate"
        );
        assert!(
            tab.input().state.lock().await.buttons_held.is_empty(),
            "mouse_drag leaked a latched button bit after a failed mouseMoved"
        );
        conn.shutdown();
    }
}
