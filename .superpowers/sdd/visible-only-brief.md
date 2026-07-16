# Task brief: wire up the `visible_only` find filter (currently a NO-OP)

## Problem
`FindBuilder` and `FindAllBuilder` expose a public `.visible_only()` modifier that is silently discarded. Both poll loops in `crates/zendriver/src/query/mod.rs` contain:

```rust
// Visible-only filter: TODO(T16) — depends on
// `actionability::check_visible`, which depends on
// `Element::call_on_main`. Until that lands, treat every
// candidate as visible so the wider Find*Builder API can ship.
let _ = self.visible_only;
let filtered = candidates;
```

Sites: `FindBuilder` (~query/mod.rs:903-906) and `FindAllBuilder` (~query/mod.rs:1273-1277). The blocker is gone — `actionability::check_visible(el: &Element) -> Result<bool>` is fully implemented (`crates/zendriver/src/query/actionability.rs:70`) and runs a real JS visibility probe via `el.call_on_main`.

## What to build
When `self.visible_only` is true, filter candidates to only those that pass `actionability::check_visible` before nth-pick / collection. Mechanism:

- `resolver.resolve(&scope).await?` returns `Vec<RemoteRef>`.
- `resolver.synthesize(r: RemoteRef, &scope, nth: usize) -> Element` builds an `Element` (consumes the `RemoteRef`).
- `check_visible` needs an `&Element`, so you must synthesize a candidate to test it. Synthesize once and reuse — do NOT synthesize twice.

### FindBuilder (single, honors `want_nth`)
Semantics: **nth-of-visible** — return the `want_nth`-th candidate *among the visible ones*. Iterate candidates in order with their ORIGINAL index; synthesize each; if `visible_only` is off keep all, else keep only `check_visible`-true ones; count visible matches; when the visible-count reaches `want_nth`, return that already-synthesized `Element`. Early-stop once found (don't synthesize/probe the rest).

**Refresh-origin `nth` decision (important):** call `synthesize(r, &scope, ORIGINAL_INDEX)` — the element's stored refresh index must be its position in the unfiltered `resolve()` order, NOT its visible-rank. Rationale: a later `element.refresh()` re-runs `resolve()` (which does not re-apply `visible_only`) and picks by stored index; using the original DOM position keeps the handle pointing at the same node regardless of visibility drift. Document this in a one-line comment.

### FindAllBuilder (all matches)
Synthesize each candidate with its original enumerate index; keep it if `!visible_only || check_visible(&el).await?`. Return the kept vec when non-empty (same loop/deadline structure as today).

### Both
- `actionability::check_visible` is `pub(crate)` in the same crate — call it directly.
- It is async and one JS round-trip per candidate. Sequential awaits are fine (this cost is only paid when the caller opts into `visible_only`). Add a `// ponytail: O(n) sequential visibility probes, only when visible_only is set` note; do NOT parallelize speculatively.
- Delete both stale `TODO(T16)` comment blocks.
- `check_visible` carries `#[allow(dead_code)] // First callers ... land in T15/T18` at actionability.rs:69. Check whether it is now stale (does `Element::is_visible` / anything already call it?). If this wiring makes it a live caller and the allow is unneeded, remove the allow. If clippy still needs it, leave it and say why in the report.

## Tests (TDD)
`check_visible` runs real DOM JS, so a MockConnection unit test would have to hand-fake the `Runtime.callFunctionOn` visibility responses — brittle. Prefer a real-Chrome `#[ignore]` integration test (matches the existing actionability integration coverage; grep `#[ignore]` in `crates/zendriver/tests/` for the pattern + feature gates). Cases:
1. A page with a visible element and a `display:none` sibling matching the same selector → `find().<selector>().visible_only().one()` returns the visible one; `find_all(...).visible_only().many()` returns exactly the visible set.
2. nth-of-visible: 3 matches, middle one `display:none` → `.visible_only().nth(1)` returns the 3rd DOM element (2nd visible), proving nth counts visible only.

Write the test first, confirm it FAILS against the current NO-OP (it returns the hidden element / wrong count), then implement, then confirm PASS. If real Chrome isn't runnable in your environment, write the `#[ignore]` test anyway (so it's committed) and note in the report that you verified compilation + logic by reasoning, not a live run.

## Gates (run, fix before commit)
```
cargo test -p zendriver --lib query::   # existing query unit tests stay green
cargo build -p zendriver
cargo fmt --all
cargo clippy -p zendriver --all-targets -- -D warnings
cargo clippy -p zendriver --all-targets --features interception -- -D warnings
```

## Commit
One commit: `fix(query): honor visible_only by filtering finds through check_visible`
(Include the test file + the two loop edits + the actionability allow change if made.)
