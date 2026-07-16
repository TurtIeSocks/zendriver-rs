# Task brief (increment 2): honor `visible_only` in the cross-frame find paths

## Context
Commit `b2161090` already made `visible_only` work for the single-scope `FindBuilder::one` / `FindAllBuilder::many` loops (nth-of-visible; synthesize-once; refresh-origin `nth` = original DOM index). BUT the cross-frame fan-out paths ignore `self.visible_only` entirely, so `tab.find()...visible_only(true).include_frames().one()/.many()` silently returns unfiltered results. Close that hole in ALL three frame paths. Same branch (`c/visible-only-filter`), same PR.

Files: `crates/zendriver/src/query/mod.rs` (the fan-out fns + their call sites) and the test file `crates/zendriver/tests/find_visible_only.rs`.

Confirmed facts:
- `RemoteRef` is `#[derive(Debug, Clone)]` (`selectors.rs:35`).
- `text_len_of(scope: &QueryScope, node: &RemoteRef) -> Result<usize>` reads text length from a `RemoteRef` (no Element).
- `check_visible(&Element) -> Result<bool>` (already imported in mod.rs) needs an `Element`.
- `resolver.synthesize(RemoteRef, &scope, nth) -> Element` and `Element::synthesize_query(RemoteRef, &scope, selector, nth) -> Element` both build Elements (consume the RemoteRef; clone it when you also need the ref for `text_len_of`).

## Call sites to thread `visible_only` through
`FindBuilder::one` calls `one_across_frames(tab, &resolver, want_nth, deadline)` (~mod.rs:877); `FindAllBuilder::many` calls `many_across_frames(tab, &resolver, deadline)`. Add a `visible_only: bool` parameter to both (pass `self.visible_only`), and to `consider_scope_best`.

## Path 1 — `many_across_frames` (easy)
For each scope (main, then each frame), synthesize each candidate with its original per-scope enumerate index, and push it iff `!visible_only || check_visible(&el).await?`. Preserve the main-first-then-frames order. (Mirror the single-scope `many` loop from `b2161090`.)

## Path 2 — `one_across_frames`, first-hit branch (moderate)
This is the `else` branch (no best_match). Semantics: **nth-of-visible across scopes**, in order main → frames(registry order). Carry a `visible_count` across scopes:
- For the main scope, then each frame scope in order: resolve candidates; for each (with original per-scope index) synthesize the Element; if `!visible_only` OR `check_visible` is true, it's a match; when the running visible/match count reaches `want_nth`, return that synthesized Element; else increment and continue.
- Refresh-origin `nth`: synthesize with the candidate's ORIGINAL index within its scope (same rationale as the single-scope fix).
- Keep the existing deadline/poll structure.

## Path 3 — `consider_scope_best` (the subtle one; best_match branch)
Add `visible_only: bool`. Currently: `resolve_many_inner` (already sorted by text-length closeness) → `nth(want_nth)` → `text_len_of` → update global best by distance. Change so that when `visible_only`, candidates are filtered to visible BEFORE the `nth` pick:
```
let hits = selector.resolve_many_inner(scope, true).await?;
let hits = if visible_only {
    let mut v = Vec::new();
    for r in hits {
        // throwaway Element just to probe visibility; refresh nth irrelevant here
        let el = Element::synthesize_query(r.clone(), scope, selector, 0);
        if check_visible(&el).await? { v.push(r); }
    }
    v
} else { hits };
let Some(candidate) = hits.into_iter().nth(want_nth) else { return Ok(()); };
// ... unchanged: text_len_of + distance + best update
```
So `want_nth` now selects the nth-closest among VISIBLE candidates in that scope; the cross-scope distance comparison is unchanged. The final returned element at the call site keeps `Element::synthesize_query(picked, &scope, selector, want_nth)` (best_match refresh is already approximate — do NOT rework its refresh index; just note it in a comment).
Thread `visible_only` into the two `consider_scope_best(...)` calls (main + per-frame) inside `one_across_frames`.

## ponytail / perf
All three add O(n) sequential `check_visible` probes, paid only when `visible_only` is set. Add a one-line `// ponytail:` note; do not parallelize.

## Tests (extend `crates/zendriver/tests/find_visible_only.rs`, real-Chrome `#[ignore]`)
Add cases (build a page with an iframe; put matching elements in BOTH main and the frame, some `display:none`):
1. `include_frames().visible_only(true).one()` → returns the first VISIBLE match across scopes (not a hidden earlier one).
2. `include_frames().visible_only(true).many()` → returns exactly the visible set across main+frame, in main-first order.
3. best_match + visible_only + include_frames: a text selector via `.best_match()`; a hidden candidate whose length is the closest and a visible candidate slightly farther → the VISIBLE one is returned (proves visibility filtering happens before length ranking).
Write tests first; confirm they fail against current (frame path ignores visible_only); implement; confirm pass. Run real Chrome if available; else commit `#[ignore]` tests + note you verified by compile+reasoning.

## Gates + commit
Same gate set as the first increment (build, fmt, clippy default + `interception`, `integration-tests` clippy for the test file, `cargo test -p zendriver --lib query::`). One commit:
`fix(query): honor visible_only across include_frames() fan-out paths`

Append a report to `.superpowers/sdd/visible-only-frames-report.md`.
