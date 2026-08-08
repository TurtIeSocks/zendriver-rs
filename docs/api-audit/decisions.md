# Human decisions — API audit

The founder's-review rounds routed 14 judgement calls to a human rather than deciding them.
Rin answered all 14 on 2026-08-08. This file is the record PRs 2–7 build against; where an
answer changed the shape of the fix, that is noted.

Sources: [round 1 §5](founders-review-round1.md), [round 2 §7](founders-review-round2.md).

---

## 1. `visible_only` means on screen right now — DECIDED

`.visible_only()` filters to elements currently intersecting the viewport, not merely
rendered. The existing `check_visible` viewport test is therefore correct for the filter and
stays.

The split follows from this rather than being a separate question: `Element::is_visible`
should mean **rendered**, dropping the viewport test. A caller asking "is this visible"
about an element it is about to scroll to must not get `false`. Two predicates, two names,
one of which already exists.

Unblocks the `check_visible` split, its rustdoc, and the three book pages that were waiting.

## 2. `Frame::name()` / `Frame::id()` may break — APPROVED

Make `FrameInner::name` and `frame_id` interior-mutable and let `Frame::name()` become an
`async fn`. Pre-release API churn is acceptable; Rin is effectively the only consumer. The
`#[ignore]`d two-navigation test shipped in #156 is the acceptance criterion — un-ignore it.

Drops the URL write-through stopgap, whose comment already had to be corrected once for
claiming more than it did.

## 3. `REDIAL_TIMEOUT` becomes a setting, not a visibility question — RESHAPED

Neither option offered. Rin: *"part of the whole point of this API audit is to reduce
opinionated constants, so can we just keep it as default and make it settable?"*

So: keep the current value as the default, expose it as a caller-settable option, and leave
`zendriver-transport` itself out of the public dependency surface. The handshake-parity
assertion then tests a real API rather than needing a `pub` const to point at, and the
tautological test can go.

Moves from "PR 5a, decide visibility" to "PR 5a, add the option".

## 4. `CallError::Timeout` gains a field — DECIDED (question answered)

Rin asked whether a caller can work out for themselves which half timed out. **They cannot**
— it is genuinely hidden. `call_raw_with_timeout` wraps enqueue *and* reply in one
`tokio::time::timeout`, and the elapsed arm constructs `CallError::Timeout { method, budget }`
from nothing but those two values. Both halves produce a byte-identical error.

That distinction is retry-safety information: a command that never reached the actor is safe
to replay, one Chrome may have executed is not. So neither "one variant with honest docs" nor
a two-variant split is right — add the fact to the existing variant:

```rust
CallError::Timeout { method, budget, enqueued: bool }
```

An `AtomicBool` set after the send succeeds, read in the elapsed arm. No caller is forced to
match on a second variant, and the information stops being unrecoverable.

## 5. `visid_incap_*` lifetime — DEFERRED, needs live observation

Whether Imperva's persistent visitor-ID cookie is a challenge marker or ambient state cannot
be settled from anything in this repo. Rin: *"we'd have to poke some sites."* Until then the
detector keeps the current conservative reading.

## 6. Cloudflare false-success vs false-timeout — DEFERRED, needs live observation

Whether real Turnstile ever presents a mounted-but-unclickable widget post-click. Same
answer as #5. The JS-level fix already shipped is defensive either way, so nothing is
blocked; this decides only whether to keep it.

## 7. OOPIF eviction — DEFERRED, but this one is local

Grouped with #5/#6 by Rin, and worth separating: whether Chrome lists remote frames in the
parent target's `Page.getFrameTree` is answerable against a **local fixture page with a
cross-origin iframe**, no live site needed. Cheap to settle whenever someone wants it.

The follow-on design choice — grace period on url-less candidates versus an OOPIF-aware
filter — is a real latency/safety tradeoff and still needs a human once the fact is known.
Until then #156's `depth > 0` exclusion stays: it breaks OOPIF recovery, but a wrong bind is
worse than the `FrameNotFound` an OOPIF gets today.

## 8. Real-browser test tier — APPROVED, spend freely

Rin: *"it's not our bill, it's GitHub's/Microsoft's."* No CI-minute constraint on the
live-Chrome tier. #158 established it with eight tests behind `integration-tests`; extend it
rather than rationing it.

## 9. `check_visible` live-Chrome fixture — APPROVED

The ~30 lines of HTML covering below-the-fold, opacity honeypot, hidden ancestor, fixed
position and quirks mode, plus fixtures for `check_stable`, `check_enabled` and
`check_receives_pointer`, which have no live coverage at all. Until it exists the module doc
must not claim the coverage.

## 10. Chrome 105–120 degradation — pass both option spellings

The problem: the predicate passes `opacityProperty` / `visibilityProperty` /
`contentVisibilityAuto`, which shipped in Chrome 121. Below 105 the call throws, which is a
loud, acceptable failure. On **105–120** it does not throw — WebIDL silently drops dictionary
members it does not recognise — so the call degrades to a bare `display: none` test and stops
covering `visibility: hidden` / `collapse` and `content-visibility: auto`, with nothing
warning the operator.

Feature-detection is possible (attach a `visibility: hidden` div, call
`checkVisibility({visibilityProperty: true})`, and see whether it answers `false`), but it
costs a round-trip and is not needed. 121 renamed those three options; the first two are
**aliases for 105's `checkOpacity` and `checkVisibilityCSS`**. So pass both spellings:

```js
el.checkVisibility({
    opacityProperty: true,    visibilityProperty: true, contentVisibilityAuto: true,
    checkOpacity:    true,    checkVisibilityCSS:  true,
})
```

Whichever pair the engine does not recognise is dropped silently — the same mechanism that
caused the bug now fixes it. 105–120 regains opacity and visibility-CSS coverage; only
`contentVisibilityAuto` stays unavailable there, because it genuinely did not exist. Two
extra keys, no probe, no version gate. Note in the rustdoc what 105–120 still lacks.

## 11. Harden the two duplicated temp-file writes — APPROVED (question answered)

Rin did not follow the question, and the round-1 framing was wrong: it cited
`persistence.rs`, which does not exist, and weighed "a new dependency plus a cross-crate
refactor". Neither is the real situation.

What is actually there: `crates/zendriver/src/io.rs` holds the hardened writer shipped in
#155 — `create_new` (`O_EXCL`), `0600`, mode preservation, atomic rename. `zendriver-fingerprints`
has two unhardened copies of the same idea, three lines each:

```rust
let tmp = cache.with_extension("tmp");
fs::write(&tmp, &bytes)?;
fs::rename(&tmp, cache)?;
```

at `generative/download.rs:42` and `pool/mod.rs:121`. A predictable path with no `O_EXCL`
follows a planted symlink and writes through it with this process's privileges — the exact
bug #155 fixed, still live in two places.

So this is a security fix, not a refactor for elegance, and the dependency cost is near zero:
`tempfile` is already in that crate's tree. `NamedTempFile::new_in(dir)` then `.persist(dest)`
gives `O_EXCL`, `0600`, a unique name and drop-cleanup — three lines replacing three lines.
`zendriver-fingerprints` cannot reuse `zendriver`'s `io.rs` because the dependency runs the
other way.

## 12. Monitor consecutive-failure backstop — APPROVED

Latch after N non-`-32601` failures. Rin: *"probably"*. Hardening against a failure mode
nobody has observed, but cheap.

## 13. Puppeteer parity on modifier+text — accept the divergence

Ratification, not a decision: the fix landed before this reached a human. zendriver now emits
Ctrl+Enter as `keyDown` with no text, which is neither Puppeteer's frame (`rawKeyDown`, no
text) nor the pre-fix frame (`keyDown` with text).

Recommendation: **keep it, document it.** Blink ignores `text` when a non-Shift modifier is
held (verified against Chrome 151), so behaviour is already correct in practice and the page
sees identical DOM events either way — the divergence is invisible to the thing we care about
not being detected by. Byte-level parity with Puppeteer buys nothing here, and for a stealth
crate resembling the most widely fingerprinted automation tool is not obviously a goal.

## 14. Blocklist decoding — lossy

Recommendation taken: a third-party host list carrying a non-UTF-8 byte loads lossily rather
than failing `Browser::launch()`. Host lines are ASCII and the offending bytes sit in comments
the parser discards, so fail-closed rejects a usable list over an irrelevant byte — and doing
it inside `launch()`, against a remote resource, with no cache write, is the worst available
combination. A round-1 fix had made it fail-closed without anyone choosing that; this reverts
the policy deliberately.

Document it in `tracker_blocklist_url`'s rustdoc beside the three bounds already recorded there.
