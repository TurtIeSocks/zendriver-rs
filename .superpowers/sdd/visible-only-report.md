# Report: wire up `visible_only` find filter

## Files changed

- `crates/zendriver/src/query/mod.rs`
  - Added `use crate::query::actionability::check_visible;`
  - `FindBuilder::one()` (~line 899-935): replaced the `TODO(T16)` NO-OP
    block with a single loop over `candidates.into_iter().enumerate()`
    that synthesizes each candidate once, applies `check_visible` only
    when `self.visible_only` is set, counts visible matches toward
    `want_nth`, and breaks as soon as the target is found (later
    candidates are never synthesized or probed).
  - `FindAllBuilder::many()` (~line 1297-1320): same synthesize-once /
    conditional-`check_visible` pattern, collecting all kept elements
    into a `Vec` instead of stopping at a target index.
  - Deleted both stale `TODO(T16)` comment blocks.
- `crates/zendriver/src/query/actionability.rs`
  - Removed `#[allow(dead_code)]` on `check_visible` (see rationale
    below) and replaced it with a doc-note listing live callers.
- `crates/zendriver/tests/find_visible_only.rs` (new)
  - Two `#[ignore]` real-Chrome integration tests, mirroring
    `find_predicate_iframe.rs`'s harness (`wiremock` fixture + `Browser
    ::builder().headless(true)` + `tab.goto`/`wait_for_load`).

## Refresh-origin `nth` handling

Both loops synthesize each `RemoteRef` with its **original** enumerate
index from the unfiltered `resolve()` output (`resolver.synthesize(r,
&scope, i)` where `i` is the position in `candidates`, not the running
visible-rank counter). This matches the brief's rationale: `Element::
refresh()` re-runs `resolve()` and picks by stored index, and `resolve()`
does not re-apply `visible_only`. If the stored index were the
visible-rank instead, a refresh after a visibility change elsewhere in
the DOM could silently retarget the handle at a different node. Using
the original DOM position keeps the handle pointing at the same node
regardless of visibility drift. This is documented inline at both call
sites.

`want_nth` (`FindBuilder::one`) is tracked by a separate `visible_count`
counter that only increments on candidates that pass the visibility
gate (or all candidates, when `visible_only` is off) — i.e. nth-of-
visible semantics, not nth-of-DOM-position.

## Verification: real Chrome, not just reasoning

Ran the full TDD red→green cycle against a **real, locally launched
headless Chrome** (confirmed `Google Chrome.app` present on this
macOS box; `zendriver`'s bundled fetcher/launcher located and used it
successfully) — this was not a compilation-only or reasoned check.

1. Wrote `crates/zendriver/tests/find_visible_only.rs` first.
2. Stashed just the two implementation files
   (`git stash push --keep-index -- crates/zendriver/src/query/mod.rs
   crates/zendriver/src/query/actionability.rs`) to restore the
   NO-OP behavior while keeping the new test file in the working tree.
3. Ran the tests against the NO-OP — both **failed** as expected:
   ```
   cargo test -p zendriver --test find_visible_only --features integration-tests -- --ignored --test-threads=1
   ...
   test visible_only_filters_out_display_none_candidates ... FAILED
     left: "hidden"   right: "shown"
   test visible_only_nth_counts_only_visible_candidates ... FAILED
     left: "second"   right: "third"
   test result: FAILED. 0 passed; 2 failed;
   ```
4. `git stash pop` to restore the implementation.
5. Re-ran the same command — both **passed**:
   ```
   test visible_only_filters_out_display_none_candidates ... ok
   test visible_only_nth_counts_only_visible_candidates ... ok
   test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.39s
   ```

## Gates (all green)

```
$ cargo test -p zendriver --lib query::
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 273 filtered out

$ cargo build -p zendriver
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.77s

$ cargo fmt --all
(reformatted the new test file's assert_eq! wrapping only)
$ cargo fmt --all --check
(no output — clean)

$ cargo clippy -p zendriver --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.97s   # no warnings

$ cargo clippy -p zendriver --all-targets --features interception -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.98s   # no warnings

# extra (not in brief, but the new test file is feature-gated on
# integration-tests, so clippy'd it under that feature too):
$ cargo clippy -p zendriver --test find_visible_only --features integration-tests -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.37s   # no warnings
```

## `check_visible`'s `#[allow(dead_code)]` — removed

Checked before wiring: `Element::is_visible` (`crates/zendriver/src/
element/reads.rs:311-314`) **already** calls `actionability::
check_visible` unconditionally (no feature gate), so the allow was
already stale before this change — `is_visible` is public API with no
cfg gate, so `check_visible` was never actually dead code. This PR adds
a second live caller (the `visible_only` filter). Removed the
`#[allow(dead_code)]` entirely and replaced it with a short doc note
naming both call sites; `cargo clippy -D warnings` (both default and
`interception` feature) confirms clippy doesn't need it back.

## Self-review

- Both edit sites use the exact resolver contract described in the
  brief: `resolve() -> Vec<RemoteRef>`, `synthesize(RemoteRef, &scope,
  nth) -> Element` (consumes the ref), `check_visible(&Element) ->
  Result<bool>` (async, `pub(crate)`, same crate — imported directly,
  no `actionability::` prefix needed at call sites).
- Each candidate is synthesized exactly once and that same `Element` is
  reused for both the visibility probe and the return/collection value
  — no double-synthesis.
- Early-stop is real: `FindBuilder::one`'s `for` loop `break`s the
  moment `visible_count == want_nth`, so candidates after the match are
  neither synthesized nor probed (matches "Early-stop once found").
- Sequential `.await` per candidate, no speculative parallelism; the
  brief's exact `// ponytail: O(n) sequential visibility probes, only
  when visible_only is set` comment is present at both loops.
- The cross-frame fan-out helpers (`one_across_frames` /
  `many_across_frames`, ~line 1369/1450) do **not** honor
  `visible_only` — `self.visible_only` is never passed into them, so
  `tab.find().include_frames().visible_only(true)` silently ignores
  the filter today. This is a pre-existing gap outside the brief's two
  named edit sites (FindBuilder ~903, FindAllBuilder ~1273); flagging
  it here rather than scope-creeping the fix into this PR.
- Did not touch `one_or_none`/`many_or_empty` (they just delegate to
  `one`/`many` and translate `ElementNotFound`) — no change needed.
- MCP coverage / docs-sync: `.visible_only()` is not a new public API
  (it already existed on both builders before this change; this PR
  only fixes its silent NO-OP behavior), so no new MCP tool surface or
  README/rustdoc/book update is triggered by the project's MCP-coverage
  or documentation-sync gates.
