# Founder's review, round 2 — PRs #150, #152, #155, #156, #157, #158

> ## Correction, applied after this report was generated
>
> **This document originally recorded #152 and #156 as "Done" with zero findings.
> That was wrong.** Both of their round-2 reviewers crashed mid-response
> (`API Error: Server error mid-response`). The synthesis read the resulting
> silence as a clean result and wrote them up as needing no further review.
>
> They were re-reviewed, and between them returned **25 findings**, including
> round 2's only two HIGH items — both introduced by round-1 fixes:
>
> - **#156**: the spawn refactor severed two tests' synchronisation. Deleting the
>   live-tree containment check, the entire point of the probe, left all ten
>   tests green; so did inverting the probe to fail closed.
> - **#156**: the write-through stopgap's comment claimed it keeps `Frame::url()`
>   live on a pre-rename handle. It repairs only the renaming navigation.
> - **#152**: the JavaScript-style rationale asserted `NodeList.prototype.length`
>   and `String.prototype.includes` are not page-instrumentable. Both are, and
>   the second is a *better* detector than the hook the comment warns about.
> - **#152**: `ChallengeGone`, a success terminal, was documented as "the iframe
>   was torn down" when the condition also fires for a mounted-but-unclickable
>   widget. The same false claim was then found in five further files.
>
> The tables and counts below are as originally generated and therefore
> **understate the round-2 total**: 22 becomes 47, and 0 high becomes 2 high.
>
> The failure mode is the one this whole review series exists to document:
> **absence of a red signal was read as evidence of correctness.** A crashed
> reviewer and a clean reviewer produce the same empty findings list, and nothing
> in the pipeline distinguished them.

Internal engineering document. Round 1 reviewed six stacked PRs line by line and
produced 108 findings (7 high, 26 medium, 75 low); 96 were fixed and 12 were
routed to a human. Round 2 re-reviewed the fixed tree with the same
reviewer-then-adversarial-verifier shape. **22 findings survived verification, 0
were struck, and 19 of the 22 were flagged as introduced by round-1 fixes.**

Severity tally: **0 high, 5 medium, 17 low.**

| PR | Scope | R1 findings | R2 findings | High | Medium | Low | Mergeable now? |
|----|-------|-------------|-------------|------|--------|-----|----------------|
| [#150](#150--transport) | transport | 16 | 5 | 0 | 0 | 5 | Yes, after five lines |
| [#155](#155--core-io) | core I/O | 20 | 5 | 0 | 2 | 3 | No |
| [#157](#157--input-and-gate) | input + actionability gate | 25 | 6 | 0 | 0 | 6 | Yes, after one clause, with a caveat |
| [#158](#158--visibility-and-tests) | text normalization + regression tests | 15 | 6 | 0 | 3 | 3 | No |
| #152 | solvers | 17 | 8 | 0 | 2 | 6 | No — reviewer crashed; see correction |
| #156 | frame registry + geo | 15 | 17 | 2 | 6 | 9 | No — reviewer crashed; see correction |

---

## 1. Convergence

**The loop is converging, but less than the numbers below suggest — see the
correction at the top: #152 and #156 were never reviewed when this was written,
and together add 25 findings including both of round 2's HIGH items.**

By count: the four PRs still carrying findings went from 76 findings in round 1 to
22 in round 2, a 71% drop. By severity the drop is larger and matters more. Those
four PRs carried 3 high and 18 medium in round 1; round 2 found **no high
findings at all** and 5 medium, of which 2 are comments, 1 is a test-coverage gap
in code that is correct today, and only 2 are live defects a user can hit
(`tracker.rs` strict UTF-8, and the element-scope XPath leak that predates the
PR). Everything else is prose, dead code, or a test barrier.

**#152 and #156 came back with zero findings.** Round 1 put 32 findings across
those two PRs, including 4 of the 7 high findings in the entire audit (the
provisional-sibling probe blocking the frame-lifecycle loop, the frame-name
doc, the Imperva deadline race). Round 2 produced nothing on either. They are
done and should merge without further review. Note the honest boundary on that
claim: "done" here rests on zero surviving findings, not on a positive
re-verification report, because no per-PR reviewer assessment came back for
either.

The part that has *not* converged is the defect **class**. 19 of 22 survivors
were flagged as artifacts of round-1 fixes; verification demoted 2 of those (see
§3), so 17 stand. Round 1 fixed 96 findings and introduced 17, an introduction
rate near 18% per fix batch. That rate is only tolerable because the introduced
items are strictly less severe than the ones they replaced: round 1 removed 7
high findings and introduced none, and 14 of the 17 introductions are comments,
docs, or dead code. The shape is almost always the same. A fix corrects a claim
in one place and leaves an adjacent claim about the same behaviour untouched, or
writes a new claim that is broader than what the code does. Four of the
seventeen are literally "the rustdoc was fixed and the inline comment fifteen
lines below still says the old thing".

What that implies for round 3. A round-2 fix pass of 22 items at the same
introduction rate yields roughly 4 new low findings, which is below the cost of
convening a full review. So: fix round 2, then run a **targeted check limited to
the round-2 diff**, not a re-review of the PRs. Two mechanical rules would have
caught most of the seventeen and should be applied while fixing, not after:

- Every doc or comment fix ends with `grep -rn '<the phrase you just corrected>'
  crates/` and the grep must come back empty or explained. This alone catches
  R2-150-2, R2-150-3, R2-157-6 and R2-158-2.
- A fix that edits a rustdoc must also re-read the inline comments inside that
  function and the CHANGELOG entry that describes it, before the commit. This
  catches R2-155-3, R2-157-6, R2-158-1 and R2-158-6.

Remaining work is polish plus two code fixes and one test. Nothing in round 2
argues against any design decision round 1 approved.

---

## 2. Verdict per PR

### #150 — transport

**Mergeable now: yes, after five lines.** Nothing here blocks on engineering.
There is no new bug, no vacuous test, no overreach, and the sweep coverage that
was round 1's real gap for this PR is now genuine and was proved by mutation.
What is left is entirely the class this series keeps producing: a comment that
states as fact two things a fifteen-line test disproves, and a doc sweep that hit
four of five sites and left the most-read one contradicting its own body eight
lines below. This repo treats a false claim on a public doc as correctness rather
than polish, and the comment in R2-150-1 is specifically the artifact round 1
wrote *to close a finding about unpinned behaviour*, so fix the five lines and
merge. Do not spend another round on this PR.

**Did round 1 fix what it claimed?** On the whole yes. All sixteen findings were
addressed or consciously deferred. Verified independently: `swept_total` is
genuinely not load-bearing (written once at `actor.rs:226`, read nowhere in
production code); the `duplex_pair` consolidation preserved the exact capacities
the three former copies used, so no test's plumbing silently changed;
`CMD_CHANNEL_CAPACITY` is 64 at both `mpsc::channel` sites, identical to before;
the `InFlightCall` helper keeps the inbound sender and outbound receiver alive in
all four drain tests exactly as the hand-rolled versions did; `clippy
--all-targets --all-features -D warnings` and `fmt --check` are clean, and the one
`cargo doc` intra-doc warning (`CdpRpcError` at `connection.rs:83`) predates the
PR on `main`. Where it fell short is the doc sweep it was proudest of: 150-6
asked for five sites and swept four, leaving the *summary line* of the method
every other site redirects readers to; and 150-1/150-2 rewrote both `Timeout`
variant docs while leaving the `method` and `budget` field docs three lines below
asserting "never answered".

**Non-vacuity spot-checks.** Three, by mutation, tree restored each time.
(1) `the_sweep_reaps_abandoned_pendings_and_spares_live_ones` with the guard
short-circuited fails with "swept 0 of 512 abandoned callers across 513
dispatches", so the headline new coverage is real; the `biased` select ordering
argument was re-derived rather than trusted, and awaiting the live reply is a
genuine happens-before barrier for the `Relaxed` load, no sleep involved.
(2) The URL-leak assertion in `redial_times_out_instead_of_hanging_forever` is
non-vacuous by construction. (3) The claim most in doubt, that the `socket_died`
clear cannot be pinned, was falsified: the fifteen-line test the comment says is
impossible passes, and commenting out the clear turns it red. That is R2-150-1.

### #155 — core I/O

**Mergeable now: no.** The security fix at the centre of this PR is real,
correctly implemented and properly proven. The temp file is owner-only before the
first payload byte, `O_EXCL` refuses a planted symlink, and all three of those
tests fail against the pre-fix shape. What blocks the merge is that nothing
connects the cookie jar to that helper: reverting both save paths to plain
`fs::write` leaves the entire suite green, so the PR's headline behaviour is
unlocked and one refactor away from silently disappearing. A round-1 fix also
changed the blocklist decode from lossy to fail-closed, which turns a stray
Windows-1252 byte in a third-party host list into a `Browser::launch()` failure
that never caches and therefore never recovers. Both are small, well-localised
edits. Nothing here argues against the design; the atomic write, the `crate::io`
placement, the three-way error split and the size cap are all the right calls.

**Did round 1 fix what it claimed?** Largely honest work. All 20 findings are
genuinely addressed: `create_new(true).mode(0o600)` replaces the umask-then-narrow
open, `temp_path` returns `io::Result` and carries a nanosecond component, the
cleanup block is written once in `fill_then_rename`, the helper moved to
`crate::io`, `num_alive_tasks` is gone crate-wide, the tracker filesystem calls
moved to `tokio::fs` with a `NotFound`-vs-other split, the blocklist gained a
content-length check plus a chunk-accumulation cap, the setgid mask got a test,
the rename-failure test asserts the error kind, and the three naming and comment
findings landed. `fmt --check`, `clippy --workspace --all-targets -D warnings` and
`clippy -p zendriver --no-default-features` are clean. Two slips: the 155-1 size
bound swapped reqwest's lossy `text()` for a strict `String::from_utf8`
(R2-155-2), and the 155-8 visibility fix widened the warn arm so ordinary
connection teardown now logs (R2-155-3, and note the verifier established this is
an improvement on the pre-PR behaviour rather than a regression). The fix
commit's claim that every behaviour change is proven by mutation holds for
everything *that commit* touched, but it does not extend to the PR's headline
change, which commit 1 shipped with tests that cannot see it.

**Non-vacuity spot-checks.** Six mutations, each restored and `git diff` verified
clean. (1) `create_new(true).mode(TEMP_FILE_MODE)` downgraded to
`create(true).truncate(true)`: three `io.rs` tests fail, and the mid-write
sampler really does observe 0644 on 10 of 10 samples. (2) The `-32601` equality
widened to `code.is_some()`: the lost-race test fails. (3) The call-site latch
guard deleted: the method-not-found test fails, though it is an
assert-absence-after-sleep (R2-155-4). (4) Both size checks disabled: both
blocklist tests fail. (5) The `tx.closed()` select arm replaced with `pending()`:
the expectation-drop test fails. (6) **Both cookie save paths reverted to
`fs::write`: all six `cookies::persistence` tests pass, including the one named
`both_save_paths_write_atomically`.** That claim does not hold, and it is
R2-155-1.

### #157 — input and gate

**Mergeable now: yes, after one clause, with a caveat that belongs to the
series rather than to this commit.** The round-1 commit is good work and the
mutation evidence backs its claims, but it must not ship with the
`actionability.rs` module doc telling the next reader that the visibility
predicate has behavioural coverage in `tests/find_visible_only.rs` for viewport,
quirks-mode, opacity and content-visibility. That file is untouched by the PR and
covers `display:none` plus three `include_frames` combinations. Deleting or
narrowing one clause fixes it. The other five findings are one-line doc and
comment corrections plus a two-site code edit. The caveat: round 1's deferral of
#157-1 and #157-2 is correctly handled and honestly declared, but it means this
PR still ships `visible_only` and `is_visible` with rustdoc that omits the
viewport requirement, so the human call in §7 item 1 is a blocker for the series
even though it is not a defect in this commit.

**Did round 1 fix what it claimed?** Strongly. 23 of 25 findings are genuinely
fixed; the two omissions are exactly the ones round 1 routed to a human, and the
commit message says so instead of quietly skipping them. The fix went further
than asked in the right places, routing `mouse_drag`'s wire value through
`buttons_held` rather than only softening the doc, and rewriting the
`check_visible` source pins so the two mutations round 1 used to discredit them
now fail. Two real problems with the round. The `check_visible` module doc claims
behavioural coverage that does not exist at that granularity, introduced by the
commit whose subject line is "correct four contradicted comments". And #157-9 was
explicitly reserved for a human in round 1's §5 item 11; the fix implemented it
anyway and got the parity claim half wrong, since Puppeteer sends Ctrl+Enter as
`rawKeyDown`. Everything else checks out against the code: `HOVER`, `hit_point`,
the `js_params` relocation, `try_expect`, `probe_count`, the screenshot comment
triple, the `IDLE_FALLBACK_TICK` audit, `ClickOptions::click_count`,
`clear_by_deleting` step 4, and `tap_at`'s cursor assertion.

**Non-vacuity spot-checks.** Nine mutations, worktree verified clean afterwards,
all nine claims held. Unconditional `click_at` cleanup restored fails the
never-latched test. The fixed 5px pacing assumption restored fails the segment
test (300ms observed against ~600ms expected). Removing `.skip(1)` fails the
already-on-this-point test at 61 frames against 60. Replacing `mouse_drag`'s
`buttons_held.insert` with a local `LEFT.bits()` now fails the mid-gesture
assertion, which is round 1's #157-5 and the exact mutation that used to leave
all four drag tests green. Deleting the insert outright fails two tests. Moving
the opacity threshold back outside the ancestor walk fails its probe test.
Swapping the quirks-mode ternary arms, the mutation round 1 used to show the old
tests pinned nothing, now fails. Transposing `function(el, dx, dy)` fails through
the relocated `js_params`, which the old `starts_with("function(el")` could not
catch. Reverting the three keyboard text-suppression edits fails three tests, and
folding event-type selection into text presence, the "obvious one-line version"
the commit argues against, fails on the `rawKeyDown` half.

### #158 — visibility and tests

**Mergeable now: no.** The headline deletion is verified safe: `resolve_one` and
all five `resolve_*_one` helpers have zero remaining references anywhere in the
workspace, every deleted item was private so the public-API baseline is
untouched, all six migrated tests drive `resolve_many` with equal-or-better
coverage, and `cargo test -p zendriver --lib` is 408/0 with `fmt` and `clippy`
clean. What blocks it is a claim, not code. The fix's own premise was probed
against real Chrome 151 and it splits: `captureBeyondViewport` genuinely rescues
a below-the-fold clip (blank without the flag, correct with it), but the
`position: fixed` side effect the commit documents in four places, and publishes
in the CHANGELOG under `### Changed` as a behaviour change existing callers will
see, does not happen on any capture shape tested. That is the sixth comment in
this series asserting something its code does not do, and this one ships to
crates.io and docs.rs. Fix it, fix the stale "Two paths" block comment that still
recommends the quote-swap this commit deleted for being broken, and fix the
element-scope `//` leak the extraction cemented, and the PR is done.

**Did round 1 fix what it claimed?** Almost everything, and the two most
substantial items are well done. The deletion is real and independently verified
by grepping the whole workspace for every deleted symbol (only historical plan
docs remain). `xpath_string_literal` is correct on every input traced by hand and
is backed by a real-Chrome test that asserts the expression *evaluates* rather
than only that it was generated, which is exactly the gap round 1 named. The
`.trim()` to `replace(/^ | $/g,"")` swap is byte-faithful to `normalize_space`,
and the `TextPred::Equals` rustdoc now states the residual `innerText`-vs-string-
value divergence instead of claiming agreement. The regression-test header
numbers its cases in order and states which are fixed in the parent commit. Two
overreaches. 158-5 was fixed by adding the flag *and* by writing an empirically
false `position: fixed` caveat into the module doc, the `clip` rustdoc, an inline
comment in `element/screenshot.rs`, and the public CHANGELOG, so the fix bought a
real behaviour improvement and a false public claim in one commit. And extracting
`text_exact_xpath` merged the tab-scope and element-scope expressions into one
shared builder; both were already wrong for element scope, but the extraction
removed the seam where the `.//` fix would naturally go and added a comment that
reads as if scoping works. One thing round 1 missed entirely: the 14-line block
comment at `selectors.rs:333` still documents the deleted `_one` resolvers and
still presents the quote-swap as the mechanism that keeps multi-quote needles
safe.

**Non-vacuity spot-checks.** The single most load-bearing test was mutation
tested, since it is the one round 1 exists because of: deleting the
`normalize_space` fold from the shipping `resolve_text_many` path turns
`text_exact_folds_whitespace_in_the_needle_before_building_the_xpath` red with
the unfolded needle visible in the message (67 passed, 1 failed). That is the
mutation that used to leave the suite green. The rest were inspected rather than
mutated because their assertions are structurally non-vacuous: the three quote
tests assert the complete emitted `var xp="…";` fragment, so they cannot pass
against the old quote-swap, and `js_normalize_space_edge_strip_is_not_trim`
asserts both the absence of `.trim()` and the presence of the edge strip. The one
test whose sharpness should not be overstated is
`caller_clip_below_the_fold_sends_capture_beyond_viewport`: asserting the flag is
on the wire is right for a unit test, but **nothing in the suite exercises what
the flag does**, which is precisely how the false `position: fixed` claim
survived review. A Chrome 151 probe filled that gap and produced R2-158-1.

---

## 3. Regressions introduced by round-1 fixes

This section is the point of the loop. Round 1 fixed 96 findings; 19 of round 2's
22 survivors were flagged as artifacts of those fixes. Verification demoted two
of the nineteen:

- [testing.rs:51](crates/zendriver-transport/src/testing.rs:51) was flagged as a
  new duplication. It is not. The pre-fix `duplex_pair` lived inside
  `#[cfg(test)] mod tests` and was equally unreachable from `testing.rs`, so
  round 1 took the construction from 7 copies to 5. It is pre-existing cleanup,
  carried in §4 at low.
- [monitor/mod.rs:1056](crates/zendriver/src/monitor/mod.rs:1056) was flagged as
  a new warn-on-shutdown. Backwards: pre-PR, the same transport-shutdown error
  already warned, additionally misattributed the failure to "needs Chrome ~124+",
  *and* latched the flag that permanently disabled streaming for the monitor.
  Round 1 strictly improved this path. What remains is one imprecise trailing
  clause, carried in §4 at low.

Seventeen stand. They are grouped by shape, because the shape is the finding.

### 3.1 Behaviour changed for the worse (2)

**R2-155-2 — a lossy decode became fail-closed, inside `Browser::launch()`.**
[tracker.rs:135](crates/zendriver/src/tracker.rs:135). The chunk-accumulation
rewrite that closed the size finding ends in `String::from_utf8(body)`. The
`Response::text()` it replaced never fails on encoding: with reqwest built
without the `charset` feature, `text()` is `String::from_utf8_lossy`. So the size
fix silently changed the decode contract, and no test covers it. Medium. Details
and fix in §4.

**R2-157-4 — the `we_latched` guard removed the incidental sweep of a stranded
bit.** [mouse.rs:265](crates/zendriver/src/input/mouse.rs:265). Round 1's fix for
#157-25 is correct for the case it names, but the pre-fix unconditional
`buttons_held.remove(bit)` at exit also swept a bit stranded by a cancelled
gesture. Now a `click_at` that finds the bit already set records `we_latched =
false` and skips cleanup if its `mousePressed` fails. The window is narrower than
first claimed and the release path still self-heals on the next successful click,
so this is low and the fix is a comment, not code.

### 3.2 A round-1 fix landed on some paths and not others (5)

Five fixes corrected one site and left a sibling site asserting the old thing,
inside the same file and usually inside the same screen.

| Finding | Fixed | Left behind |
|---------|-------|-------------|
| R2-150-2 | four "reply budget" sites | the rustdoc **summary line** of the method the other four link to |
| R2-150-3 | both `Timeout` variant docs | the `method` and `budget` **field** docs three lines below, in both crates |
| R2-157-6 | `clear_by_deleting`'s rustdoc | the inline comment fifteen lines down, still saying "one probe" |
| R2-157-2 | modifier suppression on 2 of 3 text-carrying paths | the `char`-event fallback and the `cluster_events` multi-codepoint branch |
| R2-158-2 | the text-selector code | the 14-line section comment that documents the deleted design |

R2-157-2 is the one with user-visible consequence:
`press_with(Key::Char('a'), CTRL)` now correctly sends no text while
`press_with(Key::Char('é'), CTRL)` still inserts "é". Note the verifier's
correction: no path got *worse*, since pre-fix every path sent text. Round 1
fixed two thirds and created the asymmetry.

### 3.3 New claims that are false, unverifiable, or broader than the code (6)

**R2-158-1 is the worst finding in round 2** and the reason this section exists.
Closing #158-5 wrote a `position: fixed` caveat into four places including the
public CHANGELOG under `### Changed`, and the behaviour it describes does not
occur on Chrome 151 in any capture shape tested: clipped on-screen captures are
byte-identical with and without the flag, so the `### Changed` framing is wrong
twice over. It ships to crates.io and docs.rs. Medium.

The other five: the `socket_died` comment asserting both that no test can pin the
line and that the flag has exactly one reader, when a fifteen-line test pins it
and `ConnectionInner::Debug` reads it on a line the same commit edited
(R2-150-1); the `actionability.rs` module doc claiming live-Chrome behavioural
coverage at a granularity that does not exist (R2-157-1); the
`modifiers_suppress_text` doc justifying a deliberate divergence from Puppeteer
by claiming parity with Puppeteer (R2-157-3); the `emit_request_and_settle` doc
naming a hazard its sleep cannot cover, which is the barrier the *caller* already
provides (R2-155-4); and a CHANGELOG entry that describes only the widening half
of a two-directional matching change (R2-158-6).

### 3.4 Facts traded for identifiers, and dead code added (4)

Two DRY fixes replaced a concrete number in a **public** rustdoc with the name of
a `pub(crate)` constant, which does not render on docs.rs and is not a working
intra-doc link. The reader loses the only fact the sentence carried: the queue
depth in [connection.rs:295](crates/zendriver-transport/src/connection.rs:295)
and `DEFAULT_CALL_TIMEOUT`'s doc, and the 50ms latency bound in
[tab.rs:1813](crates/zendriver/src/tab.rs:1813). `cargo doc` reports the second
as two private-intra-doc warnings.

Two more added inert code: an unreachable `parts.len() < 2` guard whose worked
example describes an input that cannot arrive
([selectors.rs:470](crates/zendriver/src/query/selectors.rs:470)), and a JS
parameter `_n` that is never read while the caller still serialises and ships the
needle to fill it
([selectors.rs:510](crates/zendriver/src/query/selectors.rs:510)).

---

## 4. Remaining findings

Severity-ordered within each PR. Findings carry the id `R2-<pr>-<n>`.

### #158 — visibility and tests

#### R2-158-1. The `position: fixed` caveat does not happen, and it is published

[screenshot/mod.rs:56](crates/zendriver/src/screenshot/mod.rs:56) · **medium** ·
false-comment · introduced by round 1

**What.** The new module doc states as fact that `captureBeyondViewport` makes
`position: fixed` elements render at their document position rather than pinned
to the viewport, so a sticky header can appear in an unexpected place in a
clipped shot. The same claim is repeated at
[screenshot/mod.rs:259](crates/zendriver/src/screenshot/mod.rs:259) (the
`ScreenshotBuilder::clip` rustdoc, which points readers at the module doc), at
[element/screenshot.rs:139](crates/zendriver/src/element/screenshot.rs:139), and
at [CHANGELOG.md:12](crates/zendriver/CHANGELOG.md:12) under `### Changed`, where
it tells every existing caller their pixels have changed.

**Why it matters.** It was measured, twice, independently. Fixture: 800x600
viewport, 6000px page, a `position: fixed` 100x50 red box at viewport (0,0), a
blue band at document y=2000, a green band at y=5000, scrolled to y=2000, Chrome
151. A clip of `{x:0, y:2000, w:100, h:50}` came back **byte-identical** with and
without the flag, red in both, so the fixed box is still pinned to the viewport.
The same clip at document y=0 with the flag came back white, so the box did not
relocate to the document top either. The full-page shape the doc's section covers
behaves the same way: with and without the flag the red box sits at document
y=2000, and the document top is white. The premise of the fix is sound, because
`{x:300, y:5000}` is white without the flag and green with it, so the flag really
does rescue a below-the-fold clip. Only the caveat is invented. Concretely: a
user reads the CHANGELOG, sees a `### Changed` entry saying their sticky headers
may now land somewhere unexpected, and spends an afternoon re-checking every
clipped screenshot in their pipeline for a difference that does not exist. A
junior reads the `clip` rustdoc on docs.rs and codes around a caveat that is not
there. The reason it slipped is worth naming: the only test for the flag asserts
it appears in the params, so a claim about what the flag *does* had nothing to
fail against.

**Fix.** Delete the `position: fixed` sentence from all four places. Drop the
`### Changed` CHANGELOG section: with the caveat gone the flag has no observable
effect on an on-screen clip, so the entry belongs under `### Fixed` next to the
`Element::screenshot` line as a single "clipped captures no longer come back
blank when the rect reaches past the rendered area". If the caveat is kept as a
hedge for older Chrome it must be stated as a version-bounded historical note
with a citation, not as current behaviour. Then add the empirical check as an
`integration-tests` case: scroll a tall page, clip a band below the fold, assert
the captured PNG is not uniform.

#### R2-158-2. The section comment documents the deleted design and recommends the deleted bug

[selectors.rs:333](crates/zendriver/src/query/selectors.rs:333) · **medium** ·
stale-comment · introduced by round 1

**What.** The 14-line `// Text (case-insensitive substring or whitespace-collapsed
exact)` block is byte-identical to its pre-fix version while the code beneath it
was rewritten, and it is now false in three ways. It cites `singleNodeValue` for
`_one`, and `_one` was deleted in this commit (the only surviving
`singleNodeValue` in the crate is inside this comment). It says the XPath string
is constructed in JS via `JSON.stringify(needle)`, and it is now built in Rust by
`text_exact_xpath` at line 480. And it presents `then \" -> '` as the scheme that
keeps multi-quote needles safe, which is precisely the quote-swap this commit
removed for being broken, and which `xpath_string_literal`'s own rustdoc 100 lines
below explicitly refutes.

**Why it matters.** This is the file's orientation comment. It sits above every
builder in the Text section and it is what someone reads first. The next person
to touch text selectors reads a comment recommending the quote-swap as the safety
mechanism, then a function whose doc says the quote-swap was never an escaping
strategy. One of them is wrong and the comment is the one with seniority, because
it looks original and considered. Concretely: someone adds a fifth selector kind
that needs an XPath literal, follows the section comment because it is the local
convention, writes `JSON.stringify(n).replace(/"/g,"'")`, and re-lands
`text_exact("it's")` giving a `SyntaxError` in a new place. The whole point of
putting the module-header note at the point of temptation was to stop the next
author repeating a deleted mistake; this comment sits closer to the code and says
the opposite.

**Fix.** Rewrite the block to match the code: exact is an XPath built in Rust by
`text_exact_xpath`, quoting handled by `xpath_string_literal` with a `concat()`
fallback for mixed quotes, evaluated with `ORDERED_NODE_SNAPSHOT_TYPE`;
non-exact is the JS tree walk with narrowing. Drop every `_one` reference.
Replace the escaping sentence with a pointer to `xpath_string_literal` rather
than restating the rule, so there is one place it can go stale. While in the
file, `eval_expr_in_scope`'s rustdoc at
[selectors.rs:190](crates/zendriver/src/query/selectors.rs:190) has the same
leftover ("both `_one` and `_many`") and should say every match arm routes
through it.

#### R2-158-3. Element-scoped `text_exact` searches the whole document

[selectors.rs:481](crates/zendriver/src/query/selectors.rs:481) · **medium** ·
correctness · pre-existing, cemented by round 1

**What.** `text_exact_xpath` emits `//*[normalize-space(.)=…]`, and that one
string is now shared by the tab-scope builder (context node `document`) and the
element-scope builder (context node `this`). A leading `//` is
`/descendant-or-self::node()/`, and the leading `/` means the root of the
document containing the context node, so the context node is ignored entirely.
The substring path immediately above gets this right by walking
`this.querySelectorAll('*')`, so the two text paths disagree about whether
element scoping means anything.

**Why it matters.** Verified in Chrome 151, not read. With
`<div id=card1><button id=b1>Cancel</button></div><div id=card2><button
id=b2>Cancel</button></div>` and context node `#card2`, evaluating
`//*[normalize-space(.)='Cancel']` returns `[DIV#card1, BUTTON#b1, DIV#card2,
BUTTON#b2]`, while `.//*[…]` returns `[BUTTON#b2]`. Concretely: a scraper does
`let card = tab.find().css("#order-1834").one().await?` then
`card.find().text_exact("Cancel").one().await?` to get that order's cancel
button. The query enumerates every `Cancel` element on the page, and because
`best_match` sorts by `|len(text) - len(needle)|` and all four candidates fold to
"Cancel", `.one()` returns `DIV#card1`, a foreign div rather than even a button.
Nothing errors; the click lands on the wrong row. The same query written as
`card.find().text("Cancel")` *is* correctly scoped, so the two APIs a user reads
as "exact versus loose" differ in scoping too, which is documented nowhere. The
bug predates the PR, but the extraction is what makes it worth raising now:
there used to be two builders where the element one could be fixed
independently, and there is now one shared string plus a comment at
[selectors.rs:501](crates/zendriver/src/query/selectors.rs:501) ("Element scope:
`this` is the context node") that is literally true and practically misleading.

**Fix.** Give `text_exact_xpath` an axis parameter, or add a sibling: tab scope
keeps `//*`, element scope emits `.//*`. Correct the comment at line 501 to say
the expression is relative *because* it is evaluated against `this`. Cover it
with a real-Chrome case next to
`text_exact_matches_needles_carrying_either_quote_kind`: two sibling cards each
containing a button reading `Cancel`, scope the query to the second card, assert
the returned element's id. That test fails today.

#### R2-158-4. An unreachable guard with an impossible worked example

[selectors.rs:470](crates/zendriver/src/query/selectors.rs:470) · **low** ·
dead-code · introduced by round 1

**What.** The `if parts.len() < 2 { parts.push("\"\"") }` guard in
`xpath_string_literal` cannot execute. Control reaches it only when `s` contains
both quote kinds, so `s.split('"')` yields at least two chunks, the loop pushes at
least one `'"'` separator, and the chunk holding the apostrophe is non-empty so at
least one quoted chunk is pushed as well. The comment's example is impossible for
a second reason: a needle of exactly `"` contains no apostrophe and returns
early, never reaching the `concat()` branch.

**Why it matters.** A defensive branch with a worked example beside it is the
most convincing kind of code there is, and this one cannot run. The next reader
trying to understand `xpath_string_literal` spends time constructing the input the
comment promises exists, fails, and is left unsure whether they misread the
function or found a bug. It also inflates the apparent complexity of a function
whose real logic is three clean cases.

**Fix.** Delete the guard and the comment. If the `concat()` arity rule is worth
recording, state it once in the function's rustdoc as the reason the split always
yields two or more parts. If a belt-and-braces check is wanted, make it
`debug_assert!(parts.len() >= 2)`, which documents the invariant without
pretending to handle an input that cannot occur.

#### R2-158-5. A JS parameter that is never read, filled by an argument still shipped

[selectors.rs:510](crates/zendriver/src/query/selectors.rs:510) · **low** ·
dead-code · introduced by round 1

**What.** `build_text_exact_xpath_fn_body` now bakes the XPath into the
declaration, so its `_n` parameter is unused, but the caller at
`resolve_text_many` still sends `"arguments": [{ "value": needle }]` on every
element-scoped `text_exact`. The needle is serialised, shipped inside the
`Runtime.callFunctionOn` payload, and discarded.

**Why it matters.** The comment is honest about it ("The leading parameter
absorbs the needle the caller still passes"), which is better than hiding it, but
an underscore-prefixed JS parameter is a Rust idiom that means nothing in
JavaScript, so a reader who knows JS will assume `_n` is used somewhere. It is
also a maintenance trap: someone tidying the JS deletes the unused parameter
without noticing the Rust side still passes an argument, `function(){}` silently
ignores it, nothing breaks, and the confusion persists. The sibling substring
path genuinely reads its `n`, so the two are asymmetric for a reason that is not
visible from the call site.

**Fix.** Drop the parameter (`function(){…}`) and drop `"arguments"` from the
`Runtime.callFunctionOn` params in the exact arm of `resolve_text_many`. The
substring arm keeps both.

#### R2-158-6. The `text_equals` CHANGELOG entry names only the widening half

[CHANGELOG.md:26](crates/zendriver/CHANGELOG.md:26) · **low** · docs ·
introduced by round 1

**What.** The entry describes the `&nbsp;` half of the change and files it under
`### Fixed`. The commit also changed `text_equals` from `TXT.trim()===needle` to a
full `normalize-space` fold on both operands, which collapses interior whitespace
runs and normalizes the needle at compile time. That is a semantic change to a
public selector, and one direction of it removes matches.

**Why it matters.** Most of the change widens matching, which is harmless:
`text_equals("Hello world")` now also matches `<b>Hello   world</b>`. But the
`&nbsp;` fix narrows it and the entry does not say so. Concretely: a user has
`text_equals("Continue")` matching `<button>&nbsp;Continue</button>`. The old
`trim()` stripped U+00A0 because JS `WhiteSpace` includes it, so it matched; the
new edge strip removes only a literal space, so `"\u{a0}Continue" != "Continue"`
and it does not. Their scraper stops finding the button after a patch upgrade,
and the changelog line they read to diagnose it describes the removal of a
*disagreement*, which reads like a consistency improvement rather than something
that can drop a match they depended on. The screenshot entry directly above gets
this right by stating the behaviour change and its consequence, so the standard is
already set in the same file.

**Fix.** Extend the bullet to name both directions: interior runs now collapse on
both sides, so a needle typed with extra spaces matches where it previously could
not; and edge `&nbsp;` / `U+FEFF` are now preserved rather than trimmed, so
`text_equals("OK")` no longer matches `<b>&nbsp;OK</b>` and the caller should use
`text("OK")` or include the character.

### #155 — core I/O

#### R2-155-1. No test can detect the PR's headline change being reverted

[persistence.rs:449](crates/zendriver/src/cookies/persistence.rs:449) ·
**medium** · weak-test · pre-existing (shipped in commit 1)

**What.** Replacing `write_atomic(path.as_ref(), &bytes)` with `fs::write(path,
bytes)` in both `save_to_file` and `save_to_file_matching` leaves all six
`cookies::persistence` tests green. `both_save_paths_write_atomically` asserts
only that the destination holds the right JSON and that the directory has one
entry, and a plain `fs::write` satisfies both.
`save_to_file_preserves_a_restrictive_mode` asserts a 0600 jar is still 0600 after
a save, which `fs::write` also satisfies because every save-path test writes to a
destination that already exists, and `O_WRONLY|O_CREAT|O_TRUNC` on an existing
inode ignores the mode argument. Both doc comments claim more than their bodies
check: "Both save paths must go through the atomic helper" is not observable from
either assertion, and "the regression as reported, end to end through the public
API" names the mid-write temp-file window, which a destination-only assertion
cannot see. `io.rs`'s own `temp_file_is_owner_only_for_the_whole_write` says
exactly this about itself, three files away. The one integration test that saves
also uses `NamedTempFile` and asserts round-trip contents only.

**Why it matters.** `crate::io`'s tests prove the helper is correct; nothing
proves the cookie jar calls it. That is the gap a refactor walks through.
Concretely: six months from now a maintainer reads `save_to_file`, sees `bytes`
already in hand and a one-line `write_atomic` call, and "simplifies" it back to
`fs::write` while resolving a merge conflict. CI is green, review sees a smaller
diff, the change ships. The next `kill -9` during a save leaves a truncated
`cookies.json` that fails to parse on the next `load_from_file`, losing the
authenticated session the file exists to preserve, and freshly created jars go
back to 0644 on a default-umask box, which is the exposure this round was
convened to close. A suite that stays green through that is worse than no suite,
because the name `both_save_paths_write_atomically` tells the reviewer the
property is covered. Severity is medium rather than high because the shipped code
is correct today and the harm requires a future regression.

**Fix.** Give one of the two tests an assertion only `write_atomic` can pass.
Cheapest and most on-point: save to a path that does not exist yet and assert the
mode, since `write_atomic` creates 0600 and `fs::write` creates `0666 & ~umask`.
Add a umask-independent discriminator as well: `save_to_file` onto a symlink must
leave a regular file at the path
(`assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink())`), because
`fs::write` follows the link. Both shapes are already proven to work by
`io.rs`'s `write_atomic_creates_a_new_file_owner_only` and
`write_atomic_replaces_a_symlinked_destination`. Then reword the two doc comments
to describe what the bodies actually check.

#### R2-155-2. A stray non-UTF-8 byte in a third-party blocklist fails `Browser::launch()`

[tracker.rs:135](crates/zendriver/src/tracker.rs:135) · **medium** ·
correctness · introduced by round 1

**What.** The chunk-accumulation rewrite ends in `String::from_utf8(body)`, which
returns `InvalidData` on a single malformed byte. The `Response::text()` it
replaced never fails on encoding: reqwest is pulled here without the `charset`
feature, so `text()` is `String::from_utf8_lossy(&full).into_owned()`. The size
fix quietly changed the decode contract as well, and no test covers it. The
tracker tests cover the timeout, the Content-Length cap and the streaming cap
only.

**Why it matters.** `load_or_download_blocklist` runs inside `Browser::launch()`
(called once at launch from `build_tracker_matcher`, `?`-propagated to
`ZendriverError::Io`), so a cosmetic byte becomes a hard launch failure.
Concretely: a user points `tracker_blocklist_url` at a mirror whose header
comment reads `# Peter Lowe's list` served as Windows-1252, or carries a latin-1
`©` in a licence line. Those bytes are in comment lines `parse_blocklist` throws
away. Before this commit the list loaded fine with a replacement character nobody
saw. Now `Browser::launch()` returns `Io(InvalidData)`, and because the failure
happens before `write_atomic`, nothing is cached, so every subsequent launch
fails identically. The error message points at UTF-8, not at a comment line the
user could delete.

**Fix.** `String::from_utf8_lossy(&body).into_owned()`. Host lines are ASCII, so
a mangled comment is harmless and this restores the pre-commit behaviour. If a
hard failure is genuinely preferred it needs to be a deliberate, documented
decision: say so in `tracker_blocklist_url`'s rustdoc alongside the three bounds
already listed there, and add a test that feeds `download_blocklist` a latin-1
body. See §7 item 15.

#### R2-155-3. The monitor warns on ordinary browser close

[monitor/mod.rs:1056](crates/zendriver/src/monitor/mod.rs:1056) · **low** ·
correctness · **not** a round-1 regression

**What.** The third branch warns on any error that is not an RPC `-32601` or
`-32602`. `code` is extracted as `match &e { CallError::Rpc(code, ..) =>
Some(*code), _ => None }`, so `CallError::Transport(_)` and `CallError::Timeout`
both reach the warn. Connection teardown resolves in-flight calls to
`Transport(Shutdown | Disconnected)`, so a normal browser close, or a
`Connection::reconnect`, now emits `WARN network monitor:
Network.streamResourceContent failed unexpectedly ... if this is persistent every
request pays a failed call`.

**Why it matters.** A warning is a claim that something is wrong, and this one
fires at the most ordinary end of a session, with a trailing clause that is false
there because the monitor no longer exists to pay anything per request. This repo
has already been burned by a canary whose signal was drowned. The reason this is
low rather than a regression: pre-PR, the same shutdown error already warned,
additionally claimed the cause was "needs Chrome ~124+", and latched the flag that
disables streaming for the rest of the monitor's life. Round 1 improved the path;
what is left is message precision.

**Fix.** Match on the error rather than on an extracted code:
`Rpc(c) if *c == JSON_RPC_METHOD_NOT_FOUND` latches and warns; `Rpc(c) if *c ==
JSON_RPC_INVALID_PARAMS` goes to debug; any other `Rpc(..)` keeps the once-only
warn, which is the persistent-failure case round 1 wanted visible;
`Transport(_) | Timeout { .. }` goes to debug, because the session is going away
and there is no next request to spend a round-trip on. Add a test that shuts the
connection down with an enable call in flight and asserts the task exits without
latching either flag.

#### R2-155-4. An assert-absence behind a fixed sleep, with a comment naming the wrong hazard

[monitor/mod.rs:1650](crates/zendriver/src/monitor/mod.rs:1650) · **low** ·
weak-test · introduced by round 1

**What.** `emit_request_and_settle`'s doc says the sleep matters in one direction
only, so that a build latching on any error would still be holding an unset flag
when the next request is processed. That hazard is already covered by each
caller's own inline `sleep(100ms)` placed after `reply_err` and before the helper
is called. The helper's sleep sits *after* r2 is emitted, so what it actually
guards is that the correlator has processed r2 and its spawned task has put the
CDP command on the wire. That only matters for the negative test, where
`mock.try_recv_cmd().is_none()` passes if nothing has happened *yet*, not only if
nothing will.

**Why it matters.** This is the false-green shape. An assert-absence behind a
fixed sleep degrades silently on a loaded CI runner: it does not flake red, it
passes for the wrong reason. The test is non-vacuous today, since deleting the
`!warned_stream_unsupported.load(..)` guard makes it fail, but that is a
statement about one laptop, not about a 16-way-parallel runner. The comment makes
it worse than a bare sleep would be, because the next maintainer wondering
whether 100ms is enough reads a rationale about the *previous* barrier, concludes
the question is settled, and moves on. The positive test does not have this
problem: its `expect_cmd` is already wrapped in a 2s timeout, which is a proper
barrier.

**Fix.** Correct the comment to name the hazard the sleep actually covers, and
replace the blind wait in the negative test with an observable barrier: after
emitting r2, drive `monitor.next()` until r2's `RequestStarted` arrives, which is
proof the correlator ran the guard for r2, and only then assert
`mock.try_recv_cmd().is_none()`. Keep a short settle after it for the spawned
task and say in the comment that this is what the remaining slack is for.

#### R2-155-5. The save methods' `# Errors` sections describe the old write

[persistence.rs:51](crates/zendriver/src/cookies/persistence.rs:51) · **low** ·
docs · pre-existing

**What.** The `# Errors` section on `save_to_file` and on
[persistence.rs:106](crates/zendriver/src/cookies/persistence.rs:106) still reads
"Returns `ZendriverError::Io` if the path is unwritable", which was accurate for
`fs::write`. The write now creates a sibling temp file with `create_new` and
renames it, so it additionally requires write and execute permission on the
containing directory, and the closing `rename` can fail on destinations a direct
write handled. The rustdoc goes into real detail on symlinks and Windows ACLs
while leaving the two most likely Unix failures unmentioned.

**Why it matters.** A user cannot recognise their own failure from the documented
one. The common case is Docker: `docker run -v $PWD/cookies.json:/app/cookies.json`
bind-mounts a single file, the temp sibling is created without trouble, and the
`rename` over the bind mount fails with `EBUSY` because the destination is itself
a mount point. So `save_to_file` now errors on a path that is manifestly
writable, and the docs say the path must be unwritable. Same shape for a jar in a
directory the process may read but not write. Both worked before this PR. The
nearest existing prose lives on `crate::io::write_atomic`, which is `pub(crate)`,
so none of it renders for a user reading `CookieJar` on docs.rs.

**Fix.** One sentence in `# Errors` on both methods: the save fills a sibling temp
file and renames it over the destination, so it needs a writable parent directory
and fails on a destination that cannot be replaced by a rename, such as a
bind-mounted file or a destination held open on Windows.

### #157 — input and gate

#### R2-157-1. The module doc claims live-Chrome coverage the fixture does not provide

[actionability.rs:22](crates/zendriver/src/query/actionability.rs:22) · **low** ·
false-comment · introduced by round 1

**What.** The module doc added by the round-1 fix ends: "Behavioral coverage lives
in the live-Chrome tier (`tests/find_visible_only.rs`, gated on the
`integration-tests` feature)." The PR does not touch that file, its five tests
cover `display:none` filtering plus three `include_frames` combinations, and
`grep -rn "opacity\|compatMode\|BackCompat\|content-visibility" crates/zendriver/tests/`
returns nothing.

**Why it matters.** The claim is not that coverage is zero. `find_visible_only.rs`
does drive real headless Chrome through `visible_only(true)`, which executes
`check_visible`, and CI runs it. What is missing is per-clause coverage of the
opacity, viewport and quirks-mode arms, and `check_stable`, `check_enabled` and
`check_receives_pointer` have no live fixture at all. The sentence is broader
than what it points at, and its shape is the expensive one: it tells a reader the
risk was retired. A reader planning to widen the viewport clause from `rect.right
<= 0` to `rect.right <= 1` for sub-pixel tolerance will look for the fixture that
would catch a mistake and conclude it exists. Round 1's #157-7 asked for exactly
that fixture (a below-the-fold div, an `opacity: 0.001` honeypot, a `visibility:
hidden` ancestor, a `position: fixed` element, a doctype-less quirks page) and the
fix wrote the sentence pointing at it without writing it. One correction to the
original finding: its own worked example is self-refuting, because
`probe_source_tests_viewport_intersection` asserts `js.contains("rect.right <=
0")`, so that specific mutation does turn a unit test red.

**Fix.** Either add the fixture, roughly 30 lines of HTML behind the existing
`integration-tests` gate, or narrow the sentence to the truth: `check_visible`'s
`display:none` path is exercised live by `tests/find_visible_only.rs`; the
opacity, viewport and quirks-mode clauses and the other three gate predicates are
pinned only by source-text assertions. Do not leave a sentence that reads as a
coverage guarantee at a granularity the tier does not reach.

#### R2-157-2. Modifier suppression covers two of the three text-carrying paths

[keyboard.rs:522](crates/zendriver/src/input/keyboard.rs:522) · **low** ·
correctness · introduced by round 1 (as an asymmetry, not a regression)

**What.** The suppression was applied to the descriptor-backed `Key::Char` path
and to the `Key::Special` path, but not to the `char`-event fallback ten lines
above. `key_events(Key::Char(c), mods, _)` where `char_descriptor(c)` is `None`,
which is every non-ASCII character, still builds `text: Some(s)` with no
reference to `mods`. `cluster_events`' multi-codepoint branch at
[keyboard.rs:689](crates/zendriver/src/input/keyboard.rs:689) does the same. Both
are reachable from the public API via `Element::press_with` and via `type_text`
under a held modifier.

**Why it matters.** `press_with(Key::Char('a'), CTRL)` correctly sends no text,
while `press_with(Key::Char('é'), CTRL)` still sends `text: "é"` on a `char`
event, and a CDP `char` event is a text insertion, so Chrome types "é" into the
focused field. A user writing a shortcut test for a French or CJK keymap gets a
stray character and no explanation, because the ASCII case they debugged against
behaves the other way. Note two corrections to the original finding: no path
regressed, since pre-fix every path sent text, and the `SpecialKey` rustdoc that
states the rule is explicitly scoped to Space and Enter ("for both"), so it does
not make the exception undiscoverable.

**Fix.** Apply the same predicate at both `char`-event construction sites:
`text: (!modifiers_suppress_text(mods)).then(|| s.clone())` in `key_events`'
no-descriptor branch and in `cluster_events`' multi-codepoint fallback. Add one
test alongside `char_with_a_non_shift_modifier_sends_no_text` covering
`Key::Char('é')` with `CTRL`, so the three paths are pinned together.

#### R2-157-3. The Puppeteer-parity justification describes a deliberate divergence

[keyboard.rs:490](crates/zendriver/src/input/keyboard.rs:490) · **low** ·
false-comment · introduced by round 1

**What.** `modifiers_suppress_text`'s doc says dropping the field "matches what
Puppeteer puts on the wire, which is the shape anything fingerprinting the CDP
stream is calibrated against". Checked against `packages/puppeteer-core/src/cdp/
Input.ts`: Puppeteer blanks the text (`if (this._modifiers & ~8) {
description.text = ''; }`) and then derives the event type from it (`type: text ?
'keyDown' : 'rawKeyDown'`). An empty string is falsy, so Puppeteer emits Ctrl+Enter
as a `rawKeyDown` with no text, while zendriver now emits a `keyDown` with no
text, a frame Puppeteer never produces.

**Why it matters.** The comment's stated justification is wire-shape parity with
the reference implementation, and the decision it justifies diverges from that
implementation on the event type. The divergence itself is defensible and is
argued correctly in the very next paragraph of the same doc, which records that
`rawKeyDown` on a printable key cost ~1.1s on first dispatch and killed the
browser process on the second. So the antecedent of "matches what Puppeteer puts
on the wire" is the field, and Puppeteer does drop the field; only the trailing
clause about calibrated fingerprinting over-reaches. Left as-is, a reader
skimming the first sentence will tell a user the two emit identical Ctrl+Enter
frames.

**Fix.** Tighten the third sentence: Puppeteer blanks the same field, but derives
its event type from it, so Puppeteer's Ctrl+Enter is a `rawKeyDown`; the
divergence is deliberate and the cost of matching it is recorded below. Do not
add a stealth claim in either direction, since neither is supported. Then record
the #157-9 call in the PR body, since round 1 reserved it (§7 item 11).

#### R2-157-4. `we_latched` also removed the sweep of a stranded bit

[mouse.rs:265](crates/zendriver/src/input/mouse.rs:265) · **low** · correctness ·
introduced by round 1

**What.** The `we_latched` guard closes #157-25 correctly, but the pre-fix
unconditional `buttons_held.remove(bit)` at exit also cleaned up a bit stranded
by a dropped gesture future. Now a `click_at` that finds the bit already set
records `we_latched = false`, and if its `mousePressed` fails the cleanup is
skipped.

**Why it matters.** Narrow: it needs a cancelled gesture followed by a click whose
press fails. The window is also smaller than first claimed, because a failure in
`move_realistic` or `move_raw` returns above the cleanup in both the old and new
shapes. And it is not permanent: the release path clears the bit unconditionally
before dispatching, so the next click that gets that far self-heals. Until then,
every `mouseMoved` this tab dispatches reports `buttons: 1` with no preceding
`mousedown`, which is the impossible stream the field doc says is worse than
omitting `buttons` entirely.

**Fix.** A comment, not code. Beside `we_latched`, which currently only argues the
concurrency side, add that a stranded bit (see the cancellation case on
`InputState::buttons_held`) is no longer swept by the next failed click, and that
the release path still clears it so it self-heals on the first click that gets
that far.

#### R2-157-5. The idle-latency bound lost its number to a private constant

[tab.rs:1813](crates/zendriver/src/tab.rs:1813) · **low** · docs · introduced by
round 1

**What.** The #157-23 fix replaced the literal "50ms" in `wait_for_idle_opts`'
public rustdoc with `[`IDLE_FALLBACK_TICK`]`, a private module const. `cargo doc
-p zendriver --no-deps` reports "public documentation for `wait_for_idle_opts`
links to private item `IDLE_FALLBACK_TICK`" at both
[tab.rs:1800](crates/zendriver/src/tab.rs:1800) and line 1813, and rustdoc renders
such links as plain text. The markdown is malformed as well, since
`` `quiet_window + `[`IDLE_FALLBACK_TICK`] `` closes a code span on a trailing
space, and the reflow left "the number of *active*" alone on its own line.

**Why it matters.** The doc is the only place a user learns the latency bound and
the number is now gone from it. On docs.rs the sentence renders as an unclickable
identifier for a constant the reader cannot open. Someone sizing a `quiet_window`
for a scraper previously read "quiet_window + 50ms" and budgeted; now they clone
the repo. The constant's own doc claims the public docs name it rather than
repeating the number, which is true and is the problem. The internal-audit half of
the fix is correct: only two 50ms prose sites remain and both are test-scenario
arithmetic. For context, private intra-doc links are house style here and the
same `cargo doc` run reports 12 of them; what is unique to this site is losing
the number.

**Fix.** Name both: "worst-case latency … is `quiet_window` plus one
`IDLE_FALLBACK_TICK` (50ms)", at both line 1800 and line 1813. That keeps the
coupling the constant's doc asks for and gives the reader the fact. Fix the
code-span seam and re-wrap the paragraph while there.

#### R2-157-6. `clear_by_deleting`'s rustdoc was corrected, its inline comment was not

[actions.rs:629](crates/zendriver/src/element/actions.rs:629) · **low** ·
stale-comment · introduced by round 1

**What.** The #157-14 fix corrected the rustdoc to say a field already emptied
costs 16 strokes and a probe rather than `len + slack`. The inline comment fifteen
lines below still says the loop "turns thousands of round-trips into one probe".
The code (`if i > 0 && i % PROBE_EVERY_N_BACKSPACES == 0`, with the constant at
16) never probes before stroke 16, so the rustdoc is right and the comment is
imprecise.

**Why it matters.** Same shape as #157-3, which this same commit fixed in
`screenshot.rs`: a corrected doc and an uncorrected comment about the same
behaviour, disagreeing within one screenful, and whichever a reader hits first is
the model they leave with. Concretely, someone tuning `PROBE_EVERY_N_BACKSPACES`
reads "one probe", assumes the cadence is near-optimal for the common case, and
does not notice the 16 mandatory `press` calls, each of which re-runs the full
focus gate (scroll, two predicates, `el.focus()`). That is 16 gate cycles on a
field the first Backspace already cleared.

**Fix.** Change "turns thousands of round-trips into one probe" to "caps thousands
of round-trips at `PROBE_EVERY_N_BACKSPACES` strokes plus one probe", matching the
wording the rustdoc already settled on.

### #150 — transport

#### R2-150-1. The comment closing 150-8 asserts two things a fifteen-line test disproves

[connection.rs:628](crates/zendriver-transport/src/connection.rs:628) · **low** ·
false-comment · introduced by round 1

**What.** The comment reads, verbatim, that the `socket_died` clear "is defensive
and currently unobservable, which is why no test pins it: `socket_died` has
exactly one reader, `actor_gone_error`". The second half is false.
`ConnectionInner`'s hand-rolled `Debug` does `.field("socket_died",
&self.socket_died)` at
[connection.rs:203](crates/zendriver-transport/src/connection.rs:203), a line this
very commit edited to add `swept_total` next to it, and `AtomicBool`'s `Debug`
loads the value. `Connection` is `#[derive(Debug)]`, re-exported from `zendriver`
and reachable via `Browser::cdp()`, so `{:?}` on a public handle surfaces the
flag without the actor being gone, which is exactly what the comment's closing
warning says must never be added. The first half is a wording problem rather than
an error: there is no *behavioural* observer, but `inner` and `socket_died` are
`pub(crate)`, so an in-crate test can load the flag directly, which is the
technique this same commit used one field over to make `swept_total` assertable.

**Why it matters.** The comment is not decoration; it is the recorded
justification for why this line has no test, and the next person to touch
reconnect will trust it and skip writing one. The test the comment says cannot
exist was written and it works: `duplex_pair()`, `spawn_actor(ws_a)`, one
`call_raw` in flight, `drop(tx_a)`, await the drained call,
`assert!(conn.inner.socket_died.load(Acquire))`, `conn.reconnect(ws_b)`,
`assert!(!conn.inner.socket_died.load(Acquire))`. It passes, and commenting out
the clear fails it with "reconnect must clear the latch". The "exactly one
reader" half is the worse of the two in the long run, because the invariant the
closing warning protects is already not what the reader will think it is.

**Fix.** Replace the two false sentences. Say that the clear has no *behavioural*
observer: `actor_gone_error` is the only reader that changes an outcome, and both
paths to it require the current actor to be gone, so removing this line cannot
change what any caller sees, which round 1's verifier confirmed empirically. Add
the state-level test above, which pins the line honestly in fifteen lines. Keep
the closing warning but re-ground it: note that `ConnectionInner::Debug` already
reads the flag without the actor being gone, so `Debug` output is best-effort, and
that a behavioural accessor such as `Connection::is_disconnected()` would be the
thing that breaks the argument.

#### R2-150-2. The one "reply budget" the sweep missed is the summary line

[connection.rs:285](crates/zendriver-transport/src/connection.rs:285) · **low** ·
stale-doc · introduced by round 1

**What.** 150-6 asked for five sites. `DEFAULT_CALL_TIMEOUT`,
`ConnectionInner::call_timeout_ms`, `Connection::call_timeout` and
`Connection::set_call_timeout` were all swept. `call_raw_with_timeout`'s own
summary line was not: it still reads ``/// [`Connection::call_raw`] with an
explicit reply budget.``, eight lines above the paragraph in the same doc comment
that says the budget covers the whole call including the enqueue. `grep -rn
'reply budget' crates/` returns exactly that one hit.

**Why it matters.** This is the rustdoc *summary* line, so it is what appears in
the method list on the `Connection` page, in search results, and in IDE
completion popups, and it is the only sentence many readers see. It is also the
specific method the four freshly corrected docs redirect people to, since
`set_call_timeout` now says to prefer `call_raw_with_timeout` when only one call
is unusual. A user follows that link precisely to learn what the budget bounds
and the first line says "reply". The realistic damage is bounded, because the
paragraph eight lines below corrects it on the same page, which is why this is
low rather than medium.

**Fix.** Change line 285 to ``/// [`Connection::call_raw`] with an explicit
per-call budget.``, matching the four siblings, then re-run `grep -rn 'reply
budget' crates/` and confirm it returns nothing.

#### R2-150-3. `Timeout`'s variant docs were rewritten; its field docs were not

[error.rs:80](crates/zendriver-transport/src/error.rs:80) · **low** · stale-doc ·
introduced by round 1

**What.** The `CallError::Timeout` variant doc was correctly rewritten to say the
command may never have reached the socket and that the variant does not
distinguish the two routes. Its two field docs three lines below were not touched:
`method` is "The CDP method that was never answered" and `budget` is "The budget
that elapsed without a reply". The identical pair survives on the public
`ZendriverError::CdpTimeout` at
[error.rs:116](crates/zendriver/src/error.rs:116).

**Why it matters.** These are public struct-variant fields, so both strings render
on docs.rs directly beneath the corrected variant doc. The verifier's correction
matters here: both field docs are literally *true* on both routes, because on the
enqueue route the command was in fact never answered and no reply ever elapsed.
They are weakly worded, implying Chrome was asked, rather than contradicting the
prose above them, and the new `Display` string ("CDP call `Page.navigate` did not
complete within 180s") is honest. So this is a consistency edit, not the
wrong-diagnosis hazard it was first filed as: a junior reading only the field doc
could still conclude Chrome received the command and go dump renderer state on a
browser that never saw it, but they would have to ignore the two corrected
sentences directly above.

**Fix.** In both crates, change the `method` field doc to "The CDP method whose
call ran out its budget (e.g. `\"Page.navigate\"`)" and the `budget` field doc to
"The budget that elapsed without the call completing". Four one-line edits, and
the two docs.rs pages become self-consistent.

#### R2-150-4. A public doc traded the queue depth for a private constant's name

[connection.rs:295](crates/zendriver-transport/src/connection.rs:295) · **low** ·
docs · introduced by round 1

**What.** Closing the DRY finding 150-4 replaced "the actor's command channel is
64 deep" in `call_raw_with_timeout`'s public rustdoc with "the actor's command
channel is `CMD_CHANNEL_CAPACITY` deep". That constant is `pub(crate)` in
`actor.rs`, so it does not exist on docs.rs, and it is written as a plain code
span rather than an intra-doc link, which is why `cargo doc` stays quiet. The same
commit made the identical substitution in
[connection.rs:58](crates/zendriver-transport/src/connection.rs:58), inside the
doc of the public `DEFAULT_CALL_TIMEOUT`.

**Why it matters.** The DRY finding was about the number being written three times
in *source*; it was never about the public doc, which is the one place restating
the value costs nothing and helps. A user on docs.rs now reads a symbol name,
searches for it, finds nothing, and has lost the one concrete fact the sentence
carried: how deep the queue is, which is what tells them whether their 200-call
burst can plausibly block. The DRY guarantee is unaffected either way, because it
comes from the two `mpsc::channel` call sites sharing the constant. Linking the
constant, as the original finding suggested, is not available for a private item
without tripping `rustdoc::private_intra_doc_links`.

**Fix.** Restore the number and keep the name as context, at **both** sites: "the
actor's command channel is 64 deep (`CMD_CHANNEL_CAPACITY`)".

#### R2-150-5. Four more copies of `duplex_pair` remain in the module downstream builds

[testing.rs:51](crates/zendriver-transport/src/testing.rs:51) · **low** · dry ·
pre-existing

**What.** The fix consolidated the three copies of `duplex_pair` from
`actor::tests`, `connection::tests` and `session::tests` into `connection::
test_only`, gated `#[cfg(test)]`. Four verbatim copies of the same construction
remain in `testing.rs` at lines 50, 72, 106 and 240, each building the same pair
of `mpsc` channels and stuffing them into a `DriverStream`. The `#[cfg(test)]`
gate is what stops them being collapsed, since `testing.rs` compiles under
`cfg(any(test, feature = "testing"))` and cannot call a `cfg(test)`-only helper
without breaking the `--features testing` build for downstream consumers.

**Why it matters.** Correcting the original filing: this is **not** a round-1
regression. The pre-fix `duplex_pair` also lived inside `#[cfg(test)] mod tests`
and was equally unreachable from `testing.rs`, so round 1 took the construction
from 7 copies to 5. The residual cost is real though: the four that remain are in
the module downstream crates actually build against, so a future change to the
plumbing, such as making the sink async rather than `try_send`-based, has to be
applied in five places instead of one.

**Fix.** Widen the two `#[cfg(test)]` attributes on `duplex_pair` /
`duplex_pair_with_capacity` to `#[cfg(any(test, feature = "testing"))]` and
rewrite the four blocks to call the helper. **Do not do this literally**:
`duplex_pair_with_capacity(n)` hardcodes the inbound channel at 32 while all four
`testing.rs` sites use 64/64, so a naive `duplex_pair_with_capacity(64)` silently
shrinks the inbound queue for downstream `--features testing` consumers. Give the
helper both capacities, or add a 64/64 variant. Confirm with `cargo clippy -p
zendriver-transport --features testing --all-targets -- -D warnings` that no
`dead_code` warning appears in the non-test build.

---

## 5. Fix order

One ordered list across the four PRs. Items in the same block are independent.
#158 is stacked on #157, so #157's fixes land first in that branch and #158
rebases.

1. **R2-158-1** Delete the `position: fixed` caveat from the module doc, the
   `clip` rustdoc, `element/screenshot.rs`, and the CHANGELOG; move the CHANGELOG
   entry from `### Changed` to `### Fixed`. First because it is the only finding
   that ships a false claim to crates.io and docs.rs.
2. **R2-158-3** Make the element-scope XPath relative (`.//*`) and add the
   two-sibling-cards real-Chrome test. The only live user-facing defect in the
   set. Sequence before item 5, same file.
3. **R2-155-2** `String::from_utf8_lossy` in `download_blocklist`, or the
   documented fail-closed decision from §7 item 15 plus a latin-1 test. Second
   live defect, and it fails `Browser::launch()` unrecoverably.
4. **R2-155-1** Add a discriminating assertion to the cookie save tests (fresh
   path plus mode, and the symlink shape) and reword the two overclaiming doc
   comments. Highest-value test in the set: it is the only thing that would stop
   the PR's headline behaviour being refactored away.
5. **R2-158-2** Rewrite the Text section block comment to match the code, drop
   every `_one` reference, point at `xpath_string_literal` instead of restating
   the escaping rule, and fix `eval_expr_in_scope`'s rustdoc at line 190.
6. **R2-157-1** Narrow the `actionability.rs` module doc to the coverage that
   exists, or add the fixture. Merge blocker for #157.
7. **R2-157-2** Apply `modifiers_suppress_text` at both `char`-event sites and add
   the non-ASCII test.
8. **R2-150-2** Fix the `call_raw_with_timeout` summary line, then `grep -rn
   'reply budget' crates/` and confirm it is empty.
9. **R2-150-1** Rewrite the `socket_died` comment to claim only "no behavioural
   observer", and add the fifteen-line reconnect-clears-the-latch test.
10. **R2-150-4 + R2-157-5** Restore the concrete numbers next to the constant
    names in all three public docs (`connection.rs:58`, `connection.rs:295`,
    `tab.rs:1800` and `1813`), and fix the code-span seam and the reflow orphan in
    `tab.rs`. One unit of work; they are the same mistake.
11. **R2-150-3** Rewrite the `method` and `budget` field docs in both crates.
12. **R2-155-5** Add the temp-file-and-rename sentence to both `# Errors`
    sections.
13. **R2-155-3** Route `Transport(_) | Timeout { .. }` to `debug!` and keep the
    once-only warn for other `Rpc` codes; add the shutdown-with-call-in-flight
    test.
14. **R2-155-4** Correct `emit_request_and_settle`'s comment and replace the blind
    wait in the negative test with an observable barrier.
15. **R2-157-3** Tighten the Puppeteer sentence to say what diverges and why.
16. **R2-157-4** Add the trade note beside `we_latched`.
17. **R2-157-6** Reword the `clear_by_deleting` inline comment to match its
    rustdoc.
18. **R2-158-4 + R2-158-5** Delete the unreachable `parts.len() < 2` guard and its
    comment; drop the unused `_n` parameter and the `"arguments"` array in the
    exact arm.
19. **R2-158-6** Extend the `text_equals` CHANGELOG bullet to name both
    directions.
20. **R2-150-5** Collapse the four `testing.rs` copies onto a widened helper, with
    the 64/64 capacity preserved explicitly.

Items 1 through 4 are the merge blockers. Items 5 through 20 are polish and can be
batched, but every one of them must end with the grep or the adjacent-comment
re-read described in §1, or round 3 will look like round 2.

---

## 6. Struck

Nothing. All 22 reviewer findings survived adversarial verification. Five were
**overstated** and were corrected rather than dropped; the corrections are
recorded inline in §4 and are the version to fix:

- **R2-150-3** (`Timeout` field docs) is a consistency edit, not a
  contradiction. Both field docs are literally true on both routes.
- **R2-150-5** (`duplex_pair` copies) is pre-existing cleanup. Round 1 went from
  7 copies to 5; it did not add any. The proposed one-line fix would also shrink
  a downstream inbound queue from 64 to 32.
- **R2-155-3** (monitor warn) is not a round-1 regression. The pre-PR code warned
  on the same error, misattributed it to Chrome 124, and permanently disabled
  streaming. Round 1 improved it.
- **R2-157-1** (actionability coverage claim) is not "coverage is zero".
  `find_visible_only.rs` does execute `check_visible` against live Chrome in CI;
  the gap is per-clause. The original finding's worked example is also
  self-refuting, since `probe_source_tests_viewport_intersection` catches it.
- **R2-157-4** (`we_latched`) does not strand a bit for the tab's life. The
  release path clears it unconditionally, so the next successful click
  self-heals. Comment only.

Do not re-raise round 1's struck finding either: #152's
`wait_for_clearance_returns_challenge_gone_when_iframe_disappears_after_click`
picks a defensible reading of an ambiguous payload and the proposed edit was a
coverage loss.

---

## 7. Still needs a human

Round 1 routed 12 findings to a human and wrote them up as 13 judgement calls in
its §5. Round 2 found no evidence that any of them were resolved, so **all
thirteen carry forward unchanged**. They are not restated here; see
[founders-review-round1.md §5](docs/api-audit/founders-review-round1.md). Three
of them were touched by round-1 fixes or round-2 evidence and their status
changed:

**1. `visible_only` semantics (round 1 §5 item 1).** Unchanged and still the
gating decision for the #157/#158 doc chain, but now with a consequence: #157 is
otherwise mergeable and will ship `visible_only` and `is_visible` with rustdoc
that omits the viewport requirement until this is answered. It is a blocker for
the series, not for the commit.

**11. Puppeteer parity on modifier+text (round 1 §5 item 11).** Round 1 reserved
this and the fix implemented it anyway. It now needs **ratification rather than a
decision**, and the trade-off the human was meant to weigh has changed shape:
zendriver now emits Ctrl+Enter as `keyDown` with no text, which is neither
Puppeteer's frame (`rawKeyDown`, no text) nor the pre-fix frame (`keyDown` with
text). The engineering reason for the divergence is sound and recorded. Confirm
it in the PR body, and decide explicitly whether the divergence is acceptable for
a stealth crate.

**12. `captureBeyondViewport` on on-screen clips (round 1 §5 item 12).**
**Dissolved by measurement.** Round 1 asked whether a deliberate change to
existing callers' output was acceptable and required a changelog entry. Chrome
151 says there is no change to existing callers' output: an on-screen clip is
byte-identical with and without the flag. There is nothing left to approve, and
the CHANGELOG entry written under this item's instruction is the false claim in
R2-158-1. Withdraw the item along with the entry.

Two new items:

**14. `check_visible`'s live-Chrome fixture (extends round 1 §5 item 6).** Round
1's item 6 framed the live-Chrome tier as a resourcing question. R2-157-1 narrows
it to one concrete ask: roughly 30 lines of HTML in an existing gated test file
covering the below-the-fold, opacity-honeypot, hidden-ancestor, fixed-position
and quirks-mode cases, plus a fixture for `check_stable`, `check_enabled` and
`check_receives_pointer`, which have no live coverage at all. Approving the
fixture is cheaper than approving a tier. Until it exists, the module doc must
not claim the coverage.

**15. Blocklist decoding policy (new, from R2-155-2).** Should a third-party host
list with a non-UTF-8 byte fail `Browser::launch()`, or load lossily? The
pre-existing behaviour was lossy and a round-1 fix silently made it fail-closed
without anyone choosing that. Lossy is recommended, since host lines are ASCII
and the offending bytes are in comments the parser discards, and fail-closed on a
remote third-party resource inside `launch()` with no cache write is the worst
combination available. Either way the answer belongs in `tracker_blocklist_url`'s
rustdoc next to the three bounds already documented there, and the fix in item 3
of §5 depends on it.
