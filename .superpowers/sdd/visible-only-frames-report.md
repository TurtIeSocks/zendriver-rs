# Report: honor `visible_only` across `include_frames()` fan-out paths (increment 2)

Builds on `b2161090` (single-scope `visible_only` wiring). This increment
closes the cross-frame hole: `tab.find()...visible_only(true).include_frames().one()/.many()`
previously ignored `visible_only` entirely once it fanned out to
`one_across_frames` / `many_across_frames` / `consider_scope_best`.

## Files changed

- `crates/zendriver/src/query/mod.rs`
- `crates/zendriver/tests/find_visible_only.rs` (extended)

## Per-path changes

### Path 1 — `many_across_frames` (mirrors the single-scope `many()` loop)

Added `visible_only: bool` param (passed `self.visible_only` from
`FindAllBuilder::many`'s call site). For each scope (main, then each
frame, in that order), each candidate is synthesized once with its
**original per-scope enumerate index** and pushed into the result iff
`!visible_only || check_visible(&el).await?`. Main-first-then-frames
ordering preserved.

### Path 2 — `one_across_frames`, first-hit (`else`) branch

Added the same `visible_only: bool` param. Implements **nth-of-visible
across scopes**: a `visible_count` counter carries across the main scope
and every frame scope (processed main → frames in registry order). Each
candidate is synthesized once (original per-scope index), probed with
`check_visible` only when `visible_only` is set, and the loop returns the
synthesized `Element` the moment `visible_count == want_nth`; later
candidates in that scope (and any later scope) are never synthesized or
probed. Refresh-origin `nth` uses each candidate's original per-scope DOM
index, matching increment 1's rationale (`Element::refresh` re-runs
`resolve()` without re-applying `visible_only`, so the original DOM
position — not the visible-rank — is what survives a refresh after
visibility drift elsewhere in the DOM).

### Path 3 — `consider_scope_best` (best_match branch; the subtle one)

Added `visible_only: bool`. The visibility filter is inserted **before**
the `nth(want_nth)` pick, exactly as specified in the brief:

```rust
let hits = selector.resolve_many_inner(scope, true).await?;
let hits = if visible_only {
    let mut visible = Vec::new();
    for r in hits {
        let probe = Element::synthesize_query(r.clone(), scope, selector, 0);
        if check_visible(&probe).await? { visible.push(r); }
    }
    visible
} else { hits };
let Some(candidate) = hits.into_iter().nth(want_nth) else { return Ok(()); };
// ... unchanged: text_len_of + distance + best update
```

This means `want_nth` now selects the nth-closest-length candidate
**among visible candidates in that scope**; the cross-scope distance
comparison and the `best` update logic are untouched. `visible_only` is
threaded into both `consider_scope_best` call sites inside
`one_across_frames` (main + each frame). Per the brief, the final
`Element::synthesize_query(picked, &scope, selector, want_nth)` refresh
index at the call site was **not** reworked — it was already approximate
before this change (re-synthesizes with `want_nth` rather than the picked
candidate's true position), and a comment now flags that explicitly
rather than silently leaving it unexplained.

### `ponytail` notes

All three paths carry a `// ponytail: O(n) sequential visibility probes,
only when visible_only is set` comment at the loop that does the
probing. Not parallelized, per the brief.

## Pre-existing bug discovered + fixed in the same commit: `Tab::frames()` includes the main frame

While writing the first two new tests (main scope has real matches, not
just frame matches — needed to actually exercise `visible_only` crossing
into a frame), `FindAllBuilder::many().include_frames()` returned **6**
elements instead of the expected **2** (2 main + 2 frame, each doubled).
Root cause: `Tab::frames()` is documented to "include the top-level frame
... plus every same-origin sub-frame" — confirmed live via a debug probe
(`tab.frames()` returned 2 entries for a page with exactly one iframe: the
child frame AND the main frame itself, `is_main() == true`). `
many_across_frames` and `one_across_frames` queried `main_scope` explicitly
*and* iterated `tab.frames()` unfiltered, so the main-frame entry in that
list caused the main document to be queried a second time via
`QueryScope::Frame(main_frame)` — double-counting every main-document
match in `many()`, and (harmlessly, since ties keep the earlier entry)
redundant work in `one_across_frames`'s best_match branch.

This is squarely inside the functions this brief assigned me to edit, and
it directly blocked the new tests (not just cosmetically — `many()`
returned duplicates), so I fixed it in the same commit rather than working
around it: both fan-out functions now filter `tab.frames()` to
`!f.is_main()` before looping over "frames after main". Documented at the
top of `one_across_frames`'s doc comment (referenced from
`many_across_frames`).

## Pre-existing bug found but NOT fixed (out of scope, flagged separately)

`resolve_text_many` (in `crates/zendriver/src/query/selectors.rs`, the
resolver behind `.text()`/`.text_exact()`) never threads `contextId` into
its `Runtime.evaluate` calls for `QueryScope::Frame`, unlike
`resolve_css_many` / `resolve_predicate_many` (which both call
`scope.execution_context_id()` and set `params["contextId"]`). Confirmed
live: a `.text("target").include_frames()` query where the target text
lived only inside a child iframe evaluated against the **main** document
instead (a debug probe on the "frame-scoped" candidate returned the main
document's node). This affects any cross-frame **text** selector query,
independent of `visible_only` — out of scope for this brief (files:
`mod.rs`), a materially different fix (CDP dispatch in `selectors.rs`,
broader blast radius, needs its own tests). I redesigned test 3 (see
below) to avoid tripping it, and I'm flagging it here rather than
papering over it or silently fixing it outside brief scope.

## Tests (`crates/zendriver/tests/find_visible_only.rs`)

Added 3 new `#[ignore]` real-Chrome tests (2 existing tests from
increment 1 untouched):

1. `include_frames_visible_only_one_skips_hidden_main_for_visible_frame_match`
   — main has a hidden `.cross`; a same-origin iframe has a visible
   `.cross`. `.css(".cross").include_frames().visible_only(true).one()`
   must return the frame's visible element, not stop at the main
   document's hidden first hit.
2. `include_frames_visible_only_many_returns_visible_set_main_first` —
   main and the iframe each have one hidden + one visible `.cross`.
   `.find_all().css(".cross").include_frames().visible_only(true).many()`
   must return exactly the 2 visible elements, main-first.
3. `include_frames_best_match_visible_only_prefers_visible_over_closer_length`
   — main has a hidden `.cross` whose text length is closest to the
   needle and a visible `.cross` whose text is farther; a harmless
   same-origin iframe with no matching text is present purely to exercise
   the frame-loop code path. `.text("target").best_match().include_frames()
   .visible_only(true).one()` must return the farther-but-visible
   candidate, proving visibility filtering happens before length ranking.
   (Both competing candidates are kept in the **main** scope deliberately
   — see the `resolve_text_many` bug above; splitting them across scopes
   would have made the test's pass/fail depend on that unrelated,
   pre-existing gap instead of the code this brief targets.)

Added a `wait_for_child_frame` helper (mirrors `find_predicate_iframe.rs`'s
wait loop) so `include_frames()` queries always have a registered
same-origin iframe document to descend into.

## Verification: real Chrome, full TDD red → green cycle

Chrome is present in this environment (`Google Chrome.app`) and launched
successfully — same as increment 1. Full cycle, `--test-threads=1`:

1. Wrote the 3 new tests against the (at-the-time) unmodified
   `one_across_frames` / `many_across_frames` / `consider_scope_best`.
2. Confirmed RED — all 3 failed for the right reason:
   ```
   test include_frames_visible_only_one_skips_hidden_main_for_visible_frame_match ... FAILED
     left: "main-hidden"   right: "frame-visible"
   test include_frames_visible_only_many_returns_visible_set_main_first ... FAILED
     left: 6   right: 2
   test include_frames_best_match_visible_only_prefers_visible_over_closer_length ... FAILED
     left: "hidden-close"   right: "visible-far"
   test result: FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
   ```
   (Confirmed via `git stash push --keep-index -- crates/zendriver/src/query/mod.rs`
   to restore the pre-fix implementation while keeping the new tests, then
   `git stash pop` to restore the fix.)
3. Implemented the fix (including the `is_main()` filter fix above).
4. Confirmed GREEN — all 5 tests pass:
   ```
   $ cargo test -p zendriver --test find_visible_only --features integration-tests -- --ignored --test-threads=1
   test include_frames_best_match_visible_only_prefers_visible_over_closer_length ... ok
   test include_frames_visible_only_many_returns_visible_set_main_first ... ok
   test include_frames_visible_only_one_skips_hidden_main_for_visible_frame_match ... ok
   test visible_only_filters_out_display_none_candidates ... ok
   test visible_only_nth_counts_only_visible_candidates ... ok
   test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.26s
   ```

## Gates (all green)

```
$ cargo build -p zendriver
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.80s

$ cargo test -p zendriver --lib query::
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 273 filtered out

$ cargo fmt --all
(reformatted the new test cases' line-wrapping only)
$ cargo fmt --all --check
(clean, exit 0)

$ cargo clippy -p zendriver --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.58s   # no warnings

$ cargo clippy -p zendriver --all-targets --locked --features interception -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.84s   # no warnings

$ cargo clippy -p zendriver --test find_visible_only --features integration-tests --locked -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.20s   # no warnings

$ cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.70s   # no warnings, full workspace
```

## MCP coverage / docs sync

`visible_only` and `include_frames` are pre-existing public API (already
wired into the MCP `find` tool — `crates/zendriver-mcp/src/tools/find.rs:211,217`
call `builder.visible_only(sel.visible_only)`); this PR only fixes their
silent no-op interaction, adding no new public API surface. No MCP tool
change, no README/rustdoc/book update triggered.

## Self-review

- Resolver contract used exactly as documented in the brief:
  `RemoteRef` is `Clone`; `check_visible(&Element) -> Result<bool>`;
  `Element::synthesize_query(RemoteRef, &scope, selector, nth) -> Element`
  (consumes the ref, hence the `.clone()` in `consider_scope_best`'s probe
  before also passing the ref along for the eventual `nth` pick);
  `text_len_of(scope, &RemoteRef) -> Result<usize>` unchanged.
- Each candidate synthesized once and reused for both the visibility
  probe and the return/collection value in Paths 1 and 2 (no
  double-synthesis); Path 3's probe Element is explicitly a throwaway
  (documented inline) since the real returned Element is re-synthesized
  at the `one_across_frames` call site regardless.
- Early-stop preserved in Path 2: returns the instant
  `visible_count == want_nth`, so later candidates (in that scope or any
  later scope) are never synthesized or probed.
- All three probing loops are sequential `.await`, no speculative
  parallelism, `ponytail` comment present at each.
- Did not touch `one_or_none`/`many_or_empty` — they delegate to
  `one`/`many` and translate `ElementNotFound`, no change needed.
- Did not rework best_match's approximate cross-scope refresh index, per
  the brief — added a comment instead.
- Fixed the `Tab::frames()`-includes-main double-counting bug because it
  was inside the assigned functions and directly blocked the new tests;
  did NOT fix the unrelated `resolve_text_many` missing-`contextId` bug
  (different file, different mechanism, broader scope) — flagged instead
  and worked around it in test 3's fixture design.
