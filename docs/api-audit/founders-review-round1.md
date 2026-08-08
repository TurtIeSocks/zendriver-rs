# Founder's review, round 1 — PRs #150, #152, #155, #156, #157, #158

Internal engineering document. Six stacked PRs were reviewed line by line, every
finding was then re-checked by an adversarial verifier that reproduced or refuted
the claim. 108 findings survived, 1 was struck.

Severity tally: **7 high, 26 medium, 75 low.**

| PR | Scope | Lines | High | Medium | Low | Merge as-is? |
|----|-------|-------|------|--------|-----|--------------|
| [#150](#pr-150--transport) | transport | 601 | 0 | 4 | 12 | No |
| [#152](#pr-152--solvers) | solvers (cloudflare / imperva / datadome) | 1637 | 1 | 5 | 11 | No |
| [#155](#pr-155--core-io) | core I/O (cookies, tracker, monitor, expect) | 821 | 0 | 3 | 17 | No |
| [#156](#pr-156--core-frame-geo) | frame registry + geo | 858 | 3 | 3 | 9 | No |
| [#157](#pr-157--input-and-gate) | input + actionability gate | 1778 | 2 | 5 | 18 | No |
| [#158](#pr-158--visibility-and-tests) | text normalization + regression tests | 945 | 1 | 6 | 8 | No |

Two notes on how to read the findings below.

First, several findings were **overstated** by the reviewer and corrected by the
verifier. Where the correction changes what should be done, it is recorded under
"Verification note". Fix the corrected version, not the original claim. In a few
cases the reviewer's proposed fix was proven to be a net regression; those are
called out explicitly.

Second, the largest single category here is comments and docs that assert
something the adjacent code does not do. That is not cosmetic in this repo: four
prior review rounds found the same class, and the two highest-value findings in
this set (`#156` frame name doc, `#157` `is_visible` doc) are exactly that shape
on public API. A false doc on a public error variant or a public predicate is
treated as correctness, not polish, in the fix order.

---

## 1. Verdict per PR

### #150 — transport

Do not merge as-is, but it is close and the engineering is sound. Four real bugs
are really fixed (write failure reports `DISCONNECTED_CODE`, the actor latches its
exit reason so post-death calls report `Disconnected`, the call budget now wraps
the enqueue, `redial` gains a 30s ceiling), and three of the four were confirmed
by reverting the fix and watching the matching test go red. Two things block it.
The whole-call budget change falsified two public error-variant docs in files the
PR never opened, including retry guidance that is now wrong for one of the two
routes into `Timeout`. And the pending-map sweep, the entire leak fix, has no test
that the actor ever calls it: disabling the sweep call site outright leaves all 57
tests green.

Reviewer's coverage assessment: roughly 80% of new production behavior covered,
below the 90% bar, and the gaps are the PR's two headline changes rather than edge
cases. Test quality is otherwise good; tests assert typed error variants and
elapsed-time bounds rather than "a frame was sent", and the sweep-boundary test
reasons correctly about the `biased` select instead of sleeping.

Reviewer's security assessment: effectively no security surface. No untrusted-byte
parsing, no file modes, no credentials, no new listener, nothing `unsafe`. One
hygiene note: the redial timeout error interpolates the raw DevTools `ws_url`,
whose path UUID is a bearer capability for the browser. That URL is already
emitted at `debug!` elsewhere and the endpoint is loopback-bound, so this is an
escalation of visibility into an error `Display`, not a new exposure.

### #152 — solvers

Do not merge as-is. The diagnosis is genuinely good: the mid-flight escalation
fix, both solver latches, and the DataDome deadline bound are real bugs found and
really fixed, and the comments explaining them are unusually careful. But four
defects were confirmed by execution, and three of them live in the JavaScript,
which has zero behavioral coverage. Imperva's solver call is not raced against the
deadline while its DataDome sibling got exactly that fix in the same PR. The
Cloudflare walker leaks two named functions onto the page's global object on every
poll tick, in the one crate whose entire purpose is leaving no trace, and the code
it replaced leaked nothing.

Reviewer's coverage assessment: the Rust is around 90% covered and the important
tests genuinely fail against the unfixed code. The JavaScript is at 0%. The only
"test" for `detect.js` greps the source for a literal substring and compares byte
offsets; it cannot catch a wrong prefix (it hard-codes the wrong one), the global
leak, or the smooth-scroll re-measure. Its docstring claims the JS "cannot be
exercised from a mocked CDP transport", which is true and is an argument for a
different harness, not for a grep.

Reviewer's security assessment: real surface, and one finding sits on it. These
crates exist to be un-fingerprintable, so what the page can observe about us is
the security model. `WALKER_JS` runs as a top-level script in the page's main
world, installing `findChallengeIframe` and `clickableRect` on `window` where the
challenge script can enumerate them. Beyond that: no `unsafe`, no new credential
or filesystem surface, no new egress. The unbounded Imperva solver is a
denial-of-service-on-yourself rather than an exploit.

### #155 — core I/O

Do not merge as-is. The direction is right and the tests are unusually
well-written, but the security-sensitive part is wrong in the exact way it set out
to fix. `write_atomic` creates the temp file with the process umask and narrows it
afterwards, so the file holding the full cookie JSON is world-readable for the
duration of the write. For a jar the user deliberately chmod'd 0600 that is a
regression against the `fs::write` it replaced, which never widened anything. The
same open has no `O_EXCL` on a fully predictable temp path. Both close with one
line: `OpenOptions::new().create_new(true).mode(0o600)`.

Reviewer's coverage assessment: roughly 85% of new lines, and the tests that exist
would genuinely fail against the unfixed code. Uncovered and unjustified: the
`& 0o7777` mask (every test masks `& 0o777`), the `fs::write` failure path, the
non-`NotFound` arm of `apply_destination_mode`, the tracker cache's route through
`write_atomic`, and the monitor latch's observable consequence at its call site.

Reviewer's security assessment: real and non-trivial. `write_atomic` is the write
path for live session cookies. Three issues: the world-readable window (confirmed
empirically, modes `["600","644"]` observed across 1668 samples of a large write),
the missing `O_EXCL` on a guessable path, and an unbounded `.text()` read of a
user-supplied blocklist URL on the `Browser::launch()` path where the only cap is
20 seconds of wall clock.

### #156 — core frame + geo

Do not merge as-is. The idea behind the provisional sweep is right, but it puts a
CDP round-trip inline in the `frameAttached` select arm, which parks the entire
frame-lifecycle loop for up to five seconds. The event source underneath is a
1024-slot broadcast that silently drops lagged frames, so on a busy page that
stall is registry corruption with no error anywhere. Separately the name-backfill
replaces the registry entry and orphans any `Frame` handle a caller already holds,
and the geo half of the PR is untested: reverting `resolve()` to the base
implementation leaves both new tests green.

Reviewer's coverage assessment: below the bar and unevenly so. The lifecycle sweep
happy paths are well covered and genuinely fail on base. Uncovered: the blocking
behavior itself, the OOPIF session-filter branch, the evict-during-queued-navigation
race, the handle-orphaning consequence, and essentially the entire geo delta,
which is three `tracing::warn!` sites nothing asserts.

Reviewer's security assessment: no meaningful new surface, checked rather than
assumed. The new `Page.getFrameTree` walkers are unbounded recursions, but
`serde_json`'s 128-level default recursion limit rejects a hostile nesting depth
before the walkers see it. No new `unsafe`, no new egress, and the added warns log
`status`, `endpoint` and `reqwest::Error` Display, none of which carry the proxy
credentials that go via `basic_auth`.

### #157 — input and gate

Do not merge as-is. The core work is good on every axis: `buttons` on the wire,
real press/release pairs for multi-click, `special_text` for Space and Enter, the
`el`-prepend off-by-one fixes, the CallArgument wrapping. The suite is green (385
passed). What blocks it is the claims around the code. Three separate places ship
a statement the adjacent code contradicts, which is the exact failure mode four
prior review rounds kept catching. And `check_visible` quietly grew a
viewport-intersection requirement that changes what the public `Element::is_visible()`
and `.visible_only()` mean, with none of the four doc surfaces updated and no
compensation on the query paths, which cannot scroll.

Reviewer's coverage assessment: well short of 90% meaningful coverage despite a
healthy test count. Proven by mutation: deleting the entire `buttons_held` insert
from `mouse_drag` leaves all four drag tests green. Both `move_realistic` behavior
changes have no test at all. The seven new actionability tests assert JS source
substrings, so a mutation that breaks the predicate while preserving the strings
passes all of them.

Reviewer's security assessment: real surface, mostly improved. The effective
opacity product closes the `opacity: 0.001` honeypot the old exact-zero string
compare sailed past, and reading the viewport from `documentElement`/`body` closes
a fail-open on quirks-mode pages. Two things to weigh: the predicate now reads
page-controlled geometry, so a hostile quirks-mode page can zero
`body.clientWidth/Height` and make every element permanently non-actionable (fail
closed, so denial of automation rather than bypass); and the hard dependency on
`Element.checkVisibility` with Chrome 121 option names means older Chrome silently
degrades to a bare `display: none` test.

### #158 — visibility and tests

Do not merge as-is. The 945 lines split into three unequal pieces and the smallest
one carries the bugs. The `normalize-space` unification is half-done: the Rust
needle folder is faithful to XPath, but the JS side it mirrors ends in `.trim()`,
which strips a wider whitespace class, so `text_equals` and `text_exact` still
disagree on `&nbsp;` markup while the new doc claims they cannot. The one test
written for the selectors half drives a resolver the file's own comment says is
dead in production; the shipping copy got the identical fix and no test.

Reviewer's coverage assessment: roughly 90% by line but badly split. The
`predicate.rs` fix is well covered by tests that fail against the unfixed code.
The `selectors.rs` fix is covered only on the dead resolver, so the shipping copy
is at 0% (proven by deleting it and watching the suite stay green). The commit's
headline claim, the ancestor-opacity honeypot walk, has zero real-Chrome coverage
anywhere.

Reviewer's security assessment: no new surface. Every caller-supplied needle goes
through JSON quoting before it reaches JS source, and `normalize_space` only
removes bytes. One adjacent sharp edge is untrusted-input-shaped but not a
vulnerability: a needle containing an apostrophe builds a malformed XPath and
surfaces as a `JsException`, a denial of the query rather than an escape.

---

## 2. Findings by PR

Ordered by severity within each PR.

### PR #150 — transport

#### 150-1. `CallError::Timeout` doc contradicts the new timeout shape

[error.rs:52](crates/zendriver-transport/src/error.rs:52) · **medium** · stale-doc

**What.** The variant doc says the command "was written to the socket but Chrome
never answered it", and the bullet list below tells the reader the connection is
"fine" and that retrying can be reasonable. The PR moved `cmd_tx.send()` inside
the budgeted future, so `Timeout` is now also returned when the command never
reached any socket.

**Why it matters.** The PR's own new test proves it:
`call_budget_bounds_the_enqueue_not_just_the_reply` builds a `Connection` with no
actor and no WebSocket, fills the single channel slot, and asserts
`CallError::Timeout`. A variant whose docs promise "written to the socket" is now
reachable in a state where no socket exists. This is the public contract, not an
internal note.

**Fix.** Rewrite the variant doc to cover both routes: the command was written and
Chrome never answered, or it never reached the socket because the actor's command
channel stayed full for the whole budget. The `#[error(...)]` string "went
unanswered after {budget:?}" is also inaccurate for the enqueue case; "did not
complete within" is honest for both.

**Verification note.** The reviewer's added claim that "each retry adds another
waiter and makes it worse" is wrong. A dropped tokio `send` future releases its
channel reservation, so a retry leaves no residue; the caller spins, it does not
compound a backlog. Do not put that sentence in the replacement doc.

#### 150-2. `ZendriverError::CdpTimeout` carries the same false claim on public API

[error.rs:99](crates/zendriver/src/error.rs:99) · **medium** · stale-doc

**What.** The user-facing mapping of `CallError::Timeout` (the `From` impl at
[error.rs:298](crates/zendriver/src/error.rs:298)) says "Chrome accepted a CDP
command and never answered it" and "here the connection is healthy and the browser
is simply wedged". After this PR Chrome may never have seen the command.

**Why it matters.** This is the variant users actually match on. `CallError` is
near-internal; `ZendriverError` is what every consumer pattern-matches. The doc
also draws an explicit contrast with `ZendriverError::Disconnected` that no longer
partitions cleanly. The repo's own CLAUDE.md makes rustdoc sync a PR-completion
gate, and this behavior change crossed a crate boundary without following.

**Fix.** Mirror whatever wording lands in `zendriver-transport`. Replace "Chrome
accepted a CDP command" with "a CDP call did not complete within its budget:
either Chrome accepted it and never answered, or it never left the send queue",
and soften "the connection is healthy" to "the connection has not reported a
failure". Separately, [error-reference.md](docs/book/src/error-reference.md) has
rows for `Disconnected` and `Timeout(Duration)` but no `CdpTimeout` row at all;
close that in the same pass.

#### 150-3. The pending-map sweep has no test that the actor calls it

[actor.rs:190](crates/zendriver-transport/src/actor.rs:190) · **medium** · test-coverage

**What.** The sweep is the entire fix for the described leak and the reason
`PENDING_SWEEP_INTERVAL`, `sweep_pending` and `dispatched_since_sweep` exist.
Editing the guard to `if false && dispatched_since_sweep >= PENDING_SWEEP_INTERVAL`
disables it completely and the suite still reports 57 passed, 0 failed.

**Why it matters.** The feature can be deleted from the actor loop and CI stays
green, so CI is not protecting it. Look at what the two sweep tests assert.
`sweep_pending_drops_abandoned_callers_and_keeps_live_ones` builds a `HashMap` by
hand and calls the free function; it proves `retain(!is_closed)` works, which was
never in doubt. `live_pending_survives_the_sweep_boundary` drives a real actor past
two boundaries but only asserts that id 1 still routes, which guards against
over-reaping. Nothing asserts the map shrinks.

**Fix.** Make the sweep observable. Add `pub(crate) swept_total: AtomicU64` to
`ConnectionInner` next to `socket_died`, bump it where the actor currently only
logs, and extend `live_pending_survives_the_sweep_boundary` to build with a real
`Arc::downgrade(&conn.inner)` instead of the `Weak::new()` it passes today, then
assert `swept_total >= PENDING_SWEEP_INTERVAL`. That turns the strongest existing
test into a genuine regression test and gives a metric worth having in production.

#### 150-4. The command-channel depth `64` is written three times

[connection.rs:267](crates/zendriver-transport/src/connection.rs:267) · **medium** · dry

**What.** The new rustdoc asserts "the actor's command channel is 64 deep". The
literal `64` appears at
[connection.rs:560](crates/zendriver-transport/src/connection.rs:560) (inside
`reconnect`) and
[connection.rs:790](crates/zendriver-transport/src/connection.rs:790) (inside the
spawn helper), and is now depended on by prose in a third place, with no named
constant tying them together.

**Why it matters.** Someone profiling a burst-heavy workload raises the spawn-site
value to 256 and misses the reconnect site, so the connection quietly gets a
smaller queue after every reconnect than it had on first connect. That is a
capacity regression that only manifests under load, only after a reconnect, and
that nothing asserts. The file already does this correctly for the neighbouring
value: `EVENT_BUS_CAPACITY` is a named `pub(crate) const` used at both spawn sites
precisely so this cannot happen.

**Fix.** Add `pub(crate) const CMD_CHANNEL_CAPACITY: usize = 64;` next to
`EVENT_BUS_CAPACITY` in `actor.rs`, use it at both `mpsc::channel` sites, and have
the rustdoc link the constant rather than restate the number.

#### 150-5. `reconnect`'s ordering comment asserts an ordering the code cannot produce

[connection.rs:593](crates/zendriver-transport/src/connection.rs:593) · **low** · stale-comment

**What.** The comment on the `socket_died` clear says it is "cleared last so it
lands after the cancelled actor's own store". `reconnect` is a synchronous `fn`
with no yield point between `guard.cancel()` and the `store(false, Release)`, so
the cancelled actor's exit store typically lands after the clear, not before.

**Why it matters.** The conclusion survives, but for a different reason than the
one stated. The safety comes from "the reader can only reach the load once the
newest actor has already overwritten", not from the ordering claim. If someone
adds a read path that does not require the current actor to be gone (a
`Connection::is_disconnected()` accessor is the obvious next feature given this
PR's premise), the stated invariant would justify trusting a flag that is simply
wrong.

**Fix.** Delete the "so it lands after" clause. Keep the rest of the comment,
which already states the real argument, and add the load-bearing sentence: do not
add a read path that does not depend on the current actor being gone without
revisiting this.

**Verification note.** The reviewer characterised the correct argument as a
"parenthetical afterthought"; it is in fact already a full sentence in the
comment. Only the false clause needs deleting.

#### 150-6. Four sibling docs still call the budget a "reply budget"

[connection.rs:141](crates/zendriver-transport/src/connection.rs:141) · **low** · stale-doc

**What.** `call_raw_with_timeout`'s rustdoc was correctly updated to say the budget
covers the whole call, but `ConnectionInner::call_timeout_ms` (line 141),
`Connection::call_timeout` (line 238), `Connection::set_call_timeout` (line 243)
and `DEFAULT_CALL_TIMEOUT` (line 26) all still describe it as a reply budget.

**Why it matters.** One knob, two contradictory descriptions in one file, three of
them on public API. A reader who lands on `set_call_timeout` (the natural entry
point for "make my calls time out faster") is told it bounds the reply, sets 500ms
for a latency-sensitive workload, and is surprised when a call fails without Chrome
being contacted. `DEFAULT_CALL_TIMEOUT`'s "Why 180s" section reasons purely about
round-trip latency and never accounts for queue-wait, which is now inside the same
budget; the 180s conclusion still holds but the reasoning is incomplete, and it is
the reasoning future tuning will lean on.

**Fix.** Sweep all four to "per-call budget", and add one sentence to the
`DEFAULT_CALL_TIMEOUT` floor bullet noting the budget now covers time waiting for
room in the command channel and that this does not move the floor. Note there is a
fifth site the suggested `grep 'reply budget'` would miss: `Connection::call_raw`'s
own doc at lines 218-227 says "a Chrome that accepts the command and never answers
yields `CallError::Timeout`".

#### 150-7. `redial_timeout_matches_the_launch_handshake_guard` locks nothing

[connection.rs:1228](crates/zendriver-transport/src/connection.rs:1228) · **low** · weak-test

**What.** The test declares a local `const LAUNCH_HANDSHAKE_TIMEOUT` and asserts
`REDIAL_TIMEOUT == LAUNCH_HANDSHAKE_TIMEOUT`. Both sides are literals in the same
six-line function. The real `HANDSHAKE_TIMEOUT` lives at
[browser.rs:2310](crates/zendriver/src/browser.rs:2310) and is never referenced.

**Why it matters.** Change `browser.rs:2310` to 60s and this test still passes,
the two constants diverge, and the `REDIAL_TIMEOUT` doc claim of "deliberately
matched" becomes false. The dependency direction makes the honest version
impossible here: `zendriver-transport` is a dependency of `zendriver` and
structurally cannot see `HANDSHAKE_TIMEOUT`. Writing the assertion in the crate
that cannot check it is how you end up believing you have a guard you do not have.
The same shape already exists at
[connection.rs:1494](crates/zendriver-transport/src/connection.rs:1494), so this
is a pattern worth stopping.

**Fix.** Cheaper honest option: delete the test and downgrade the `REDIAL_TIMEOUT`
doc from "deliberately matched" to "chosen to match, by convention". The
alternative (make `REDIAL_TIMEOUT` `pub` and assert from `zendriver`) widens the
transport crate's public surface, which `lib.rs` explicitly steers consumers away
from. See "Needs a human".

#### 150-8. `reconnect()` clearing `socket_died` has no test

[connection.rs:599](crates/zendriver-transport/src/connection.rs:599) · **low** · test-coverage

**What.** New behavior with an eight-line justification comment and no test.
Commenting out the line leaves the suite green (57 passed).

**Why it matters.** The line is unobservable belt-and-braces, which is why no test
pins it, but new behavior that the author justified at length should either be
pinned or marked as deliberately unobservable.

**Fix.** Either add the test (kill socket A, drain the in-flight `Disconnected`,
`reconnect(ws_b)`, `shutdown()`, assert `Shutdown`) or, better, note in the
comment that the clear is defensive and unobservable given the single reader.

**Verification note.** The reviewer's stated harm (latch stays `true` forever,
`Browser::reconnect`'s documented recipe loops infinitely) was refuted. The
verifier wrote exactly the proposed test with line 599 removed and it passed five
times out of five: `socket_died` has exactly one reader, `actor_gone_error`, and
both paths to it require the current actor to be gone, so every actor exit
re-latches the flag. This finding is a coverage gap on a redundant line, nothing
more. Note it directly contradicts finding 150-5's harm story; 150-5's corrected
comment is the argument that makes 150-8's harm impossible.

#### 150-9. Two new drain tests duplicate two existing ones

[connection.rs:1033](crates/zendriver-transport/src/connection.rs:1033) · **low** · dry

**What.** `calls_made_after_the_socket_dies_still_report_disconnected` and
`calls_made_after_a_clean_shutdown_still_report_shutdown` are structurally parallel
to the two older tests at
[connection.rs:946](crates/zendriver-transport/src/connection.rs:946) and
[connection.rs:973](crates/zendriver-transport/src/connection.rs:973), differing by
an appended three-iteration loop.

**Why it matters.** Four tests now assert the two in-flight drain behaviours, so a
change to that path fails twice with near-identical output and a maintainer reads
both to learn they say the same thing. The duplicated prefix is not gratuitous: it
establishes that the actor was provably up before being killed.

**Fix.** Optional. Either delete the two narrow tests or extract the shared prefix
into a helper returning the connection, the inbound sender and the in-flight
handle. Do not do both.

**Verification note.** The reviewer's "verbatim", "statement for statement" and
"strict superset" claims are all wrong; the tests differ in payloads, bindings,
local imports and trailing `shutdown()`. This is a readability observation, not a
duplication defect, and leaving it is defensible.

#### 150-10. `PENDING_SWEEP_INTERVAL` names a count but reads as a duration

[actor.rs:48](crates/zendriver-transport/src/actor.rs:48) · **low** · naming

**What.** The constant is a dispatch count in a file dense with real `Duration`
values, next to `REDIAL_TIMEOUT` and `DEFAULT_CALL_TIMEOUT` in the sibling module.

**Why it matters.** A time-based interval bounds debris by wall clock; the actual
dispatch-based one bounds it by count and holds entries indefinitely on an idle
connection. Those are different properties and the name points at the wrong one.

**Fix.** Rename to `PENDING_SWEEP_DISPATCH_INTERVAL` if you touch this file
anyway. Taste-level.

**Verification note.** The unit is stated in the first line of the constant's own
doc and the loop variable is `dispatched_since_sweep`, so the risk of a real
misread is low. "Interval" for a count is also a common idiom.

#### 150-11. "pointer-sized entries" understates the sweep's footprint

[actor.rs:44](crates/zendriver-transport/src/actor.rs:44) · **low** · docs

**What.** The doc says the sweep bounds debris at "a few hundred pointer-sized
entries". A `oneshot::Sender` is one pointer wide, but it retains a heap cell sized
for `Result<Value, CdpRpcError>`, where `CdpRpcError` holds a `String` and an
`Option<Value>`. With the map slot, each entry is closer to 100 bytes.

**Why it matters.** The conclusion is unaffected (about 25 KB at the bound), which
is precisely why it is worth fixing rather than arguing about: the doc makes a
defensible call rest on a premise that is not true, and someone re-tuning this
will trust the stated magnitude instead of re-deriving it. Getting the small
claims right is what makes the big claims in this file credible.

**Fix.** Say "a few hundred small heap entries, tens of kilobytes at the bound".
While editing, add the omitted caveat: because the counter advances on dispatch
rather than on a clock, an idle connection holds its debris until traffic resumes.

#### 150-12. The exit-latch comment misattributes where the happens-before comes from

[actor.rs:371](crates/zendriver-transport/src/actor.rs:371) · **low** · stale-comment

**What.** The comment says `Release`/`Acquire` "is the point: this store must be
visible to the thread that later learns of the channel close". The visibility
actually comes from the channel: the store precedes the `cmd_rx` drop in program
order, the drop synchronizes with a sender observing the channel closed, and
happens-before is transitive. A `Relaxed` store would still be visible.

**Why it matters.** The code is correct and conservatively over-ordered, so
nothing is broken. But this is a file where the next person will reason about
atomics under time pressure, and someone who internalizes "Release/Acquire on this
flag is what makes it visible" will later add a second flag and believe they have
an ordering guarantee between the two that pairwise release/acquire on distinct
locations does not give them.

**Fix.** Credit the channel, and state the real reason not to use `Relaxed`: the
stronger ordering costs nothing and keeps the flag correct if a future read path
ever reaches it without going through the channel.

#### 150-13. A load-bearing `Vec` in a test looks like dead code

[connection.rs:1190](crates/zendriver-transport/src/connection.rs:1190) · **low** · readability

**What.** In `redial_times_out_instead_of_hanging_forever` the silent-server task
accumulates accepted sockets into a `Vec` it never reads, with no comment saying
why.

**Why it matters.** Verified by mutation: replacing the body with the obvious
cleanup `while let Ok((_sock, _peer)) = listener.accept().await {}` makes the test
fail with "expected a TimedOut io error, got ConnectionReset". Dropping the socket
closes the connection, so the client gets a fast error instead of the silence the
test needs, and the panic message points at the timeout logic rather than at the
test's own server. No linter fires, because `push` counts as a use.

**Fix.** One comment on the binding: hold every accepted socket open, because
dropping it would close the connection and give the client a fast error instead of
the silence this test needs.

#### 150-14. `duplex_pair` is defined three times and the copies have now diverged

[actor.rs:556](crates/zendriver-transport/src/actor.rs:556) · **low** · dry

**What.** `duplex_pair` exists at
[actor.rs:544](crates/zendriver-transport/src/actor.rs:544),
[connection.rs:891](crates/zendriver-transport/src/connection.rs:891) and
[session.rs:98](crates/zendriver-transport/src/session.rs:98) with identical
bodies. This PR extended only the `actor.rs` copy with a
`duplex_pair_with_capacity` variant, so the copies are now unequal in capability.

**Why it matters.** The new helper's docstring captures a genuinely non-obvious
gotcha (the driver's sink is `try_send`-based, so a test that dispatches a large
burst without draining needs room for all of it or the write fails and kills the
actor). That knowledge is invisible from the other two modules, and the next person
who needs a capacity-controlled duplex in `connection.rs` will add a fourth copy.

**Fix.** Move both helpers into the existing `pub(crate) mod test_only` at
[connection.rs:831](crates/zendriver-transport/src/connection.rs:831), which is
already `#[cfg(any(test, feature = "testing"))]` and already hosts `DriverStream`
for exactly this reason.

#### 150-15. The redial timeout error interpolates the DevTools URL

[connection.rs:645](crates/zendriver-transport/src/connection.rs:645) · **low** · security

**What.** `format!("redial to {ws_url} did not complete within {budget:?}")`.
Chrome's DevTools URL is `ws://127.0.0.1:PORT/devtools/browser/<UUID>`, and that
UUID is a bearer capability for the browser.

**Why it matters.** Not a new exposure: the endpoint is loopback-bound and
[browser.rs:3361](crates/zendriver/src/browser.rs:3361) already emits the same URL
at `debug!`. It is an escalation of visibility, moving the token from a debug
record (usually filtered in production) into an error `Display` that surfaces at
any log level and propagates into whatever error tracker the consumer uses. The
general rule is worth internalizing: a URL carrying a credential in its path
should be treated as a secret in error messages even when debug logs treat it
casually, because errors travel further than logs.

**Fix.** Drop the URL from the message. The caller passed `ws_url` in, so they
already know it, and the sibling `TransportError::Ws` path does not echo it
either.

#### 150-16. `call_raw_with_timeout` allocates the method name twice

[connection.rs:297](crates/zendriver-transport/src/connection.rs:297) · **low** · perf

**What.** `let method = method.into();` then `let cmd_method = method.clone();`,
on every call.

**Why it matters.** It does not, materially. The next thing the actor does with
the command is `serde_json::to_string` over the whole frame before a WebSocket
write, so one clone of a short method name is noise. Recorded so nobody re-raises
it.

**Fix.** None. Leave it.

**Verification note.** The reviewer's claim that the clone is wasted when `budget`
is `None` is backwards: `cmd_method` is moved into `OutboundCmd` on every path; it
is the retained original that goes unread. Treat this as a non-issue.

---

### PR #152 — solvers

#### 152-1. Imperva's solver call is not raced against the deadline

[bypass.rs:380](crates/zendriver-imperva/src/bypass.rs:380) · **high** · correctness

**What.** `self.solve_captcha(kind).await?` runs unbounded. The DataDome sibling
wraps the identical call in `tokio::time::timeout_at(deadline, ...)` at
[bypass.rs:266](crates/zendriver-datadome/src/bypass.rs:266), with a test and a
six-line comment explaining why. Imperva got the latch from that fix but not the
bound.

**Why it matters.** `timeout()` is a contract: the caller says "I have 30 seconds
for this page" and schedules around it. A 2captcha/anticaptcha round-trip is
routinely 15 to 60 seconds and carries no bound of its own, so a solve entered on
the last tick runs to completion regardless. Measured: an 80ms budget with a 3s
solver returned after 3.008s. `wait_for_clearance`'s rustdoc promises "until
clearance is achieved or the configured timeout elapses", which is now breakable.

**Fix.** Mirror the sibling exactly:
`match tokio::time::timeout_at(deadline, self.solve_captcha(kind)).await { Ok(res) => res?, Err(_) => return Ok(ClearanceOutcome::TimedOut { last_surface: Some(snap.surface) }) }`.
Port the DataDome comment and the
`solver_cannot_overrun_the_configured_timeout` test across so the two read the
same.

**Verification note.** Two corrections. The `probe_with_deadline` comment the
reviewer called false is not false; it is scoped to probes, and every probe still
goes through `probe_with_deadline`. And the pre-PR code was equally unbounded, so
the "37x over" measurement is not a regression by itself. What the PR changed is
the worst case, from `solver_latency` to `timeout + solver_latency`, by moving the
call inside the loop where it can be entered on the last tick. Still a real
regression, still the same five-line fix.

#### 152-2. Cloudflare returns `ChallengeGone` while the challenge is still running

[bypass.rs:190](crates/zendriver-cloudflare/src/bypass.rs:190) · **medium** · correctness

**What.** The arm `None if clicks > 0 || (seen_markers_before && !state.has_markers)`
decides "the challenge went away" from `state.bbox`, but this PR redefined `bbox`
from "the iframe is mounted" to "the iframe is a valid click target". Once a click
has landed, any tick where the iframe is mounted but momentarily not clickable
(0-height during re-layout, `opacity: 0` during a transition) yields
`bbox == None` with `has_markers == true`, and the run returns `ChallengeGone`.

**Why it matters.** `ChallengeGone` is a success terminal: it means Cloudflare let
you through, go ahead and scrape. Returning it while the challenge is still
running hands the caller a page that is still a challenge page. Reproduced against
the branch: poll 1 returns a clickable 300x65 widget, we scroll and click, poll 2
returns `bbox: null` with `hasMarkers: true`, and the driver returns
`ChallengeGone` before poll 3 delivers the real token. Before this PR, `findBbox`
returned a rect for any mounted iframe, so `bbox == None` and "iframe gone" meant
the same thing and the arm was sound. Adding `clickableRect()` broke that
equivalence without updating the arm that depended on it. That is the classic
shape of a change being wrong because of something it did not touch.

**Fix.** Fix it in the JavaScript, not in the arm. `POLL_JS` currently collapses
"iframe present" and "iframe clickable" into one nullable field; return
`iframePresent` alongside `bbox` and key the terminal off iframe presence.

**Verification note.** The reviewer's proposed fix (key the terminal off
`seen_markers_before && !state.has_markers` and delete the `clicks > 0` disjunct)
was applied and proven to be a net regression. `hasMarkers` includes
`.cf-turnstile, .turnstile, [data-sitekey]`, containers authored in the site's own
HTML that Cloudflare does not remove when the widget clears, so under that fix
`has_markers` stays true forever on such a page and `ChallengeGone` becomes
unreachable for the whole interactive path: a fast success turns into a
full-budget `TimedOut`. Running the crate's tests individually with a hang guard,
`wait_for_clearance_returns_challenge_gone_when_iframe_disappears_after_click`
hangs under that fix. Also note the false-success scenario itself is inferred from
a mock payload the reviewer chose; no evidence was produced that real Turnstile
emits a mounted-but-unclickable state post-click. The JS-level fix is the safe one
because it removes the ambiguity rather than guessing which reading is right.

#### 152-3. `WALKER_JS` installs two named functions on the page's global object

[bypass.rs:254](crates/zendriver-cloudflare/src/bypass.rs:254) · **medium** · stealth-leak

**What.** `WALKER_JS` declares `findChallengeIframe` and `clickableRect` as bare
top-level function declarations, and `eval_main_world`
([bypass.rs:328](crates/zendriver-cloudflare/src/bypass.rs:328)) ships
`format!("{WALKER_JS}{body}")` to `Runtime.evaluate` with no `contextId`. That runs
as a classic script in the page's default context, where top-level function
declarations become properties of the global object, on every poll tick and again
on the scroll evaluator.

**Why it matters.** Cloudflare's challenge script runs in that same realm.
Whether or not Turnstile enumerates `window` today, this is a pure regression: at
the base commit the equivalent `findBbox` was nested inside the IIFE and leaked
nothing, so a refactor aimed at sharing a helper between two evaluators traded
away something the crate exists to protect, for free. The general rule: anything
sent through `Runtime.evaluate` that is not wrapped in a function scope becomes
part of the page, and the page is adversarial here.

**Fix.** `format!("(function(){{{WALKER_JS}{body}}})()")`, with `POLL_JS` and
`SCROLL_JS` reduced to their `return {...}` statements. One IIFE, zero globals,
identical completion value. Add a regression guard asserting the composed
expression contains no top-level `function ` declaration.

**Verification note.** Downgraded from high. The names are demonstrably installed,
but "a signed confession" is rhetoric; nothing establishes that Turnstile matches
on them, and this crate is used alongside `zendriver-stealth`, which already writes
to the page. The reason to fix is that it is free and it is a regression.

#### 152-4. The Cloudflare book chapter documents behavior that no longer exists

[cloudflare.md:36](docs/book/src/cloudflare.md:36) · **medium** · docs

**What.** The chapter was not touched, but this PR changed both the click strategy
(one click became up to `MAX_CLICK_ATTEMPTS` clicks spaced `CLICK_RETRY_TICKS`
apart, preceded by a scroll-into-view and a re-measure) and the `ChallengeGone`
contract. Line 36 still describes `ChallengeGone` as the container disappearing;
line 60 still says "A raw `mousedown` / `mouseup` is dispatched", singular, with no
retries and no scrolling.

**Why it matters.** CLAUDE.md makes this a blocking gate, not a nicety. The
practical cost is concrete: a user whose widget sits below the fold, or who is
debugging why three clicks show up in their CDP trace instead of one, reads the
book, finds one click and no scrolling, and concludes the driver is misbehaving.

**Fix.** Rewrite the "How it works" list as scroll-into-view, re-measure, click,
with the retry cap and spacing. Update the `ChallengeGone` bullet after 152-2
lands. The chapter needs a full pass rather than a bullet edit: it also documents
`NoChallenge` (line 44) and `ClearanceTimeout` (line 47) as `CloudflareError`
variants, and
[error.rs](crates/zendriver-cloudflare/src/error.rs) declares exactly two,
`Call(#[from] CallError)` and `JsError(String)`. The code sample at line 138
(`Err(CloudflareError::NoChallenge) => ...`) would not compile, and lines 70, 146
and 157 repeat the fiction.

#### 152-5. `visid_incap_*` breaks the rule the PR just introduced for `nlbi`

[detect.js:46](crates/zendriver-imperva/src/detect.js:46) · **medium** · correctness

**What.** The new comment justifies dropping `nlbi` from the `hasLegacyCookies`
scan because it is "set on ordinary traffic and outlives clearance, unlike
`incap_ses_*` / `___utmvc`, which track the challenge itself". `visid_incap_*` is
still in that scan, and it is Imperva's persistent visitor-ID cookie: long-lived
and set on ordinary traffic, which is the stated disqualifying property.

**Why it matters.** The comment's criterion is the valuable part of the change.
Applied consistently it also excludes `visid_incap_*`, and if that is right then
the bug the PR set out to fix is only half fixed: `hasLegacyCookies` still pins
`surface` at `Legacy` on a cleared page, which blocks the `ChallengeGone` terminal
(the arm at [bypass.rs:349](crates/zendriver-imperva/src/bypass.rs:349) requires
`surface_clear`) and burns the budget to `TimedOut`. Scope is limited: a reese84
site still clears via `TokenAcquired`, which needs only `body_clean` and a token,
so this only bites pure-legacy Incapsula flows.

**Fix.** Decide it explicitly and write the decision down. If `visid_incap_*` is
ambient, remove it from the scan (it stays in the `sessions` collector either way)
and update `ImpervaSurface::Legacy`'s rustdoc at
[detection.rs:18](crates/zendriver-imperva/src/detection.rs:18), which lists it as
a Legacy marker. If it is a genuine challenge marker, extend the comment to say
why it differs from `nlbi_*`. See "Needs a human".

#### 152-6. `captcha_solver_is_invoked_at_most_once_per_clearance` discards the outcome

[bypass.rs:1105](crates/zendriver-imperva/src/bypass.rs:1105) · **medium** · weak-test

**What.** The test ends `let _ = fut.await.unwrap();` (the `unwrap` is on the
`JoinHandle`, so the inner `Result<ClearanceOutcome, ImpervaError>` is dropped) and
asserts only `calls == 1`.

**Why it matters.** `calls == 1` does not distinguish "the latch worked and the
loop polled to its deadline" from "the first solve errored and the whole call
returned `Err` on tick one". Both leave the counter at 1. If a future change makes
`solve_captcha` fail (a bad site-key probe, an injection that returns `false`, a
transport hiccup), this test stays green while the feature it guards is completely
broken. The DataDome twin gets this right: it asserts
`matches!(outcome, ClearanceOutcome::TimedOut { .. })` with the message "a CAPTCHA
that never clears must poll to its deadline". Same test, same PR, one checks the
terminal and one does not.

**Fix.** Two lines before the `calls` assertion: unwrap the inner `Result` with an
expect message, then assert `TimedOut`.

#### 152-7. `scrollIntoView` omits `behavior`, so the re-measure can read the pre-scroll rect

[bypass.rs:320](crates/zendriver-cloudflare/src/bypass.rs:320) · **low** · correctness

**What.** `iframe.scrollIntoView({ block: "center", inline: "center" })` defaults
`behavior` to `"auto"`, which resolves to the element's computed `scroll-behavior`.
On a page with `html { scroll-behavior: smooth }` the scroll animates and the
`clickableRect(iframe)` call on the next synchronous line returns the pre-scroll
rect.

**Why it matters.** The whole point of the new round-trip is to re-measure after
scrolling; the module doc says "scrolls it into view, re-reads its box" and the
function doc promises the "post-scroll rect". Under smooth scrolling neither is
true, so for a widget below the fold the computed click lands outside the
viewport. The mock harness cannot catch this, because the mock is what supplies
the "post-scroll" rect the test asserts on.

**Fix.** Add `behavior: "instant"`, the spec value that overrides the computed
`scroll-behavior`.

**Verification note.** Downgraded from high. The same PR added the retry loop that
defeats the worst case: `SCROLL_JS` recomputes `fullyVisible` from the live rect
on every attempt, so by attempt 2 (about 2s at the default interval) the smooth
scroll has finished and the correct rect is returned. Realistic cost is one wasted
click out of three, not a guaranteed timeout.

#### 152-8. `on_captcha`'s docs omit the token-in-hand exception

[bypass.rs:119](crates/zendriver-imperva/src/bypass.rs:119) · **low** · stale-comment

**What.** The rustdoc says "Without this, a CAPTCHA surface returns
`ImpervaError::CaptchaRequired` immediately", repeated in the `# Errors` block at
line 194 and verbatim in
[imperva.md:97](docs/book/src/imperva.md:97). The PR added a guard so that a
CAPTCHA surface with a usable reese84 token in hand does not escalate at all.

**Why it matters.** The docs are incomplete rather than false, but the omitted case
is the one a user will hit and be unable to explain: they registered a solver,
they are on a CAPTCHA surface, and nothing calls their solver. The DataDome side of
this same PR documented its equivalent latch in both the rustdoc and the book, so
the omission is asymmetric rather than considered.

**Fix.** State the real contract in both rustdocs and in `imperva.md`: a CAPTCHA
surface escalates when a solver is registered and no usable reese84 token is
present; without a solver that same condition yields `CaptchaRequired`; with a
token in hand the run keeps polling regardless of surface; and the solver is
invoked at most once per `wait_for_clearance`.

**Verification note.** The reviewer claimed the PR made the sentence false and
cited `a_token_suppresses_escalation_even_while_the_body_is_still_dirty` as an
executable contradiction. Tracing the code shows the sentence is still true for
the case it describes: a CAPTCHA page with no reese84 reaches `solve_captcha` on
the first tick and returns `Err(CaptchaRequired)` with no waiting. The cited test
feeds a token, so it exercises the exception, not the clause.

#### 152-9. `nlbi` prefix match is missing its underscore

[detect.js:126](crates/zendriver-imperva/src/detect.js:126) · **low** · correctness

**What.** `name.indexOf("nlbi") === 0` in the sessions collector, while the comment
two lines above names the target as `nlbi_<siteid>`. Pre-PR it was an exact
equality match, so the PR widened it.

**Why it matters.** The `sessions` array is handed to the caller for cookie
replay, so anything that lands in it gets replayed to the origin as if it were
Imperva state. `nlbi`, `nlbistore`, `nlbi2fa` all match and get swept in. Small
leak, annoying to trace later because nothing errors.

**Fix.** `name.indexOf("nlbi_") === 0`, and update the two string literals in
`detect_js_prefix_matches_nlbi_for_sessions_but_not_as_a_surface_signal`, which
currently pin the sloppy form. While there, the comment added at line 119 ("Same
prefix match as the legacy-cookie scan above") is now wrong: `nlbi` is no longer in
the legacy scan at all.

**Verification note.** The reviewer's consistency argument does not hold (the block
has five conditions, only two of which are prefix tests, and the same file's
reese84 lookup at line 22 is an unanchored substring match that is looser still).
It is a one-character sloppiness, not a convention break.

#### 152-10. The `detect.js` test asserts on source text, not behavior

[detection.rs:313](crates/zendriver-imperva/src/detection.rs:313) · **low** · weak-test

**What.** The test counts occurrences of the literal `indexOf("nlbi") === 0`,
asserts the absence of `=== "nlbi"`, and compares two `str::find` byte offsets to
assert that the surviving match sits after the `hasLegacyCookies` scan.

**Why it matters.** It verifies formatting, not behavior. Reflowing the `if`,
renaming a local, or switching to `startsWith` breaks it with no behavior change,
and it happily passes the wrong prefix because it hard-codes the wrong prefix as
the expected string. The byte-offset comparison encodes a fact about text layout
that any reordering invalidates.

**Fix.** Delete the byte-offset assertion and fix the hard-coded prefix. Keep it as
the cheap source guard it is.

**Verification note.** The reviewer's proposed remedy (add `boa_engine` or
`deno_core` as a dev-dependency to run the file) is disproportionate for a
cookie-name prefix assertion, and the docstring it attacks is accurate: it claims
the crate's mock transport cannot exercise the JS, not that no harness could.

#### 152-11. Two divergent shadow-DOM walkers plus two copies of the evaluate decode

[detection.rs:72](crates/zendriver-cloudflare/src/detection.rs:72) · **low** · dry

**What.** `detect_challenge` at
[detection.rs:33](crates/zendriver-cloudflare/src/detection.rs:33) repeats
`eval_main_world`'s call, `exceptionDetails` and `result.value` decode almost line
for line, and `detect.js`'s `walk()` returns a rect for any mounted challenge
iframe while `clickableRect()` returns null for a hidden or zero-size one.

**Why it matters.** Neither file was touched by this PR, so the duplication is
pre-existing, but the PR widened the divergence between the two walkers.

**Fix.** Extract `eval_main_world` into a shared helper and point
`detect_challenge` at it. Do not take the reviewer's further suggestion.

**Verification note.** The reviewer framed this as "two public-ish answers to the
same question that no longer match" and proposed reimplementing
`is_challenge_present` as `poll_state(...).has_markers`. That is wrong on both
counts. The two predicates are deliberately different and correctly named:
`is_challenge_present` documents "a challenge iframe is currently **mounted**",
`PollState.bbox` documents "**a valid click target**". And the proposed rewrite
would silently loosen the public predicate to return true for any `[data-sitekey]`
container on a page that has already cleared.

#### 152-12. `next_cmd` is duplicated byte-for-byte across two crates

[bypass.rs:440](crates/zendriver-cloudflare/src/bypass.rs:440) · **low** · dry

**What.** The bounded 300-iteration drain over `try_recv_cmd` is identical at
[bypass.rs:440](crates/zendriver-cloudflare/src/bypass.rs:440) and
[bypass.rs:606](crates/zendriver-datadome/src/bypass.rs:606). It is a generic
`MockConnection` utility with nothing vendor-specific in it.

**Why it matters.** It exists because `MockConnection` offers only a blocking
`expect_cmd` (which hangs a finished driver) and a non-blocking `try_recv_cmd`
(which returns `None` on an empty-but-live channel). Every future crate that needs
to drain a non-deterministic command sequence will write it a third time with a
different iteration count.

**Fix.** Add `MockConnection::recv_cmd_timeout(&mut self, dur: Duration)` to
`zendriver_transport::testing`, taking the budget as a parameter rather than
hard-coding 300 x 1ms, and delete both local copies.

#### 152-13. DataDome's poll loop has no stall detection

[bypass.rs:294](crates/zendriver-datadome/src/bypass.rs:294) · **low** · consistency

**What.** Both siblings warn "clearance stalled, is `BrowserBuilder::stealth`
enabled?" after ten ticks on an unchanging surface
([bypass.rs:393](crates/zendriver-imperva/src/bypass.rs:393),
[bypass.rs:196](crates/zendriver-cloudflare/src/bypass.rs:196)). DataDome tracks
`last_surface` but never compares consecutive snapshots.

**Why it matters.** That warning is the single most useful diagnostic these crates
emit; "your bypass is stuck because stealth is off" is the answer to most support
questions about them, and it is the one thing a user cannot work out from a
`TimedOut` return value. A DataDome device check that never resolves produces
thirty seconds of silence and then `TimedOut { last_surface: Some(DeviceCheck) }`
with no hint that the fix is one builder call away.

**Fix.** Port `prev_surface` / `stall_ticks` / the ten-tick warn from
[bypass.rs:387](crates/zendriver-imperva/src/bypass.rs:387). Port the Imperva
variant, not the Cloudflare one: Cloudflare's copy never resets `stall_ticks` (see
152-15).

#### 152-14. Interception is armed before the `Block` terminal is decided

[bypass.rs:163](crates/zendriver-datadome/src/bypass.rs:163) · **low** · perf

**What.** `wait_for_clearance` spawns the Fetch interception task before entering
the loop, and the loop is where `DataDomeSurface::Block` is decided, so a pre-loop
probe that already reports a ban does a full `Fetch.enable` round-trip that is
cancelled one iteration later.

**Why it matters.** Marginal. It only fires for callers who opted into
`with_interception()`, the loop consumes the primed snapshot with no extra probe so
`Block` returns on the first match microseconds later, and the spawned task's only
job is to continue everything it sees.

**Fix.** Probably none. The reviewer's proposed pre-loop short-circuit puts the
same terminal in two places, which the PR's own comment deliberately argues
against ("`Block` and CAPTCHA escalation live in the loop so they also fire when
the surface changes mid-flight"). Recorded so it is not re-raised.

#### 152-15. Three interacting counters in the Cloudflare loop, one with a hole

[bypass.rs:205](crates/zendriver-cloudflare/src/bypass.rs:205) · **low** · readability

**What.** `clicks`, `ticks_since_click` and `stall_ticks` are updated at three
different points with no single statement of the intended cadence.
`ticks_since_click` increments before any click has happened (harmless only
because `clicks == 0` short-circuits the gate), and `stall_ticks` is not
incremented on a tick where `bbox` was `Some`, `may_click` was true, and
`scroll_into_view` returned `None`.

**Why it matters.** Working out that the second click lands on tick 5 rather than
tick 4 requires simulating the loop by hand. The `stall_ticks` hole is a small
real defect: if the widget stays mounted but stops being measurable, every tick
enters the arm, does nothing, and skips the stall counter, so the stall hint that
exists precisely to diagnose a stuck run can never fire on the case it would most
help with. There is also a dead conjunct at line 195: `stall_ticks` only ever
increments, so `stall_ticks == 10` fires exactly once and `&& !warned_stall` can
never be false when reached (pre-existing).

**Fix.** Increment `stall_ticks` on any tick that makes no progress, including the
scroll-returned-`None` path, and reset it only when a click lands. Guard the
`ticks_since_click` increment on `clicks > 0`. Add a one-line comment above
`may_click` spelling out the cadence. Keep `warned_stall`: once `stall_ticks` can
reset, `== 10` becomes reachable repeatedly and the latch is the only thing
stopping a repeated warn. (The reviewer's fix text said to delete it, which
contradicts the reset.)

#### 152-16. A dead `Result` wrapper in an Imperva test

[bypass.rs:1013](crates/zendriver-imperva/src/bypass.rs:1013) · **low** · readability

**What.** `while let Ok(Ok(id)) = tokio::time::timeout(dur, mock.expect_cmd(...)).await.map(Ok::<u64, ()>)`.
The `()` error is never constructed and never matched, so the inner layer carries
no information.

**Why it matters.** It takes three reads to confirm the inner `Result` is inert,
and the same file already has the readable idiom:
`captcha_solver_is_invoked_at_most_once_per_clearance` uses plain
`let Ok(id) = ... else { break }` for the same job.

**Fix.** Delete the `.map(Ok::<u64, ()>)` and match on `Ok(id)`.

#### 152-17. Three JavaScript styles inside one crate

[bypass.rs:255](crates/zendriver-cloudflare/src/bypass.rs:255) · **low** · idiom

**What.** `WALKER_JS` is ES5 (`var`, indexed loops, function declarations) while
`crates/zendriver-cloudflare/src/detect.js`, in the same crate and doing the same
walk, uses `const` and `for...of`. The DataDome and Imperva `detect.js` files use
`var`.

**Why it matters.** There is a real argument for ES5 in injected scripts (broader
engine reach, older syntax is marginally less distinctive) and a real argument for
modern syntax (readability). Either is fine. Having both inside one crate for the
same DOM walk is not, because the next person has to guess which convention
applies.

**Fix.** Pick one and state it in a short comment at the top of the JS constants.
If ES5 is deliberate for injection-surface reasons, that reasoning is worth one
line, because it is not self-evident and someone will otherwise modernise it.

---

### PR #155 — core I/O

#### 155-1. The blocklist fetch is time-bounded but not size-bounded

[tracker.rs:92](crates/zendriver/src/tracker.rs:92) · **medium** · correctness

**What.** `.text()` buffers the whole response into a `String` with no cap. The
commit subject is "bound the blocklist fetch", but only the time dimension was
bounded: a response has 20 seconds to send as much as it likes.

**Why it matters.** This runs inside `Browser::launch()`
([browser.rs:1746](crates/zendriver/src/browser.rs:1746)), and the URL is one the
user pasted in, typically a third-party list mirror. If that mirror is taken over,
misconfigured, or serves an HTML error page from a CDN that streams forever,
`DOWNLOAD_TIMEOUT` caps the damage at 20 seconds of transfer, which on a
datacentre link is multiple gigabytes of `String` in the launch path, followed by
`write_atomic` copying all of it to the cache directory. A time bound is not a
resource bound when the resource is memory. The repo's own fork-eval memory already
tracks `BoundedBody` as a wanted upstream item, so the gap is recognised rather
than hypothetical.

**Fix.** Add `MAX_BLOCKLIST_BYTES` (32 MiB is generous; real lists are around
1 MB), reject early on `content_length` when the server is honest, and stream with
`resp.chunk()` checking the accumulated length, which is what actually holds
against a chunked response. Test with a `wiremock` route (already a
dev-dependency) that streams past the cap.

#### 155-2. Both latch tests use `num_alive_tasks()` as their completion oracle

[mod.rs:1104](crates/zendriver/src/monitor/mod.rs:1104) · **medium** · weak-test

**What.** `latch_after_stream_error` polls
`tokio::runtime::Handle::current().metrics().num_alive_tasks()` in a sleep loop to
decide the spawned task finished, and the same pattern is copy-pasted into both
`expect` drop tests. The doc asserts "the only other task on this runtime is the
mock connection's actor, which stays alive for the whole test".

**Why it matters.** That claim is true today but it is a claim about another
crate's internals, and `zendriver-transport` already spawns additional tasks on
observer-dispatch paths. The failure mode when it stops holding is silent: if an
unrelated task exits during the wait, the count drops to baseline early, the loop
breaks, and the latch is read before the task under test has run. The `-32601`
test would then fail visibly, but the `-32602` test would read `false` and pass,
which is a permanently green test that no longer tests anything. And if an extra
task is spawned and stays alive, the loop hangs to its 2s timeout and fails for a
reason unrelated to the code under review.

**Fix.** Give the test a direct handle instead of inferring one from global runtime
state. `spawn_stream_resource_content` already ends in `tokio::spawn`; return the
`JoinHandle<()>` and have the production call site drop it (which detaches exactly
as today). The test becomes `handle.await.unwrap();` before reading the latch: no
sleeps, no timeout, no dependency on what else is on the runtime. Do the same for
the two `expect` tests via a `#[cfg(test)] register_with_handle`.

#### 155-3. Nothing tests the latch's observable consequence

[mod.rs:1104](crates/zendriver/src/monitor/mod.rs:1104) · **medium** · test-coverage

**What.** Both new tests drive `spawn_stream_resource_content` directly and assert
on the `AtomicBool`. Nothing asserts that after a lost `-32602` a second request
still gets a `Network.streamResourceContent` call, or that after a `-32601` it does
not.

**Why it matters.** The bug this PR fixes does not live in
`spawn_stream_resource_content`; it lives in the interaction between that function
and the `!warned_stream_unsupported.load(...)` guard at
[mod.rs:648](crates/zendriver/src/monitor/mod.rs:648). Testing the flag is testing
the wire, not the circuit. If someone reorders that condition, drops the `!`, or
moves the guard, both new tests stay green and body streaming silently stops after
the first fast favicon on every page, which is the original reported symptom. Every
other streaming test in the file drives a single request, so nothing covers the
second-request case.

**Fix.** Two end-to-end tests on the existing `spawn_monitor_streaming(None)`
harness. Positive: emit `requestWillBeSent` for r1, answer its
`streamResourceContent` with `-32602`, emit r2, then assert the next
`Network.streamResourceContent` carries `requestId == "r2"`. Negative: same but
answer r1 with `-32601`, emit r2, then assert `try_recv_cmd` yields no
`Network.streamResourceContent`. The first fails against `origin/main`; the second
pins the behavior being kept.

#### 155-4. `write_atomic` creates the temp file world-readable, then narrows it

[persistence.rs:79](crates/zendriver/src/cookies/persistence.rs:79) · **low** · security

**What.** `fs::write(&tmp, bytes)` creates with the process umask (0644 on a stock
box) and `apply_destination_mode` narrows afterwards. For the whole duration of the
write the file holds the complete cookie JSON while world-readable.

**Why it matters.** Confirmed on the branch rather than reasoned about: writing
256 MiB to a destination already chmod'd 0600, with a background thread stat-ing
the `.tmp` sibling, reported modes `["600","644"]` across 1668 samples. For an
existing 0600 destination this is a regression; the `fs::write` this replaced wrote
through the existing inode and never widened anything. Any other local uid can
`open()` the file in that window and keep the fd, and narrowing the mode afterwards
changes nothing, because Unix permission checks happen at `open()`, not per read.
The justification comment for this helper is "don't silently widen a jar the user
deliberately created chmod 600", and on the way to that promise the code does
exactly that.

**Fix.** Create with the restrictive mode instead of fixing it up: on Unix,
`fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)` then
`write_all`. Keep `apply_destination_mode`, which now only ever widens to match an
existing destination. Add a test asserting the temp file's mode never exceeds 0600
while the write is in flight.

**Verification note.** Downgraded from the reviewer's higher rating on
exploitability: this needs another local uid on the box. It is still a regression
against the code it replaced, and it is one line.

#### 155-5. The temp file open has no `O_EXCL` on a predictable path

[persistence.rs:79](crates/zendriver/src/cookies/persistence.rs:79) · **low** · security

**What.** `fs::write` is `O_WRONLY|O_CREAT|O_TRUNC` with no `O_EXCL`, so it follows
an existing symlink and truncates an existing regular file. The name is
`<dest-file-name>.<pid>.<seq>.tmp` with `SEQ` starting at 0 for every process.

**Why it matters.** `save_to_file` takes an arbitrary caller-supplied path, and
`tracker.rs`'s cache root falls back to `std::env::temp_dir()` when
`dirs::cache_dir()` returns `None`. A local attacker who can create files in the
destination directory pre-creates the temp name as a symlink and the process writes
the cookie JSON through to a target of their choosing with your privileges.

**Fix.** `create_new(true)`, which is the same line as 155-4, so fix both together.
The open then fails with `AlreadyExists` instead of following anything.

**Verification note.** Exploitability is low: every variant needs write access to
the destination's directory, which for the primary caller is the user's own, and
Linux `fs.protected_symlinks` blocks the follow in a sticky world-writable
directory. Fix it because it is free, not because it is live.

#### 155-6. The Windows no-op comment overstates what a rename preserves

[persistence.rs:125](crates/zendriver/src/cookies/persistence.rs:125) · **low** · stale-comment

**What.** The `#[cfg(not(unix))]` doc says "Windows carries its ACLs on the
containing directory, so the renamed file picks them up". Windows keeps a DACL on
each file; the directory carries inheritable ACEs that seed newly created files.

**Why it matters.** Narrower than the reviewer claimed, but real: a user who ran
`icacls cookies.json /inheritance:r` set an explicit DACL on the file, and that is
lost when the rename installs a different inode at the path.

**Fix.** One clarifying sentence: the rename installs a new inode, so only the
containing directory's inheritable ACEs are reapplied; an explicit DACL set on the
destination file is lost, and preserving it would need
`GetNamedSecurityInfo`/`SetNamedSecurityInfo` or `ReplaceFileW`, neither of which
is wired up.

**Verification note.** The reviewer's consequence ("the jar silently reopens to
every account on the machine") is not supported. The temp file is created in the
same directory as the destination, so it inherits the same inheritable ACEs, and a
same-volume `MoveFileEx` preserves the moved file's own security descriptor. The
outcome the comment describes is correct for the default case.

#### 155-7. The atomic-rename claim is not scoped to Unix

[persistence.rs:36](crates/zendriver/src/cookies/persistence.rs:36) · **low** · docs

**What.** "A same-directory `rename` is atomic on every platform zendriver targets"
is stated unconditionally. On Windows the underlying call is best-effort rather
than a documented atomic replace, and it fails outright when the destination is
open in another process without `FILE_SHARE_DELETE`, or is read-only.

**Why it matters.** Scoping a claim you cannot verify is cheap; leaving it
unqualified in a module whose whole subject is durability is not.

**Fix.** Scope the sentence to Unix and note the Windows behavior. Do not add the
reviewer's user-facing failure narrative to the `# Errors` section: neither the
reviewer nor the verifier could test Windows, and the claim that a plain
`fs::write` would have succeeded where the rename fails is not reliable (a write
open also fails against a handle that did not grant `FILE_SHARE_WRITE`). This is a
difference in likelihood, not a brand-new failure mode.

#### 155-8. The `-32601` narrowing removed the backstop for other persistent failures

[mod.rs:1009](crates/zendriver/src/monitor/mod.rs:1009) · **low** · correctness

**What.** Narrowing the latch to exactly `-32601` is right for the `-32602` race,
but a persistently failing `streamResourceContent` reporting some other code now
gets one doomed attempt per request forever, and the evidence moved from `warn!` to
`debug!`, invisible under a default subscriber.

**Why it matters.** The visibility half is the real part. The cost half is smaller
than the reviewer argued: the call site guards on
`!streaming.contains(&p.request_id)`, so it is one attempt per request id for
requests the URL filter admits, and a doomed attempt is one small JSON message on
an already-open socket.

**Fix.** Log the persistent case where an operator can see it. A consecutive-failure
counter and latch is a hardening proposal rather than a defect fix; see "Needs a
human".

#### 155-9. Blocking filesystem calls inside an async fn

[tracker.rs:118](crates/zendriver/src/tracker.rs:118) · **low** · correctness

**What.** `std::fs::read_to_string` (line 118) and `std::fs::create_dir_all`
(line 127) sit either side of an `await` on the now-async `write_atomic`. The PR
moved the write to `tokio::fs` and left its neighbours synchronous.

**Why it matters.** Consistency, mainly. The mixed style is a readability trap: a
maintainer sees `.await` on the write and assumes the reads are async too. The
runtime-stall argument is weak at this scale (one read of a roughly 1 MB file, once
per URL per launch).

**Fix.** Use `tokio::fs` for both. While there, the `if let Ok(text)` on the read
swallows every error kind, so a permission-denied cache logs "cache miss" and
re-downloads every launch; match on `NotFound` for the miss path and `warn!`
anything else.

**Verification note.** The reviewer's claim that there is "no log line" is wrong;
there is one at
[tracker.rs:123](crates/zendriver/src/tracker.rs:123), it is just mislabeled as a
miss.

#### 155-10. `tracker_blocklist_url`'s rustdoc does not mention the new bound

[browser.rs:1001](crates/zendriver/src/browser.rs:1001) · **low** · docs

**What.** The rustdoc still says only "Fetched once at launch and cached on disk",
with no mention of the 5s connect / 20s total bound or that exceeding it fails
`Browser::launch()`. [interception.md:173](docs/book/src/interception.md:173) has
the same gap.

**Why it matters.** A user pointing this at a slow-but-alive mirror now gets a
launch failure after 20 seconds with no way to know from the docs that 20 seconds
is a threshold zendriver chose, that it is not configurable, or that the workaround
is to pre-seed the cache or use `tracker_blocklist_file`. CLAUDE.md makes doc sync
a blocking condition.

**Fix.** Two sentences in the rustdoc and one row edit in `interception.md`.

**Verification note.** Launch could already fail on a fetch error before this PR;
what is new is that a slow source is now one of those failures. The doc gap is "a
new failure trigger is undocumented", not "a new failure mode exists".

#### 155-11. `write_atomic` lives in `cookies::persistence` while two worse copies remain

[persistence.rs:52](crates/zendriver/src/cookies/persistence.rs:52) · **low** · architecture

**What.** A general filesystem primitive is reached from `tracker.rs` as
`crate::cookies::persistence::write_atomic`, while
[mod.rs:121](crates/zendriver-fingerprints/src/pool/mod.rs:121) and
[download.rs:42](crates/zendriver-fingerprints/src/generative/download.rs:42) still
carry the old `cache.with_extension("tmp")` + write + rename pattern.

**Why it matters.** The doc is honest that the placement is a compromise ("it lives
here because this module is compiled unconditionally"), which is a fine reason for
a temporary shim. The concrete cost is that `zendriver-fingerprints` is a separate
crate and structurally cannot reach `crate::cookies::persistence`, so those two
copies can never be fixed without moving this code. They also have every defect
this PR just fixed plus one more: `with_extension("tmp")` is not unique per
process, so two zendriver processes starting at once race on the same temp path.

**Fix.** Move `write_atomic` / `apply_destination_mode` / `temp_path` to a
`crate::io` module now (a file move plus two `use` updates). Migrating the two
fingerprints call sites is a larger decision; see "Needs a human".

#### 155-12. The cleanup-on-failure block is written three times

[persistence.rs:77](crates/zendriver/src/cookies/persistence.rs:77) · **low** · readability

**What.** `if let Err(e) = ... { let _ = fs::remove_file(&tmp).await; return Err(e); }`
appears three times verbatim. The cleanup is the invariant and the three steps are
the variable part; the code is written the other way round.

**Why it matters.** Adding the `create_new` open and the `write_all` from 155-4
makes it four or five copies, and the odds that one ends up without the
`remove_file` rise with each.

**Fix.** Put the fallible sequence in an inner async block and handle cleanup once.
Do this in the same edit as 155-4.

#### 155-13. The `& 0o7777` mask is justified by a comment and covered by nothing

[persistence.rs:116](crates/zendriver/src/cookies/persistence.rs:116) · **low** · stale-comment

**What.** The comment promises setuid/setgid/sticky are preserved, and `mode_of` in
the test module masks `& 0o777`, so every bit the mask exists to carry is
untested. A non-privileged `chmod` can also have `S_ISGID` cleared by the kernel,
which the comment does not mention.

**Why it matters.** If someone changes `0o7777` to `0o777` tomorrow, nothing in the
suite notices. The bits themselves are harmless on a non-executable data file, so
this is a docs-and-coverage nit, not a security one.

**Fix.** Add the kernel caveat to the comment, and add one test: chmod a
destination to `0o2600`, run `write_atomic`, assert `mode() & 0o7777 == 0o2600`.

#### 155-14. `write_atomic_cleans_up_temp_file_when_rename_fails` does not check which step failed

[persistence.rs:335](crates/zendriver/src/cookies/persistence.rs:335) · **low** · weak-test

**What.** `let err = write_atomic(...).await.unwrap_err(); let _ = err;` asserts
only that some error occurred. The `let _ = err;` exists solely to quiet an
unused-variable warning that would not fire without the binding.

**Why it matters.** The test's name is a claim about which step failed and the body
cannot tell. It happens to reach `rename` today (metadata on the directory
succeeds, `set_permissions` on the temp file succeeds), but nothing pins that.

**Fix.** Assert the error kind you are naming, and drop the `let _ = err;`.

**Verification note.** The reviewer called this the "green test asserting something
that never happens" shape; it is not. The named property is cleanup, and the test
does assert it (`dir_entry_count(dir.path()) == 1`), correctly, whichever step
failed. Tightening the name-to-assertion link is the whole fix.

#### 155-15. `temp_path` invents a filename instead of rejecting the input

[persistence.rs:138](crates/zendriver/src/cookies/persistence.rs:138) · **low** · correctness

**What.** `path.file_name()` returning `None` falls back to the literal
`"zendriver"`, undocumented and untested. For `path = "/"` this produces
`/zendriver.<pid>.0.tmp`.

**Why it matters.** `file_name()` is `None` only for paths ending in `/`, `.` or
`..`, all of which are directories, so `write_atomic` was always going to fail. The
fallback rescues nothing; it just means the failure happens after creating a stray
file somewhere the caller never named. The cleanup path removes it, so this is
ordering, not hazard. But silently substituting a made-up filename for a path the
caller gave you should either be an error or be written down.

**Fix.** Make `temp_path` return `io::Result<PathBuf>` and fail with
`InvalidInput`. The caller already returns `io::Result`, so it is a `?`.

#### 155-16. A dead `clippy::panic` allow

[tracker.rs:136](crates/zendriver/src/tracker.rs:136) · **low** · idiom

**What.** The test module's allow was widened from `clippy::unwrap_used` to
`clippy::panic, clippy::unwrap_used`, and nothing in the module panics explicitly.
Removing `clippy::panic` and running clippy with `-D warnings` is clean.

**Why it matters.** Blanket allows are load-bearing suppressions; the next reader
sees `clippy::panic` allowed and concludes a bare `panic!` somewhere is sanctioned.
Dead allows accumulate until the module is effectively unlinted.

**Fix.** Revert to `#[allow(clippy::unwrap_used)]`. The sibling modules
(`monitor/mod.rs`, `expect/request.rs`, `expect/response.rs`) genuinely need the
pair; only this one is dead.

#### 155-17. The `warned` parameter is named for a side effect, not for what it decides

[mod.rs:973](crates/zendriver/src/monitor/mod.rs:973) · **low** · naming

**What.** The parameter carries "this browser has no `streamResourceContent`, stop
calling it" and gates every future call, but is named as if it only tracked whether
a log line fired. The call site's local is correctly named
`warned_stream_unsupported`.

**Why it matters.** If the flag reads as "did we warn?", then `if !warned.swap(true)`
looks like log deduplication and it is easy to miss that flipping it permanently
disables a feature.

**Fix.** Rename the parameter to `stream_unsupported`.

**Verification note.** The flag carried both meanings before this PR too; what
changed is when it gets set. This is a drive-by rename while the lines are open,
not a defect the branch introduced.

#### 155-18. A guard's explanation sits five lines below the guard

[mod.rs:652](crates/zendriver/src/monitor/mod.rs:652) · **low** · readability

**What.** The comment explaining the `!warned_stream_unsupported.load(...)` clause
sits inside the `if` body, immediately above an unrelated `streaming.insert(...)`,
so it reads as documentation for the insert.

**Why it matters.** A reader scanning the four-clause condition gets no explanation
where they are looking, then hits one after they have moved past. The updated text
is genuinely useful ("only that error latches the flag"); it is filed in the wrong
place.

**Fix.** Move the block above the `if`, or inline a short pointer on the guard
clause and let the function's rustdoc carry the long version.

#### 155-19. "outlives nothing"

[response.rs:8](crates/zendriver/src/expect/response.rs:8) · **low** · docs

**What.** The module doc ends "so each `expect_response` call is observably
one-shot and outlives nothing". The intended subject is the subscriber task, but
the grammatical subject is the call, so the sentence carries no information.

**Why it matters.** It is the first paragraph a reader of this module sees, and the
property it is fumbling toward is the headline change in the file.

**Fix.** "The subscriber task self-cancels after sending the first match, and exits
the moment the caller drops the expectation, so it never outlives the
`expect_response` that created it." The sibling `expect/request.rs` doc is already
clear, so this is a one-file edit.

#### 155-20. The two `expect::register` bodies and their two new tests are near-duplicates

[request.rs:175](crates/zendriver/src/expect/request.rs:175) · **low** · dry

**What.** `register` in `expect/request.rs` and `expect/response.rs` are
structurally identical 30-line `select!` loops, and the two new drop tests are
byte-identical apart from the word "request"/"response" in their doc comments.

**Why it matters.** Weak. The loops are similar in shape while the bodies differ in
meaning (different event type, different URL field, different constructed struct,
and the response side additionally captures a session handle so `.body()` works
later). `response.rs`'s own module doc says it mirrors `request.rs`, so this is a
documented convention rather than accidental drift.

**Fix.** Probably none. Readability > KISS > DRY resolves toward two obvious loops
over a generic with a closure parameter for two call sites. If the two new tests
bother you, share only their polling body. Recorded so it is not re-raised.

---

### PR #156 — core frame + geo

#### 156-1. The provisional-sibling probe blocks the whole frame-lifecycle loop

[lifecycle.rs:146](crates/zendriver/src/frame/lifecycle.rs:146) · **high** · correctness

**What.** `dead_provisional_siblings(...).await` issues a `Page.getFrameTree`
round-trip inline inside the `Some(ev) = attached.next()` arm of the
`tokio::select!`. While that future is pending the whole loop is parked: no
`frameNavigated`, no `frameDetached`, no registry work. The bound is
`PROVISIONAL_PROBE_TIMEOUT` (5s), not the round-trip latency, because a wedged
probe holds the arm for the full timeout.

**Why it matters.** A `select!` arm is not a background job; the loop cannot
advance past an `.await` inside an arm. Proven rather than inferred: attach F1,
attach F2 (which starts the probe), never reply to the probe, then emit
`Page.frameDetached` for F1. The detach was still unapplied 500ms later and only
landed after 5.0 seconds. Now stack the event source on top:
`Connection::subscribe_raw` is a broadcast channel with `EVENT_BUS_CAPACITY = 1024`
and a `res.ok()` filter whose comment reads "Lagged frames are dropped", and the
bus carries every raw CDP event, not just `Page.frame*`. On a page with Network and
Runtime domains enabled, 1024 events in a five-second stall is easy, and the moment
the subscriber lags the dropped `frameDetached` is gone forever with no log. An
ad-heavy page attaches an iframe, the probe wedges, 1200 network events fly by, and
`tab.frames()` reports a frame that no longer exists for the rest of the tab's
life. Nothing awaits this eviction; it is pure housekeeping and has no business on
the critical path of the loop that owns registry correctness.

**Fix.** Keep the attach arm synchronous (build the `Frame`, take the write lock,
insert), then `tokio::spawn` the sweep with cloned `session`, `frames` and the
attached id. Re-validate inside the spawned task under the write lock before
removing (re-check `parent_frame_id`, `session_id` and `url.is_empty()` at eviction
time rather than trusting the pre-probe snapshot), which closes the
snapshot/evict race for free. Add a regression test: with a probe outstanding, an
unrelated `Page.frameDetached` must be applied within about 100ms. Also fix the
constant's own doc, which currently claims "Bounded so a wedged probe can never
stall the event loop that keeps the registry current" — the bound does not prevent
the stall, it caps it at 5s.

#### 156-2. `FrameInner::name`'s doc says renames are not tracked; this PR tracks them

[mod.rs:70](crates/zendriver/src/frame/mod.rs:70) · **high** · stale-comment

**What.** The doc reads "Captured at construction time; the spec does not currently
track renames after the fact since `Page.frameNavigated` does not carry the name
field." `NavigatedFrameInner` deserializes `name` off `Page.frameNavigated`, and
[lifecycle.rs:156](crates/zendriver/src/frame/lifecycle.rs:156) exists specifically
to backfill it, with two tests.

**Why it matters.** Both halves of the sentence are false, and the same changeset
adds a correct comment 90 lines away saying the opposite. Two comments in one
changeset asserting opposite facts is the failure mode to kill. The practical
damage: the next person to touch frame naming believes `frameNavigated` carries no
name and either re-implements the backfill or removes it as dead code, and the
field is `pub(crate)` so nothing in the type system contradicts them.

**Fix.** Rewrite to describe what happens: set from `Page.frameNavigated`'s
`frame.name` when Chrome supplies a non-empty one; `Page.frameAttached` carries no
name, so entries created from an attach start `None` and are backfilled on the
first navigation; immutable, so a backfill replaces the registry entry rather than
mutating in place. While in the file, two more stale P4-era claims: the `Frame`
struct doc still says sub-frames "arrive via the lifecycle / OOPIF wiring in later
P4 tasks" (both modules are shipped), and the dead-code comment still refers to
"later P4 tasks (T15+T16)".

#### 156-3. Both new geo tests pass against the unfixed code

[geo_resolver.rs:233](crates/zendriver/src/geo_resolver.rs:233) · **high** · weak-test

**What.** `missing_country_code_yields_none` and `non_json_error_page_yields_none`
were run against `resolve()`'s pre-PR body and both passed. The entire behavioural
delta of the file is three new `tracing::warn!` calls, and nothing asserts that any
of them fire.

**Why it matters.** A test that is green against the unfixed code is not coverage;
it is a claim of coverage, which is worse than nothing because it stops the next
reviewer from looking. The base code returned `None` for both inputs via `?`, so
both assertions were already satisfied. `non_json_error_page_yields_none` is also
behaviourally identical to the pre-existing `bad_body_yields_none`: same code path
(`resp.json()` fails), same assertion, differing only in a status code and a body
string that neither test observes. `missing_country_code_yields_none` does at least
pin a branch that had no test before, so it earns its place as a regression guard
even though it does not test this change.

**Fix.** Assert the behavior actually added. Add `tracing-subscriber` as a
dev-dependency, install a capturing layer under
`tracing::subscriber::with_default`, and assert the warning text and level for each
new exit, including `apply_geo_overlay`'s, which has no test at all. Fold
`non_json_error_page_yields_none` into `bad_body_yields_none` as a second case.
Separately, all seven tests in the module repeat the `MockServer::start` +
`Mock::given` + `mount` scaffolding; one `async fn probe(status, body)` helper turns
each into two lines.

#### 156-4. The name backfill orphans every `Frame` handle taken before it

[lifecycle.rs:179](crates/zendriver/src/frame/lifecycle.rs:179) · **medium** · correctness

**What.** The backfill path replaces the registry entry with a brand-new `Frame`
(`Frame::new(...)` + `map.insert`). Any handle a caller already holds keeps
pointing at the old `Arc<FrameInner>`, which no future event touches again: its
`url` is frozen and its `name()` stays `None`.

**Why it matters.** `Frame` is a cheap `Arc` handle and `Tab::frames()` hands out
clones, so callers holding one is the intended usage. Before this PR the lifecycle
task only mutated `existing.inner.url` through the `Arc`, so every held handle
tracked navigations. Measured: after two navigations the registry entry read
`https://host.test/two` while a handle taken right after attach still read `""` with
`name() == None`. That breaks the obvious polling loop
(`let f = tab.frames()...find(...); loop { if f.url().await.contains("/checkout") { break } }`),
which now hangs forever on a named iframe and worked before.

**Fix.** Cheapest correct fix, two lines: write the URL through the old `Arc`
before swapping, so existing handles stay current on the field they poll. The
proper fix is to make `FrameInner::name` a `RwLock<Option<String>>` and
`Frame::name()` async, mirroring `Frame::url()`, so the backfill is an in-place
write and no entry is ever replaced. That is a public API shape change; see "Needs
a human". Add a test that takes a handle before the navigation and asserts it
observes the new URL.

**Verification note.** Exposure is narrower than it first looks: `name_changed` is
only true when a non-empty name differs from the recorded one, which for a named
iframe is the first navigation, so a handle must be taken between `frameAttached`
and that first `frameNavigated` to be orphaned. `frame_by_name` cannot hit it at
all.

#### 156-5. `QuitOutcome::NotPending` claims certainty the transport cannot provide

[browser.rs:2319](crates/zendriver/src/browser.rs:2319) · **medium** · correctness

**What.** The variant is documented as "No quit can be in flight: ... the transport
was already shut down so it was never sent at all", and the arm at
[browser.rs:4426](crates/zendriver/src/browser.rs:4426) logs "Browser.close was
never sent". `TransportError::Shutdown` has three sources and only one of them is
"never sent": the third is `SHUTDOWN_DRAIN_CODE`, which is what an already-written,
pending command gets when the actor is cancelled mid-flight.

**Why it matters.** This is the exact defect the enum was introduced to remove, a
categorical claim about bytes the transport cannot vouch for, reintroduced one
level down. In the drain case the `Browser.close` frame is on the wire and Chrome
may be acting on it; we then skip the grace and SIGTERM a browser that was already
quitting cleanly, while logging a sentence that is untrue. It is reachable from
public API: `Browser::cdp().shutdown()` racing `Browser::close()`.

**Fix.** Keep the classification (skipping the grace when the transport is gone is
the right pragmatic call) and stop asserting the unknowable. Doc: the transport is
gone so no reply can arrive; either the enqueue failed outright or a written
command was drained by a cancelled actor, and `TransportError::Shutdown` covers
both, so this never claims which. Log: "Browser.close did not complete: the
transport shut down; it may never have reached chrome". Consider renaming the
variant to `Unanswerable`, which is what it actually establishes.

**Verification note.** The reviewer's claim that the new test cannot distinguish
the two paths is wrong: it calls `cdp().shutdown()` before `close()`, and the
actor's select is `biased;` with `shutdown.cancelled()` first, so it deterministically
drives the enqueue-failure path. The race is real but the test is not ambiguous.

#### 156-6. `Frame::evaluate` has no stale-context retry while `Tab::evaluate` does

[mod.rs:273](crates/zendriver/src/frame/mod.rs:273) · **medium** · correctness

**What.** `Tab::evaluate` catches `-32000 Cannot find context with specified id`,
clears the cached `context_id` and retries once
([tab.rs:1118](crates/zendriver/src/tab.rs:1118)). `Frame::evaluate` does not, and
nothing on the `Frame` path ever clears a dead context.

**Why it matters.** `IsolatedWorldCache`'s own module doc says the context id is
"invalidated when Chrome reports 'Cannot find context with specified id', typically
after a navigation destroys the previous context" — true only for `Tab`. So:
`let f = tab.frame_by_name("sidebar").await?.unwrap(); f.evaluate::<i32>("1").await?;`
caches ctx 7, the iframe navigates, and `f.evaluate::<i32>("2").await` fails
permanently. `Tab::evaluate` in the same situation recovers transparently. This PR
did not introduce the asymmetry, but it lands squarely in this code, and the rename
branch now papers over it for exactly one subset of frames, which makes the failure
harder to reproduce, not easier.

**Fix.** Mirror the `Tab::evaluate` retry with an attempt counter. Better, hoist
the retry into a shared helper so the two cannot drift again.

#### 156-7. "Fails open in every ambiguous case" is stronger than the code earns

[lifecycle.rs:224](crates/zendriver/src/frame/lifecycle.rs:224) · **low** · correctness

**What.** The doc enumerates no parent, no candidates, probe error and timeout. A
fifth case is not covered: a frame that is genuinely alive but absent from this
session's `Page.getFrameTree` is evicted. The session-id guard only catches entries
already re-homed onto a child session.

**Why it matters.** The OOPIF scenario the reviewer built on this is plausible but
unproven, and its two premises (that the parent-session placeholder stays url-less
forever, and that Chrome omits remote children from the parent target's frame tree)
are both unverified. Blast radius is also narrower than claimed: the OOPIF's real
handle, keyed under `target_id` with the child session, is filtered out by the
session guard and survives; only the parent-side host row would be lost.

**Fix.** Correct the doc's "every". If a real-Chrome check confirms that remote
children are omitted from the parent target's tree, add a grace period (a candidate
must have been url-less for longer than some interval before it is eligible) or an
OOPIF-aware filter. See "Needs a human" for the verification.

#### 156-8. The `frameNavigated` arm escalates to the write lock unconditionally

[lifecycle.rs:165](crates/zendriver/src/frame/lifecycle.rs:165) · **low** · perf

**What.** The base took a read lock for the common in-place URL update and only
escalated for insert-if-missing. The PR opens with `frames.write().await` and holds
it across the nested `inner.url.write().await`. The write lock is only needed for
the new rename branch.

**Why it matters.** Tidiness, mainly. `Page.frameNavigated` fires on document
navigations, not sub-resources, and the guard is held for one uncontended nested
lock acquisition. The reviewer's "fully-concurrent hot path converted into a
serializing one" is not supported by anything measured.

**Fix.** Take the read lock, compute `name_changed`, write the URL through the
`Arc` and continue under the read guard when no rename is needed; drop and escalate
only for the rename, re-looking-up because a detach can land in the gap.

#### 156-9. "Re-bind a rejected `frame_id`" overstates what recovery achieves

[mod.rs:588](crates/zendriver/src/frame/mod.rs:588) · **low** · docs

**What.** `discover_current_frame_id`'s new doc says it re-binds the frame id.
`FrameInner::frame_id` is immutable, so only the isolated-world cache learns the
live id. `Frame::id()` keeps returning the dead id, and `Frame::wait_for_load()`
filters `Page.frameStoppedLoading` on `self.inner.frame_id`, so after a successful
recovery it can never match and always burns the full 30s timeout.

**Why it matters.** "Re-bind" tells the reader the `Frame` is whole again, which is
not true: `evaluate`/`find` work because they route through the isolated-world
cache; anything reading the id directly is still broken. The underlying breakage is
pre-existing (any frame whose id Chrome rewrote has a broken `wait_for_load`), but
this PR is the first thing to document the function and the doc it chose overstates
the result.

**Fix.** Narrow the doc: discover the live frame id to use for
`Page.createIsolatedWorld`; only the isolated-world cache is rebound, and
`Frame::id()` plus anything filtering on it keeps the stale id. Making `frame_id`
interior-mutable is the better fix and a separate change.

#### 156-10. Two more hand-rolled frame-tree walkers

[lifecycle.rs:291](crates/zendriver/src/frame/lifecycle.rs:291) · **low** · dry

**What.** `collect` here and `collect_children` at
[mod.rs:601](crates/zendriver/src/frame/mod.rs:601) both recurse the
`{frame, childFrames}` shape, one collecting every id into a `HashSet` and one
skipping the root and filtering on `parentId`.

**Why it matters.** Modest. Both were added by this PR and both encode the same
protocol fact, so folding them into one private depth-first `walk` is cheap and
gives one obvious place to hang a real-Chrome test.

**Fix.** Add a private `crate::frame::tree` with `walk(root, &mut f)` plus a typed
`FrameNode` projection, and rebuild both on it.

**Verification note.** The reviewer counted five duplicated traversals. Two of the
five (`tab.rs:683`, `tab.rs:1249`) are single root-field reads with no recursion at
all, and rewriting them as `walk(root).next()` would be a readability regression.
Scope this to the two real walkers.

#### 156-11. The PR's central empirical claim is asserted only in a comment

[lifecycle.rs:215](crates/zendriver/src/frame/lifecycle.rs:215) · **low** · test-coverage

**What.** "Chrome drops a provisional frame from the tree the moment it commits the
real one" is the hypothesis the sweep rests on. Every test that exercises it feeds
`MockConnection` a hand-authored `Page.getFrameTree` reply written to match the
claim. The PR touches no test files, so the existing real-browser fixture
(`frame_find_inside_iframe`, which already drives a `srcdoc` iframe under
`--headless=new`, the precise quirk cited in the code) was not used.

**Why it matters.** `MockConnection` validates nothing about CDP shape or
semantics, so the tests prove "if Chrome omits the provisional, we evict it", and
the `if` is the entire hypothesis. The consequence of the hypothesis being false is
mild, though: the probe never evicts and the code fails open, which is safer than
the pre-PR behavior of sweeping unconditionally.

**Fix.** Add a `#[serial]` integration test alongside `frame_find_inside_iframe`
that captures a `Page.getFrameTree` snapshot right after the second attach and
asserts the provisional id is absent. If Chrome no longer reproduces the
double-attach on the pinned version, say so in the doc comment and pin the version
checked. Give the same treatment to the `-32602 "No frame for given id found"`
string the recovery path matches on, which is also pinned only by mock replies.

#### 156-12. Every `frameAttached` in a burst costs a serialized round-trip

[lifecycle.rs:229](crates/zendriver/src/frame/lifecycle.rs:229) · **low** · perf

**What.** `Page.frameAttached` fires before `Page.frameNavigated`, so during a
burst every already-attached sibling is still url-less, which is exactly the probe
trigger. Measured: 5 attaches produced 4 `Page.getFrameTree` round-trips,
serialized because of 156-1.

**Why it matters.** This is 156-1 counted a second time. A `Page.getFrameTree` is a
local socket round-trip, so 20 to 30 of them across a page load is single-digit
milliseconds on a task nothing latency-sensitive awaits.

**Fix.** Spawning the sweep dissolves the serialization; nothing further is needed.
Do not add the reviewer's coalescing cache or the `live_frame_ids` allocation
micro-optimisation. Note the comment above
`lone_attach_does_not_probe_the_frame_tree` ("the common case keeps its
zero-round-trip cost") reads as reassurance that does not survive the second
iframe; reword it.

#### 156-13. The geo overlay warning is prefixed with a builder method the caller may not have used

[browser.rs:1465](crates/zendriver/src/browser.rs:1465) · **low** · docs

**What.** The warning in `apply_geo_overlay` is prefixed `"geo_auto: ..."`, but the
function runs for any resolver including one installed via the public
`geo_resolver(..)`. The comment above also asserts "The resolver has already logged
why it gave up", which only `IpApiResolver` guarantees.

**Why it matters.** Log prefixes are how someone greps their way to a cause at 2am.
A user who wired `.geo_resolver(MyOfflineDb)` and never called `geo_auto()` gets a
warning naming a builder method they did not use, plus a comment promising the
reason is one line above in the log, which for a custom resolver may not be there.

**Fix.** Prefix on the function ("geo overlay: ..."), and soften the comment to
name `IpApiResolver` specifically.

#### 156-14. The module header omits the CDP round-trip it added

[lifecycle.rs:17](crates/zendriver/src/frame/lifecycle.rs:17) · **low** · docs

**What.** The "Wiring" section was updated for the `frameNavigated` name backfill
but not for the attach path gaining a `Page.getFrameTree` call. The
`Page.frameAttached` bullet is byte-identical to base.

**Why it matters.** This header is the map someone reads before touching the file.
Adding a CDP round-trip to an event handler is the most consequential structural
fact about the module now, and it is what makes the loop blockable at all. Someone
debugging why frame events lag reads three pure registry mutations and does not
think to look for a network call.

**Fix.** Extend the `Page.frameAttached` bullet. Write it to survive the 156-1
refactor: after the sweep is spawned, the sentence should say "spawned", not "first
confirm".

#### 156-15. The local `Child` struct's fields are undocumented

[mod.rs:600](crates/zendriver/src/frame/mod.rs:600) · **low** · docs

**What.** The type has a doc comment; its three fields do not.

**Why it matters.** Marginal. It is a function-local struct, so the
document-every-public-item bar does not straightforwardly reach it, and the
reasoning the reviewer wanted recorded there (that `name` is `Option` because
Chrome omits it) is not the reasoning the code implements: the emptiness filter at
[mod.rs:653](crates/zendriver/src/frame/mod.rs:653) is on our own recorded name,
not the candidate's.

**Fix.** Three short field docs if you are in the file anyway.

---

### PR #157 — input and gate

#### 157-1. `check_visible` gained a viewport clause that the query filters cannot satisfy

[actionability.rs:127](crates/zendriver/src/query/actionability.rs:127) · **high** · correctness

**What.** The viewport-intersection clause was added to `check_visible`, which is
shared by the actionability gate and by the public `FindBuilder::visible_only()` /
`FindAllBuilder::visible_only()` filters and `Element::is_visible()`. Every gate
caller was compensated with a `scroll_into_view()` first. The query filters were
not, and cannot be: a `find()` poll loop has no element to scroll to yet (`grep
scroll_into_view crates/zendriver/src/query/mod.rs` returns nothing).

**Why it matters.** These are two different questions wearing one predicate. "Is
this element rendered?" and "is it currently on screen?" coincide only for the
gate, because the gate always scrolls. For a query they come apart badly.
`tab.find().css("a.tos-link").visible_only().one().await?` on a rendered, opaque
footer link now polls for the full 5s timeout and returns `ElementNotFound`,
because nothing scrolled and nothing will. Worse for `find_all`:
`find_all().css(".row").visible_only().many()` on a 200-row table used to return
all 200 rendered rows and now returns only the roughly 15 in the viewport,
silently, with no error, so the caller iterates a truncated list and never learns
why. The PR's own rustdoc on `check_visible` names both non-gate callers, so the
coupling was seen; the consequence was not followed through.

**Fix.** Split the predicate. Keep the five-condition JS as the gate's
`check_visible`, and give the query filters a `check_rendered` that runs everything
except the viewport clause (isConnected, `checkVisibility`, positive bbox,
effective opacity). Point `wait_actionable` at the first and `visible_only` /
`is_visible` at the second. If you would rather keep one predicate, make the clause
a parameter the gate passes `true` and the filters pass `false`, the same shape as
the `position` parameter this PR just threaded through `check_receives_pointer`.
Note one nuance cutting the other way: the PR's own test comment says
"`visible_only(true)` promises offscreen candidates are filtered out", so the
author may have regarded offscreen filtering as intended. Nothing in the rustdoc or
the book says so, and the silent `find_all` truncation is undocumented either way.
See "Needs a human".

#### 157-2. `Element::is_visible()`'s rustdoc describes the old predicate

[reads.rs:295](crates/zendriver/src/element/reads.rs:295) · **high** · stale-doc

**What.** The doc says only "Attached to the document, has a positive bounding box,
and is not hidden via `display`, `visibility`, or `opacity: 0`". The predicate it
calls straight through to now also requires viewport intersection and effective
ancestor opacity at or above 1%, and covers `content-visibility`.
[quickstart.md:111](docs/book/src/quickstart.md:111),
[faq.md:35](docs/book/src/faq.md:35) and
[migration-playwright.md:35](docs/book/src/migration-playwright.md:35) are stale
the same way; the last maps `is_visible()` onto Playwright's `locator.isVisible()`,
which has no viewport requirement.

**Why it matters.** A user reads three conditions, writes
`assert!(footer_link.is_visible().await?)`, and gets `false` for a rendered, opaque
link 2000px down the page, with nothing in the documentation explaining why. The
Playwright mapping is worse than incomplete: it is an active promise of equivalence
to a method that behaves differently, so someone porting a suite gets silent
behavior drift across dozens of assertions. The only doc file this PR touched is
`input.md`, so the repo's three-surface doc-sync gate was not met.

**Fix.** State all five conditions, viewport first because it is the surprising
one, with a pointer to `scroll_into_view`. Update `quickstart.md` and `faq.md` to
say `.visible_only()` also drops off-screen elements. Annotate the
`migration-playwright.md` row so the mapping stops reading as exact. Land after
157-1, because the split decides what `is_visible` should actually promise.

#### 157-3. The screenshot scroll comment contradicts the clip comment fifteen lines below

[screenshot.rs:73](crates/zendriver/src/element/screenshot.rs:73) · **medium** · stale-comment

**What.** Line 73 justifies the scroll with "the clip below is viewport-relative".
Line 91 says "Chrome reads `clip` in DOCUMENT coordinates while `bounding_box` is
viewport-relative", and line 120 does `bbox.x + page_x`. Both cannot be true and
the second is the one the code implements. The line-73 reason is independently
false: viewport-relative bbox plus page offset is the correct document coordinate
whether or not the element is on screen, so scrolling buys nothing for the clip.

**Why it matters.** Comments are the only thing telling the next reader why a line
exists, and these disagree inside one function body, so whichever you read first
you leave with a wrong model. A future contributor reads line 73, sees `+ page_x`,
decides the addition is a bug and deletes it, reintroducing exactly the defect this
PR fixed: a screenshot of an element 3000px down the page cropping the top-left of
the document instead.

**Fix.** Rewrite line 73 to carry only the reason that is still true (the
visibility predicate requires viewport overlap, so gating before the scroll rejects
every below-the-fold element) and delete the clip clause, which line 91 already
owns and states correctly. The module doc at lines 6-8 carries the same false claim
and must be fixed in the same pass, or the fix is half-done.

#### 157-4. `ClickOptions::click_count` is documented as a wire value and is now a loop bound

[actions.rs:122](crates/zendriver/src/element/actions.rs:122) · **medium** · stale-doc

**What.** The rustdoc says "`clickCount` for the CDP dispatch. `1` by default; set
`2` for a double-click in a single `click_with` call." `click_at` now loops
`for n in 1..=click_count.max(1)` and sends `n` as `clickCount` on each pair. The
`.max(1)` also means `click_count: 0` silently performs one click, documented
nowhere.

**Why it matters.** This is a public field on a public options struct, so the doc is
the contract. Someone wanting Chrome's triple-click selection sets
`click_count: 3` expecting one dispatch pair carrying `clickCount: 3`, and gets six
dispatches. A page that increments a counter in its `mousedown` handler now reads 3
instead of 1, and the bug looks like it is in the page. The implemented behavior is
the correct one and is well pinned by
`double_click_emits_two_pairs_with_increasing_click_count`; only the doc is behind.

**Fix.** "Number of press/release pairs to emit. `1` by default; `2` sends the two
pairs with `clickCount` 1 then 2 that Chrome produces for a real double-click. `0`
is treated as `1`."

#### 157-5. `mouse_drag`'s held-button tracking has no test that can observe it

[tab.rs:2250](crates/zendriver/src/tab.rs:2250) · **medium** · weak-test

**What.** Deleting the whole `buttons_held.insert(LEFT)` block and rebuilding leaves
all four `mouse_drag` tests passing, including
`mouse_drag_clears_the_held_bit_when_a_move_fails`. The same mutation applied to
`click_at`'s cleanup does fail its test, so the technique works; the drag tests
simply cannot distinguish "correctly cleaned up" from "never set".

**Why it matters.** `mouse_drag_clears_the_held_bit_when_a_move_fails` asserts
`buttons_held.is_empty()` after a failed move. On `origin/main` the field was never
written, so it was always empty and the assertion was already true before the fix
existed. `mouse_drag_reports_the_button_held_on_every_move` only inspects wire
frames, which carry a hardcoded `LEFT_HELD` constant rather than the tracked state.
The PR added state tracking and shipped it untested while appearing to test it.

**Fix.** Assert the state while it is live: after replying to the `mousePressed`
and before failing the move, assert
`tab.input().state.lock().await.buttons_held.contains(MouseButtonSet::LEFT)`. That
one line fails against the mutation and against `origin/main`, and turns the
existing `is_empty()` assertion into a real before/after pair.

#### 157-6. Both `move_realistic` behavior changes have no test

[mouse.rs:109](crates/zendriver/src/input/mouse.rs:109) · **medium** · test-coverage

**What.** Skipping `path.points[0]` (so a move emits n-1 frames and no longer
re-dispatches a `mouseMoved` to the cursor's current position) and deriving the
per-segment delay from real segment length instead of a fixed 5px assumption. The
only test touching this path drains frames and asserts each one's `type`; it never
counts them and never looks at timing.

**Why it matters.** The arithmetic is correct (`(seg / speed) * 1000.0` is
microseconds, `from_micros` matches, `f64 as u64` truncates sub-microsecond only,
`speed > 0.0` correctly excludes NaN) and the 3x/4x claim in the comment holds,
since `BezierPath` clamps its sample count to [8, 60]. But none of it is pinned.
The next person to touch this loop, say to add the overshoot behavior
`InputProfile::overshoot_rate` already declares, has nothing telling them the first
point must stay skipped, and re-adding it silently reintroduces a duplicate
`mouseMoved` to the origin on every move. That is a behavioral fingerprint (real
cursors do not emit a move to where they already are) in a crate whose purpose is
not looking synthetic.

**Fix.** Two unit tests needing no mock. First: build a `BezierPath` for a known
distance and assert `move_realistic` emits exactly `points.len() - 1` `mouseMoved`
frames, none at the start coordinate. Second: with a stub profile at
`mouse_speed_px_per_ms: 1.0`, drive a 100px move under `tokio::time::pause()` and
assert the advanced clock is about 100ms, which pins total duration to path length
and would have caught the old fixed-5px assumption directly.

#### 157-7. The new `check_visible` tests assert on JS source text

[actionability.rs:416](crates/zendriver/src/query/actionability.rs:416) · **medium** · weak-test

**What.** All five new tests match on substrings of the predicate's source
(`js.contains("effective *= own")`, `js.contains("rect.right <= 0")`,
`!js.contains("offsetParent")`). The sixth feeds the mock a canned boolean and
asserts the Rust side unwraps it, which tests `res.get("value").as_bool()`, not the
predicate.

**Why it matters.** Demonstrated by mutation: swapping the quirks-mode ternary arms
to `document.compatMode === 'BackCompat' ? document.documentElement : document.body`,
which is precisely the defect
`check_visible_probe_reads_the_layout_viewport_in_both_rendering_modes` says it
pins, leaves all six tests green while breaking the predicate in standards mode.
The tests are also brittle in the other direction: renaming a local breaks one of
them without changing behavior.

**Fix.** Be explicit that these are source pins, not behavior coverage: rename with
a `probe_source_` prefix and say in a module comment that the predicate's logic is
covered by the real-browser tier. Then add the real coverage where it can exist. A
live tier already exists to host it
([find_visible_only.rs](crates/zendriver/tests/find_visible_only.rs), gated on the
`integration-tests` feature): a fixture with a below-the-fold div, an
`opacity: 0.001` honeypot, a `visibility: hidden` ancestor, a `position: fixed`
element and a doctype-less quirks-mode page is about 30 lines of HTML and covers
everything these tests gesture at.

**Verification note.** Two of the reviewer's supporting examples were refuted: the
vw/vh transposition mutation does fail
`check_visible_probe_tests_viewport_intersection`, and renaming `effective` breaks
one test, not three. The finding survives on the quirks-mode mutation above.

#### 157-8. `test_support::expect` claims "Every wait goes through here"

[test_support.rs:19](crates/zendriver/src/test_support.rs:19) · **low** · stale-comment

**What.** `grep -rn expect_cmd` over the modules this PR edits returns 169 hits, two
of them added by this PR (`next_dispatch`, `drain_mouse_dispatches`), each wrapping
its own `tokio::time::timeout`.

**Why it matters.** The sentence is the invariant that makes the helper worth
having; the helper exists because `expect_cmd` silently discards non-matching
frames and has no timeout, so a test one frame out of step hangs forever rather
than failing. A reader who believes the claim will not think to bound a new wait.
The claim is also self-defeating: the PR created the helper and added two more
unbounded-by-`expect` waits in the same commit.

**Fix.** Reword it. Rerouting 169 call sites is not the fix; say what is true
("Prefer this for bounded waits on a specific method") and note that
`drain_mouse_dispatches` deliberately uses a shorter timeout as an end-of-stream
signal.

#### 157-9. Space and Enter attach `text` even when Ctrl/Alt/Meta is held

[keyboard.rs:550](crates/zendriver/src/input/keyboard.rs:550) · **low** · correctness

**What.** `let text = special_text(k);` ignores `mods`, so Ctrl+Enter dispatches
with `text: "\r"`. Puppeteer blanks the field when any non-Shift modifier is
pressed.

**Why it matters.** Less than it appears. The consequence was tested against real
Chrome 151 over CDP: Ctrl+Enter with `text: "\r"` into a `<textarea>` preloaded with
"hello" left the value unchanged while the page's keydown handler still received
`{key: "Enter", ctrlKey: true}`; the identical payload without the modifier produced
"hello\n". Ctrl+Space left its input unchanged. Ctrl+A and Meta+A with `text: "a"`
behaved identically to the blanked-text version. Blink suppresses character
generation when a non-Shift modifier is held even when `text` is set, so the
"message sent with a trailing newline" scenario does not occur.

**Fix.** Optional cosmetic normalization to match Puppeteer:
`let text = if mods.difference(KeyModifiers::SHIFT).is_empty() { special_text(k) } else { None };`,
with the same rule on the `Key::Char` branch. No behavior change in Chrome. See
"Needs a human".

#### 157-10. `Page.getLayoutMetrics` missing `cssVisualViewport` degrades silently

[screenshot.rs:102](crates/zendriver/src/element/screenshot.rs:102) · **low** · correctness

**What.** `page_x` and `page_y` fall back to `0.0` via `unwrap_or(0.0)` with no log
line, and the clip reverts to viewport coordinates, which is the exact behavior this
hunk exists to fix.

**Why it matters.** Silent degradation to the bug you just fixed is the worst
available failure mode, because the symptom (a screenshot cropped from the wrong
band) looks like a product bug and nothing in the logs points at a missing CDP
field. It is also inconsistent within this PR: `wait_for_load` gained a
`tracing::warn!` forty lines away for precisely this reason, with a comment saying
swallowing it made a failed evaluate indistinguishable from a not-yet-complete
page. The trigger is not reachable on any supported browser (the crate pins Chrome
121+ and this field predates that by years), so this is defensive-logging
consistency.

**Fix.** Warn on the fallback. Optionally fall back to
`cssLayoutViewport.pageX/pageY` first, which carries the same document offset for
any page that is not pinch-zoomed.

#### 157-11. The effective-opacity loop walks to the root after it can already answer

[actionability.rs:177](crates/zendriver/src/query/actionability.rs:177) · **low** · perf

**What.** The loop multiplies every ancestor's opacity and only compares against
the threshold after it ends. Opacity is in [0, 1], so the running product only
decreases and cannot recover once it is below 0.01.

**Why it matters.** Marginal, and less than the reviewer argued. The early exit
fires only on the reject path; for the normal case (every ancestor at opacity 1)
the loop still walks to the root either way, so the "tens of thousands of
`getComputedStyle` calls" scenario is unchanged by the fix. The whole loop is one
JS round trip per poll and the round trip dominates it.

**Fix.** One line if you are in the file: move the threshold check inside the loop
and drop the trailing one. Update
`check_visible_probe_multiplies_ancestor_opacity_against_a_threshold`, which
matches on the exact source strings.

#### 157-12. Four spellings of "wait for the next CDP frame, bounded"

[mouse.rs:290](crates/zendriver/src/input/mouse.rs:290) · **low** · dry

**What.** `test_support::expect` (5s), `next_dispatch` (5s),
`drain_mouse_dispatches` (500ms) and the inline loop in
`mouse_move_emits_mousemoved` (500ms), three of which this PR added. There are at
least six such timeout-wrapped loops in the tab tests alone.

**Why it matters.** The PR's stated reason for creating `test_support.rs` is that a
sequence several tests replay should be written down once, and then it wrote the
same wait three more ways in the same commit. The bounds differ with nothing
recording why, so the next person picks whichever they copy from, and when the
bound needs raising for a slow CI runner the copies that are missed hang or flake
and look like product bugs.

**Fix.** Rebuild `next_dispatch` on `expect`. Give `test_support` a second helper
for the silence-as-terminator case (`try_expect(mock, method, timeout) -> Option<u64>`)
so the intent is named rather than inferred from the number, and port the inline
loops onto it.

#### 157-13. `serve_gate_probes` takes a bare count whose enumeration is already incomplete

[test_support.rs:63](crates/zendriver/src/test_support.rs:63) · **low** · stale-doc

**What.** The doc says "4 for `FULL`, 2 for `TEXT_INPUT`, 1 for `VISIBLE_ONLY`",
and this PR calls it with 3 for the hover test's ad-hoc check set. The parameter is
a bare `usize`, so a wrong count is not a compile error.

**Why it matters.** The helper's contract is the enumeration in its doc, and the
enumeration was incomplete on the day it shipped. A wrong count does not fail
cleanly: it consumes the next unrelated `Runtime.callFunctionOn`, so a subsequent
`expect` waits for a frame that was already eaten and you get "timed out waiting for
DOM.getBoxModel" pointing at a line that is not the problem.

**Fix.** Take the check set instead of a count:
`serve_gate_probes(mock, require: ActionabilityCheck)` computing `n` internally. The
count and its enumeration then disappear from the doc. Pairs naturally with 157-19.

#### 157-14. `clear_by_deleting`'s doc describes a fixed keystroke count

[actions.rs:589](crates/zendriver/src/element/actions.rs:589) · **low** · stale-doc

**What.** Step 3 says read `value.length` then press Backspace that many times plus
slack, bounded by a max. The loop now re-reads the value every 16 strokes and
returns as soon as the field reports empty.

**Why it matters.** Anyone building a test that counts `Input.dispatchKeyEvent`
frames from the doc's formula gets a mismatch on any field longer than 14
characters. It also hides the thing a caller might need to know: if the page's
`value` getter under-reports (a custom element backing its text with a shadow-DOM
node), the method now stops early and leaves the field partly filled.

**Fix.** Add a fourth step naming `PROBE_EVERY_N_BACKSPACES` and the early return,
and note that a page whose `value` getter under-reports will stop early.

#### 157-15. `next_dispatch`'s doc names a parameter that does not exist

[mouse.rs:288](crates/zendriver/src/input/mouse.rs:288) · **low** · stale-comment

**What.** "assert the frame is the expected `type`, then hand it to `respond`". The
function takes `mock` and `expected_type` and returns the frame id; there is no
`respond`.

**Why it matters.** A leftover from an earlier callback-taking signature that sends
the reader looking for a parameter that is not there. Small on its own; it is the
fourth comment in this diff asserting something the adjacent code does not do,
which is the pattern rather than the instance.

**Fix.** "...and return its id so the caller can reply."

#### 157-16. `buttons_held`'s doc says the drag path sends it; the drag path does not read it

[mod.rs:54](crates/zendriver/src/input/mod.rs:54) · **low** · stale-doc

**What.** The field doc says it is "written by every press/release and by the drag
path, and sent as CDP's `buttons` bitmask on every dispatched mouse event".
`Tab::mouse_drag` writes the field but puts a locally-declared `LEFT_HELD` (and a
literal `0` on release) on the wire.

**Why it matters.** Loose about provenance rather than false about the wire value:
`LEFT_HELD` is `MouseButtonSet::LEFT.bits()`, so it is by construction what the
field would yield for a left-only hold. They can only disagree if a second button
is held concurrently from another task.

**Fix.** Either route `mouse_drag` through `buttons_held.bits()` (preferred, and it
removes `LEFT_HELD`) or reword the sentence to say the drag path writes it for
coherence and emits the left bit directly.

#### 157-17. `mouse_drag` reimplements `click_at`'s skeleton, including its rationale comment

[tab.rs:2216](crates/zendriver/src/tab.rs:2216) · **low** · dry

**What.** The held-button bookkeeping, the fallible-section-plus-single-cleanup
structure and the pointer-cache update are written twice, and the roughly 18-line
comment explaining why a `Drop` guard was rejected is duplicated near word for word
at [tab.rs:2219](crates/zendriver/src/tab.rs:2219) and
[mouse.rs:196](crates/zendriver/src/input/mouse.rs:196).

**Why it matters.** Duplicated prose rots asymmetrically: someone who later adds the
`Drop`-guard escape hatch will update one copy of that argument and leave the other
asserting the opposite conclusion.

**Fix.** Move the rationale to one place (the `buttons_held` field doc in
`input/mod.rs` is the natural home) and replace both inline copies with a pointer.
Longer term the press/hold/release skeleton wants to be one helper in `mouse.rs`.

**Verification note.** The reviewer's primary consequence (that the wire value and
tracked state could diverge if `MouseButtonSet::LEFT` changed) is wrong: `LEFT_HELD`
is derived from that constant, not hardcoded.

#### 157-18. `js_params` and a weaker equivalent assertion coexist

[actions.rs:904](crates/zendriver/src/element/actions.rs:904) · **low** · dry

**What.** `js_params`, which parses a declaration's parameter list, lives in
`element/actions.rs`'s test module. `query/actionability.rs` checks the same
invariant with `js.trim_start().starts_with("function(el")`.

**Why it matters.** Two mechanisms for one invariant, at two levels of strictness.
`starts_with` accepts `function(elephant, dx, dy)` and cannot verify the rest of
the list, so `function(el, dy, dx)` with coordinates transposed passes; `js_params`
would catch it.

**Fix.** Move `js_params` into `test_support.rs` and have the actionability test
assert the full expected list. Marginal (the actionability assertion covers four
declarations at once), but cheap and strictly better.

#### 157-19. `wait_actionable` gained a fourth positional parameter

[actionability.rs:304](crates/zendriver/src/query/actionability.rs:304) · **low** · readability

**What.** `position: Option<(f64, f64)>` at the end. Five of six call sites now read
as a bare `None,` on its own line.

**Why it matters.** `Option<(f64, f64)>` could plausibly be a timeout override, a
frame or a hit-test point, and the call site says nothing. It is `pub(crate)`, so
this is a readability cost rather than an API one.

**Fix.** Fold the position into `ActionabilityCheck` as a `hit_point` field, so the
presets stay `ActionabilityCheck::FULL` and the click path writes
`ActionabilityCheck { hit_point: opts.position, ..FULL }`. The `None`s disappear
from the other five sites. Composes with 157-13 and 157-20.

#### 157-20. The hover check set has no name

[actions.rs:327](crates/zendriver/src/element/actions.rs:327) · **low** · dry

**What.** `hover` and `hover_fast` each construct the same inline
`ActionabilityCheck { visible: true, stable: true, enabled: false, receives_pointer: true }`
while `FULL`, `VISIBLE_ONLY` and `TEXT_INPUT` exist as named presets.

**Why it matters.** The knock-on shows up in the tests: `serve_gate_probes(&mut mock, 3)`
encodes the probe count of an unnamed set, and `serve_gate_probes`'s doc enumerates
only the three named presets, so the hover configuration is the one nobody can
refer to by name in the code or in the helper.

**Fix.** Add `pub(crate) const HOVER: Self` with the existing rationale (hover does
not activate the element, so disabled controls still accept mouseover) as its doc,
and use it at both sites.

#### 157-21. Enter is documented as inserting a newline unconditionally

[keyboard.rs:31](crates/zendriver/src/input/keyboard.rs:31) · **low** · docs

**What.** `SpecialKey`'s rustdoc says Space and Enter "type `" "` and a newline",
and [input.md:238](docs/book/src/input.md:238) says "`Enter` inserts a newline (and
submits a form that allows implicit submission)". Enter inserts a newline in a
`<textarea>`; in a single-line `<input>` it inserts nothing and only triggers
submission. Confirmed against live Chrome.

**Why it matters.** Both sentences are unconditional, so a reader concludes
`press(Key::Special(SpecialKey::Enter))` on a search box appends a newline to the
query. The same PR gets it right in a test comment in the same file, which is the
accurate version; the public-facing copies are the loose ones.

**Fix.** Port the accurate sentence up into the rustdoc and `input.md:238`.

#### 157-22. `force_char` types a special key's name as literal text

[keyboard.rs:538](crates/zendriver/src/input/keyboard.rs:538) · **low** · correctness

**What.** `special_text(k).unwrap_or(key_str)` means a char event for Tab carries
`text: "Tab"`, F5 carries `"F5"`, Escape carries `"Escape"`.

**Why it matters.** Latent only. `KeyPress::Char` has no non-test constructor at
all: the enum's own doc says so and it carries `#[allow(dead_code)]`, and the emoji
fallback the reviewer named as a live constructor is a different branch. Keeping the
contract total is the right instinct; the total answer chosen is a wrong one rather
than a neutral one.

**Fix.** Return an empty event vector for a non-printable special on the char path,
with a comment noting the branch is unreachable from any public API today.

#### 157-23. `IDLE_FALLBACK_TICK`'s doc undercounts its prose copies

[tab.rs:48](crates/zendriver/src/tab.rs:48) · **low** · stale-doc

**What.** The doc says the constant is the "+50 ms" in the documented worst case
"so the two must move together". Five prose sites spell 50ms
([tab.rs:1797](crates/zendriver/src/tab.rs:1797), 1806, 1809, 4272, 4287) and none
references the constant.

**Why it matters.** The extraction moved the magic number out of the code and left
copies in the prose, so someone who changes the constant to 25ms and dutifully
updates the site the doc implies still leaves the rest claiming 50ms.

**Fix.** Replace the number with the constant name at the two contract sites
(1797, 1806). Leave 4272 and 4287, which are test-scenario arithmetic where the
constant name would read worse.

#### 157-24. `tap_at`'s new pointer-cache write has no test

[touch.rs:45](crates/zendriver/src/input/touch.rs:45) · **low** · test-coverage

**What.** `tap_at` now writes `pointer_x` / `pointer_y` after a successful tap.
`tap_dispatches_touchstart_with_point_then_touchend_empty` checks the two dispatched
frames and stops.

**Why it matters.** The change is correct and its comment's reasoning is sound (a
following `move_realistic` would otherwise build its Bezier from a stale origin and
open by teleporting back to the last mouse position, a visible non-human artifact).
But it is three lines with no guard in a file the mouse work does not otherwise
touch, so it is the easiest thing in this diff for a later refactor to drop. The
equivalent write in `mouse_drag` does have an assertion, so the omission is
inconsistent rather than deliberate.

**Fix.** Two lines on the existing tap test asserting the cached cursor matches the
tap point. Use whichever coordinates that test actually taps.

#### 157-25. `click_at`'s cleanup clears a held bit it may not own

[mouse.rs:267](crates/zendriver/src/input/mouse.rs:267) · **low** · correctness

**What.** The single-exit cleanup does an unconditional
`buttons_held.remove(bit)` without checking whether this call set it.

**Why it matters.** Low-probability. Task A starts `mouse_drag`, which sets LEFT
and is mid-interpolation; task B calls `click_fast` with the default left button
and its cleanup clears LEFT. A third task's `move_realistic` then reports
`buttons: 0` while the page believes the button is down. `InputController` is
per-tab and driving one tab's mouse from two tasks is already asking for trouble.
(Task A's own drag frames are unaffected, because `mouse_drag` hardcodes
`LEFT_HELD` rather than reading the field.)

**Fix.** Record `let we_latched = !s.buttons_held.contains(bit)` inside the press's
locked section and gate the final `remove` on it. Or state the constraint out loud
in one line on `click_at` so the next reader knows it is a decision.

---

### PR #158 — visibility and tests

#### 158-1. The only test for the selectors fix drives a resolver production never reaches

[selectors.rs:1143](crates/zendriver/src/query/selectors.rs:1143) · **high** · weak-test

**What.** `text_exact_folds_whitespace_in_the_needle_before_building_the_xpath`
calls `SelectorKind::resolve_one` → `resolve_text_one`. Lines 565-567 directly above
that function say "test-only: production `.one()` resolves via `resolve_many_inner`
+ take-first; this single-match resolver is exercised only by this file's
`#[cfg(test)]` tests", and it carries `#[allow(dead_code)]`. `query/mod.rs` calls
only `resolve_many_inner`. The identical fix in the live `resolve_text_many` has no
test at all.

**Why it matters.** Verified: deleting the 12-line fold from `resolve_text_many`,
the path that actually ships, leaves `cargo test -p zendriver --lib query::` at 61
passed, 0 failed, including the test named after the behavior. This is the failure
mode the whole branch series was opened to fix, reproduced inside the PR meant to
be the proof. A reviewer sees a passing test named after the behavior and stops
looking. The junior-facing rule: before writing a test, trace who calls the
function you are about to assert on. If the answer is "only tests", the test proves
nothing about what users run.

**Fix.** Re-point the test at `resolve_many_inner`. Then delete `resolve_text_one`
outright: it has zero production callers and its existence is what made the
mistake possible. That also dissolves 158-9.

#### 158-2. The JS normalizer ends in `.trim()`, which strips more than XPath does

[predicate.rs:41](crates/zendriver/src/query/predicate.rs:41) · **medium** · correctness

**What.** `JS_NORMALIZE_SPACE` is
`.replace(/[ \t\r\n]+/g," ").trim()`. ECMAScript's `WhiteSpace` production includes
U+00A0, VT, FF, U+FEFF and every Unicode space separator, so `trim()` removes them
from the ends of the page-side string. The Rust needle folder deliberately
preserves them.

**Why it matters.** The point of the change is that both sides of the `===` get
folded identically, and they do not. Verified in node (same V8 as Chrome): input
codepoints `a0,4f,4b` come out as `4f,4b` (leading NBSP stripped), `feff,61` comes
out as `61`, and interior `61,a0,62` is preserved. XPath `normalize-space()` does
not strip NBSP, so on `<b>&nbsp;OK</b>`, `text_exact("\u{a0}OK")` matches and
`text_equals("\u{a0}OK")` does not. That is the same divergence the change set out
to remove, moved from interior runs to the edges, and `&nbsp;` in button and label
text is common.

**Fix.** Replace `.trim()` with an XPath-faithful edge strip. After the collapse any
leading or trailing run is exactly one space, so
`.replace(/[ \t\r\n]+/g," ").replace(/^ | $/g,"")` matches `normalize_space` byte
for byte. Add a test asserting the emitted JS contains no bare `.trim()`.

**Verification note.** Two corrections. The doc at lines 53-57 is specifically about
the collapse class and is accurate (widening the replace class would break interior
NBSP, which the code correctly preserves); the false claim is the broader one at
lines 46-48, "so both operands of a `text_equals` compare are normalized the same
way". And this is not a regression: the pre-PR filter was `TXT.trim()===needle`, so
the edge behavior already existed. What is new is a doc that overclaims the
guarantee.

#### 158-3. `TextPred::Equals` claims agreement with XPath while comparing `innerText`

[predicate.rs:31](crates/zendriver/src/query/predicate.rs:31) · **medium** · stale-comment

**What.** The new doc says "Both sides are normalized, so this agrees with the
`text_exact` selector kind, which compiles to XPath
`//*[normalize-space(.)=<needle>]`." The JS reads
`(el.innerText||el.textContent||"")`, which is not the string XPath compares
against; XPath's `normalize-space(.)` operates on the node's string-value, the
concatenation of every descendant text node.

**Why it matters.** The two are different strings for ordinary markup, so the
selectors still disagree even after 158-2 is fixed. The robust case:
`<div>a<span style="display:none">b</span></div>` gives `innerText` `"a"` and
string-value `"ab"`, so the two selector kinds match different elements. Block
boundaries differ too. Someone reading this doc will switch between selector kinds
expecting identical results and get silently different match sets, which in a
scraper means silently missing rows. Note the source itself flips: `innerText ||
textContent` falls back to `textContent` whenever `innerText` is empty, so the
divergence is state-dependent, not just markup-dependent.

**Fix.** Either state the residual divergence honestly (agrees on whitespace
folding, still differs because this reads rendered `innerText` while XPath reads
the raw string-value) or make it true by using `el.textContent` for the `Equals`
arms only, leaving `Contains`/`Matches` on the current source. Either way add a
unit test pinning the chosen source so the next edit cannot drift.

#### 158-4. `Element::is_visible`'s rustdoc still describes the pre-rebuild predicate

[reads.rs:293](crates/zendriver/src/element/reads.rs:293) · **medium** · stale-comment

**What.** Same defect as 157-2, seen from this PR. The doc lists three conditions;
the predicate requires five. This PR's own integration test
`is_visible_rejects_a_below_the_fold_field_on_a_doctype_less_page` asserts
`!is_visible()` for an `<input>` where every stated condition holds.

**Why it matters.** It is the docs.rs-rendered surface, so it is the version users
see, and it now denies the existence of the clause that changes their answer. The
accurate description does exist, on `check_visible`'s own rustdoc, but that is not
what users read.

**Fix.** One rewrite, shared with 157-2. Land it there.

**Verification note.** The predicate rebuild and this doc both belong to the parent
commit; #158 only adds a test that pins the new behavior. Fix in #157, not here.
Also, the reviewer's aside that `FindBuilder::visible_only`'s doc "says only
offscreen" is backwards: `visible_only` does say offscreen, and that is precisely
the word `is_visible` is missing.

#### 158-5. `Element::screenshot` does not send the flag the new module doc says every clip gets

[mod.rs:53](crates/zendriver/src/screenshot/mod.rs:53) · **medium** · correctness

**What.** The new module doc says "Any capture carrying a `clip`, full-page or a
caller's own rect, also sends `captureBeyondViewport: true`", and names
`Element::screenshot` two lines up as a clip-carrying capture.
[screenshot.rs:113](crates/zendriver/src/element/screenshot.rs:113) builds its
params as `{format, clip}` with no flag.

**Why it matters.** By this PR's own premise (a clip covering unrendered area comes
back blank), element screenshots of anything taller than the viewport are still
part-blank: a 3000px `<article>` in a 1080px viewport scrolls into view, the clip
asks for all 3000 rows, and roughly a third are rendered. The user gets a PNG with
a band of content and the rest empty, with no error. `Element::screenshot` does
scroll first, so only elements taller than the viewport are exposed.

**Fix.** Add `"captureBeyondViewport": true` to `Element::screenshot`'s params and
assert it in the existing
`screenshot_sends_page_capturescreenshot_with_clip_matching_bbox`. Longer term,
route `Element::screenshot` through `ScreenshotBuilder::clip` so one place decides
clip params. If it is deliberately left out, narrow the doc to name
`ScreenshotBuilder`.

**Verification note.** The flag was never sent on that path, so this is a
pre-existing gap the new doc newly claims to have closed, not a regression. The
blank-band consequence follows from the PR's own premise about Chrome, which was
not independently verified against a real browser.

#### 158-6. XPath needles are quoted by swapping double quotes for single ones

[selectors.rs:542](crates/zendriver/src/query/selectors.rs:542) · **medium** · correctness

**What.** All four builders use `JSON.stringify(n).replace(/"/g,"'")`.

**Why it matters.** `text_exact("it's")` builds `//*[normalize-space(.)='it's']`
(verified in node), the string literal terminates at the third quote,
`document.evaluate` throws a SyntaxError, and the caller gets a `JsException`
instead of a match or an empty result. Apostrophes are everywhere in real button
and link text. The double-quote case is broken too: `JSON.stringify` escapes an
embedded `"` as `\"`, the replace turns that into `\'`, and XPath 1.0 has no
backslash escapes, so `say "hi"` is equally malformed. Junior-facing rule:
quote-swapping is never a correct escaping strategy, because it just moves which
character breaks you.

**Fix.** `document.evaluate` has no parameter binding, so build the literal with
the XPath 1.0 `concat()` trick, which is the safe general answer for strings
containing either quote kind. Add unit tests with `it's` and with `say "hi"`.

**Verification note.** Pre-existing; the PR does not touch these builders. Fixing it
here is a scope judgement, but the PR is the one editing the needle handling that
feeds them.

#### 158-7. The regression-test header and the commit message describe the wrong commit

[element_world_regressions.rs:4](crates/zendriver/tests/element_world_regressions.rs:4) · **medium** · docs

**What.** The header opens "Every case below shipped broken" and lists five
defects. Four are fixed in `actionability.rs` / `keyboard.rs` / `tab.rs`, none of
which are in this commit; they are in the parent. Case 1 (a per-tab isolated-world
context cache going stale across a navigation) is not fixed anywhere in this tree,
because element reads do not use the isolated world: `attr` / `inner_text` route
through `call_on_main` → `Runtime.callFunctionOn` on the element's own handle, so
there is no cached `contextId` to go stale. The commit message has the same
mismatch: its title is "fix(query): rebuild the visibility predicate", the rebuilt
predicate is not in the commit, and the screenshot change that is in the commit
goes unmentioned.

**Why it matters.** On a stacked PR this cross-wires the review. Someone opening
#158 reads a detailed rationale for `check_visible` and cannot find the code;
someone opening the parent gets an input-focused message and no explanation for the
397-line `actionability.rs` change they are being asked to approve. The header also
walks itself back at line 68 ("they stay meaningful when the isolated-world move is
re-landed"), which makes case 1 a forward guard against re-landing a reverted
change, a fine thing to have and a different thing from what the header claims.

**Fix.** Move the `check_visible` paragraphs of the commit message onto the parent
commit and let this one describe what it contains (text normalization, the
screenshot flag, the tests). In the header, name which cases shipped broken and
which are forward guards, and say that the fixes land in the parent commit of the
stack.

#### 158-8. The needle-folding block is duplicated verbatim

[selectors.rs:574](crates/zendriver/src/query/selectors.rs:574) · **low** · dry

**What.** A six-line comment plus a five-line deferred-initialization shadow
(`let normalized; let needle = if exact { normalized = ...; normalized.as_str() } else { needle };`)
appears identically in `resolve_text_one` and `resolve_text_many`. The
`let normalized;` trick is also not how Rust expresses this.

**Why it matters.** Two copies of a rule means two places to fix, and 158-2 is about
to change the rule. When you find yourself pasting a comment as well as the code,
that is the signal to extract: the comment being worth repeating means the rule is
worth naming.

**Fix.** Deleting `resolve_text_one` (158-1) removes one copy and the problem with
it. If it stays for any reason, extract
`fn exact_needle(needle: &str, exact: bool) -> Cow<'_, str>` and carry the comment
on it once.

**Verification note.** The reviewer's causal claim (that the duplication produced
158-1's coverage hole) is backwards: the hole comes from the test being pointed at
the dead resolver, and a shared helper called from both sites would still have been
tested only through that caller.

#### 158-9. `call_on_main_binds_the_element_as_the_first_argument` passes no other arguments

[mod.rs:338](crates/zendriver/src/element/mod.rs:338) · **low** · weak-test

**What.** The test and its doc claim the element is bound "ahead of the caller's
`args`", but the call passes `json!([])`, so the ordering is never exercised.

**Why it matters.** The property that matters is the order, because that is the
contract `check_receives_pointer(el, dx, dy)` depends on. Swap the two lines in
`call_on_main` so the extras are pushed before the handle and this test still
passes, while every multi-argument probe receives its arguments shifted by one:
`dx` would arrive holding the element handle, `Number.isFinite` would reject it,
and the hit test would quietly fall back to the centre on every positioned click.

**Fix.** Pass a real extra argument and assert its position: element at
`arguments[0]`, the caller's value at `arguments[1]`.

**Verification note.** `call_on_main`'s body is identical to `origin/main`, so this
is characterization coverage, not regression proof. Fine, as long as nobody reads
it as the latter.

#### 158-10. The whitespace fixture couples an input test to the visibility fix

[element_world_regressions.rs:527](crates/zendriver/tests/element_world_regressions.rs:527) · **low** · weak-test

**What.** The fixture carries a `height:6000px` spacer that puts `#line` far below
the fold, under `StealthProfile::spoofed()`, which is exactly the configuration
another test in the same file exists to cover. The module doc says of this very
defect: "Nothing about it is page-shaped: the stall and the crash reproduce
identically on a page with nothing to scroll."

**Why it matters.** `type_text` focuses first, focus runs the actionability gate,
and the gate's on-screen clause is what the other two tests check, so a regression
in `check_visible` turns this test red with a message about whitespace and sends the
next person to `keyboard.rs` for a bug that lives in `actionability.rs`. Independent
tests should fail for one reason each; that is what makes a red suite readable.

**Fix.** Either drop the spacer so the field is on screen at load, or keep it and
say in the fixture comment why the coupling is deliberate. The same module doc notes
the bug first surfaced as a CDP timeout on a below-the-fold field, so reproducing
the original conditions is defensible; it is just not written down.

#### 158-11. Two more hand-rolled bounded waits

[mod.rs:538](crates/zendriver/src/screenshot/mod.rs:538) · **low** · dry

**What.** `crate::test_support::expect` (which `reads.rs` and `element/mod.rs` are
migrated onto in this same diff), a hand-rolled `tokio::time::timeout(...).await.expect(...)`
in the new selectors test, and a hand-rolled `match tokio::time::timeout(...)` here
that reproduces `expect`'s body verbatim.

**Why it matters.** The PR establishes `expect` as the one way to do this and then
adds two tests that bypass it. That is how a convention dies in the first week: the
next author copies whichever neighbour they opened.

**Fix.** Import `crate::test_support::expect` in both new tests. It is `pub(crate)`
and already imported elsewhere in this diff, so it is a one-line change each.

#### 158-12. `captureBeyondViewport` is now sent for on-screen clips too

[mod.rs:388](crates/zendriver/src/screenshot/mod.rs:388) · **low** · correctness

**What.** The flag insertion moved out of the full-page branch into the
`effective_clip` branch, so a caller clipping a rect already inside the viewport now
gets it where they previously did not. The flag makes `position: fixed` content
render at its document position rather than pinned to the viewport.

**Why it matters.** Existing callers doing the common thing get different pixels
with no error: a page with a fixed header, scrolled to y=2000, clipping the region
under it, previously contained the sticky header where the user sees it and now
does not. The change is deliberate and is documented in the same commit (an in-code
comment and the new module doc both spell out the side effect), so the residual gap
is narrow: the commit message never mentions the screenshot change at all, so
release-plz will generate no changelog line for a user-visible behavior change.

**Fix.** Get it into the changelog. The module doc text already written is most of
the entry.

#### 158-13. The regression-test header numbers its cases 1, 2, 3, 5, 4

[element_world_regressions.rs:44](crates/zendriver/tests/element_world_regressions.rs:44) · **low** · readability

**What.** Item 5 ("The on-screen clause in quirks mode") is written before item 4
("Whitespace in `type_text`").

**Why it matters.** This is the file's map: five numbered defects, five tests, read
in order to work out which test covers what. Item 5 also opens "Its replacement
read `document.documentElement.clientHeight`", where "its" refers to item 3, which
is presumably why it drifted.

**Fix.** Renumber to 1-5 and put the quirks-mode item immediately after the
spoofed-viewport item with a lead-in that names the relationship, rather than
encoding it in a number.

#### 158-14. A negative assertion that covers only one of two shapes

[selectors.rs:1173](crates/zendriver/src/query/selectors.rs:1173) · **low** · weak-test

**What.** `assert!(!expr.contains("Hello \\t"))` checks for the JSON-escaped form a
raw tab takes after `json!(needle)`. It does not catch a literal tab byte, and it
takes a second read to work out which form is being tested.

**Why it matters.** The assertion means "no unfolded whitespace survived into the
XPath" but covers one of the two ways that could look. If the embedding ever changed
to splice the needle without JSON-encoding it, a literal tab would sail through
while the positive `contains("Hello world")` also passes for a different substring
of the same expression.

**Fix.** Assert the exact fragment the builder should emit and drop the negative
check, which the positive then subsumes.

#### 158-15. `normalize_space` allocates a `Vec` to feed a `join`

[predicate.rs:58](crates/zendriver/src/query/predicate.rs:58) · **low** · perf

**What.** `s.split(...).filter(...).collect::<Vec<_>>().join(" ")`.

**Why it matters.** It does not; this runs once per query compile.

**Fix.** Optional, and closer to a wash than it looks: the split/filter/join form
states the rule in one readable line while a manual fold needs an is-first branch.
Take it only if you are already editing the function.

---

## 3. Fix order

One ordered list across all six PRs. Items in the same phase are independent and
can be parallelised unless a dependency is named. #158 is stacked on #157, so
#157's fixes land first in that branch and #158 rebases; the other four PRs are
independent branches and their phase-1 work can all proceed at once.

### Phase 1 — blocking correctness and false public contracts

1. **#156-1** Move the provisional-sibling probe off the `select!` arm
   (`frame/lifecycle.rs`). Highest-value fix in the set: it is the only one whose
   failure mode is silent registry corruption. Unblocks 20 and 33.
2. **#157-1** Split `check_visible` into a gate predicate and a rendered predicate
   (`query/actionability.rs`). Unblocks 12, 21, 27. Do this before touching any
   `is_visible` documentation, because the split decides what the doc should say.
3. **#152-1** Wrap Imperva's `solve_captcha` in `timeout_at(deadline, ..)` and port
   the DataDome test (`zendriver-imperva/src/bypass.rs`). Independent.
4. **#158-1** Re-point the selectors test at `resolve_many_inner` and delete
   `resolve_text_one` (`query/selectors.rs`). Independent. Dissolves item 30.
5. **#158-2** Replace `.trim()` in `JS_NORMALIZE_SPACE` with the XPath-faithful
   edge strip (`query/predicate.rs`). Must precede the `TextPred::Equals` doc
   rewrite (#158-3, item 40) so one edit covers both divergences.
6. **#158-6** Replace the XPath quote-swap with `concat()`
   (`query/selectors.rs`). Independent of 4 and 5 but in the same file; sequence
   after 4 to avoid a conflict.
7. **#152-2** Return `iframePresent` from `POLL_JS` and key the `ChallengeGone`
   terminal off it (`zendriver-cloudflare/src/bypass.rs`). Do **not** apply the
   reviewer's marker-based fix; it was proven to be a net regression. Unblocks 25.
8. **#155-4 + #155-5 + #155-12** One edit: `create_new(true).mode(0o600)` plus the
   single-cleanup restructure (`cookies/persistence.rs`). The three findings share
   the same lines.
9. **#155-1** Add a size cap to the blocklist fetch (`tracker.rs`). Unblocks 28.
10. **#156-4** Write the URL through the old `Arc` before the rename swap
    (`frame/lifecycle.rs`). Same file as item 1; sequence after it.
11. **#156-6** Add the stale-context retry to `Frame::evaluate`
    (`frame/mod.rs`), ideally as a helper shared with `Tab::evaluate`.
12. **#158-5** Send `captureBeyondViewport` from `Element::screenshot`
    (`element/screenshot.rs`). Independent.
13. **#152-3 + #152-7** Wrap the Cloudflare evaluators in one IIFE so `WALKER_JS`
    leaks no globals, and add `behavior: "instant"` to `SCROLL_JS`'s
    `scrollIntoView`. Same file as item 7; sequence after it, and do the two
    together since both edit the JS constants.
14. **#156-5** Correct `QuitOutcome::NotPending`'s doc and log, and consider the
    `Unanswerable` rename (`browser.rs`).
15. **#150-1 + #150-2** Rewrite the `CallError::Timeout` and
    `ZendriverError::CdpTimeout` docs together, plus the `#[error(...)]` string and
    the missing `CdpTimeout` row in `error-reference.md`. One unit of work: the
    second must match the first.
16. **#156-2** Rewrite `FrameInner::name`'s doc and clear the two other stale
    P4-era claims in `frame/mod.rs`.
17. **#157-3** Fix the screenshot scroll comment **and** the module doc at lines
    6-8 (`element/screenshot.rs`). Half of this fix is worse than none.
18. **#157-4** Rewrite `ClickOptions::click_count`'s rustdoc.
19. **#152-9** Fix the `nlbi_` prefix and the two test literals that pin the wrong
    form, and correct the "same prefix match as the legacy scan" comment.

### Phase 2 — coverage that would have caught these

20. **#150-3** Add `swept_total` and turn `live_pending_survives_the_sweep_boundary`
    into a real sweep assertion. Independent.
21. **#157-7** Rename the `check_visible` source pins and add the real-browser
    fixture to `tests/find_visible_only.rs`. Depends on item 2.
22. **#155-3** Add the two end-to-end monitor latch tests. Independent, and the
    highest-value test in this set: it is the only one that would have caught the
    original symptom.
23. **#155-2** Replace the `num_alive_tasks()` oracle with returned `JoinHandle`s
    in all three tests. Can run alongside 22.
24. **#157-5** Assert `buttons_held` while it is live in the drag test.
25. **#157-6** Add the two `move_realistic` tests (frame count, and total duration
    under `tokio::time::pause()`).
26. **#152-6** Assert the terminal in
    `captcha_solver_is_invoked_at_most_once_per_clearance`.
27. **#156-3** Add a log-capturing layer and assert the geo warnings; fold the
    duplicate `non_json_error_page_yields_none` into `bad_body_yields_none`; add
    the `probe()` helper.
28. **#156-11** Add the real-Chrome fixture for the provisional-frame hypothesis
    alongside `frame_find_inside_iframe`. Depends on item 1 landing first so the
    test targets the final shape.
29. **#158-9**, **#158-10**, **#158-14**, **#155-14**, **#157-24**, **#152-10**
    Small test tightenings, all independent of each other. For #152-10, delete the
    byte-offset assertion and fix the hard-coded prefix; do not add a JS engine as
    a dev-dependency.

### Phase 3 — dependent cleanups

30. **#158-8** Needle-folding duplication. Already dissolved if item 4 deleted the
    dead resolver; verify rather than redo.
31. **#152-13 + #152-15** Port stall detection into DataDome, and fix the
    Cloudflare `stall_ticks` hole plus the `ticks_since_click` guard. Do the
    Cloudflare fix first so the ported version is the correct one, and keep
    `warned_stall` once `stall_ticks` can reset.
32. **#150-4** Extract `CMD_CHANNEL_CAPACITY`.
33. **#156-8** Restore the read-lock fast path in the `frameNavigated` arm. Depends
    on item 10 (same block).
34. **#157-19 + #157-13 + #157-20** One coherent change: add `hit_point` and a
    `HOVER` preset to `ActionabilityCheck`, then make `serve_gate_probes` take the
    check set instead of a count. Do them together; done separately they conflict.
35. **#157-12 + #158-11** Consolidate the bounded-wait helpers onto
    `test_support::expect` plus a named `try_expect`. Depends on 34 only for merge
    order in `test_support.rs`.
36. **#155-11** Move `write_atomic` and friends to `crate::io`. Depends on item 8
    (do not move the file mid-fix).
37. **#155-9**, **#155-15**, **#155-16**, **#155-17**, **#155-18**, **#157-25**,
    **#157-10**, **#157-11**, **#157-22**, **#155-8**, **#157-17**, **#157-18**,
    **#152-12**, **#152-16**, **#150-14**, **#150-13**, **#150-15**, **#156-10**
    Independent low-severity cleanups, parallelisable in any order and groupable by
    file.

### Phase 4 — docs and remaining prose

38. **#157-2 (with #158-4)** Rewrite `Element::is_visible`'s rustdoc and the three
    book surfaces. Depends on item 2. `#158-4` is the same fix seen from the child
    PR; do not fix it twice.
39. **#152-4** Rewrite the Cloudflare book chapter, including the phantom
    `NoChallenge` / `ClearanceTimeout` variants and the non-compiling code sample.
    Depends on item 7 for the `ChallengeGone` bullet.
40. **#158-3**, **#155-10**, **#152-8**, **#156-7**, **#156-9**, **#156-13**,
    **#156-14**, **#157-14**, **#157-16**, **#157-21**, **#157-23**, **#157-8**,
    **#157-15**, **#155-19**, **#150-6**, **#150-11**, **#150-12**, **#150-5**,
    **#155-6**, **#155-7**, **#155-13**, **#156-15**, **#158-13**, **#152-17** Doc
    and comment corrections. All independent except where noted. #158-3 depends on
    item 5. #156-14 depends on item 1 (the wording must describe the spawned
    sweep). #155-10 depends on item 9 (the doc should name both bounds). #156-7 is
    the doc half only, correcting "every ambiguous case"; the candidate-filter half
    waits on the human call in section 5.
    **#150-7** also lands here, in whichever direction section 5 decides.
41. **#158-7** Rewrite the regression-test header and move the `check_visible`
    paragraphs of the commit message onto the parent commit. Do this last in the
    #157/#158 stack, once the fixes have settled which commit owns what.
42. Gated on section 5, not scheduled here: **#152-5** (the `visid_incap_*` call)
    and **#157-9** (Puppeteer parity on modifier+text). Neither is an agent
    decision; both become a small edit once decided.
43. Recorded, no action: **#150-16**, **#150-9**, **#150-10**, **#150-8**,
    **#152-11**, **#152-14**, **#155-20**, **#156-12**, **#158-15**, **#158-12**
    (beyond the changelog line). These are either non-issues after verification or
    changes whose cost exceeds their benefit. Listed so nobody re-raises them.

---

## 4. Struck findings

One finding did not survive verification.

### #152 — `wait_for_clearance_returns_challenge_gone_when_iframe_disappears_after_click` asserts the bug

[bypass.rs:705](crates/zendriver-cloudflare/src/bypass.rs:705)

**Claimed.** The test replies to poll 2 with `poll_value(None, None, true)` (no
bbox, `hasMarkers: true`) and asserts `ChallengeGone`, with a comment reading
"iframe gone, no token, markers linger in the DOM". The reviewer read that as the
finding-152-2 false positive written down as the expected result, and proposed
changing poll 2 to `hasMarkers: false`.

**Why it did not hold.** Given that payload, "the iframe we clicked was torn down
and the site's own `.cf-turnstile` container is still in the DOM" is a realistic
and arguably correct reading, and `ChallengeGone` is the right terminal for it. The
payload is ambiguous because `POLL_JS` cannot express the difference, which is
152-2's actual root cause, so the test picks a defensible interpretation of an
ambiguous input rather than asserting a bug.

The offered proof was circular. The evidence that the test is wrong was "with the
corrected predicate applied, this is the only test in the crate that fails". That
was reproduced exactly, and it is evidence about the proposed fix, not about the
test: that fix makes `ChallengeGone` unreachable whenever a site container lingers,
and this test is the only one covering that path, so it is the only casualty.

The proposed edit is also a coverage loss. Changing poll 2 to `hasMarkers: false`
makes it a near-duplicate of
`wait_for_clearance_returns_challenge_gone_when_markers_vanish_without_click` and
deletes the crate's only test of the clicked-then-torn-down path. The two sibling
tests do not disagree about what "the challenge went away" means; one covers
markers vanishing with no click, the other covers a clicked iframe disappearing.
Different arms, both wanted.

**Do not re-raise.** Fix 152-2 in `POLL_JS` and this test keeps its meaning.

---

## 5. Needs a human

These are judgement calls about product direction, API shape, or acceptable risk,
not defects. An agent should not decide any of them; each needs an explicit call
recorded in the PR before the dependent fix lands.

1. **`visible_only` semantics (#157-1).** Should `.visible_only()` mean "rendered"
   or "on screen right now"? The gate needs the second; a `find_all()` filter that
   silently truncates a 200-row table to the viewport is almost certainly not what
   a scraping user wants, but the PR's own test comment suggests the author
   intended offscreen filtering. This is a public API semantics decision and it
   determines the shape of the split, the rustdoc, and the Playwright migration
   mapping. Everything else in the #157/#158 doc chain waits on it.

2. **`Frame::name()` and `Frame::id()` interior mutability (#156-4, #156-9).** The
   correct fix for handle orphaning and for `wait_for_load` on a rewritten frame id
   is to make `FrameInner::name` and `frame_id` interior-mutable, which turns
   `Frame::name()` into an `async fn` and changes a public signature. The repo's
   recorded position is that pre-release API churn is acceptable, but this is a
   deliberate break and should be decided rather than assumed. The two-line
   URL-write-through stopgap is available either way.

3. **`visid_incap_*` (#152-5).** Is Imperva's persistent visitor-ID cookie a
   challenge marker or ambient state? Neither the reviewer nor the verifier could
   establish its lifetime from anything in this repo, and getting it wrong in the
   permissive direction makes the surface detector too loose. Needs someone with
   Imperva domain knowledge or a live observation, not a guess.

4. **The Cloudflare false-success vs false-timeout tradeoff (#152-2).** The
   JS-level fix removes the ambiguity, but the underlying question, whether real
   Turnstile ever presents a mounted-but-unclickable widget post-click, was never
   established against a live browser. If it does not, the current code is fine and
   the fix is defensive. Someone should decide whether to spend a live-browser
   session confirming it before changing the wire shape.

5. **OOPIF eviction (#156-7).** Whether Chrome lists remote frames in the parent
   target's `Page.getFrameTree` is an empirical question that decides whether the
   sweep can delete a live OOPIF host row. The verification is agent-doable; the
   subsequent design choice (a grace period on url-less candidates versus an
   OOPIF-aware filter) is a real tradeoff between eviction latency and safety.

6. **Real-browser test tier investment (#152 JS, #156-11, #157-7, #158 opacity
   honeypot).** Four separate findings reduce to one question: the JavaScript in
   the solver crates, the frame-tree hypothesis, the visibility predicate and the
   ancestor-opacity walk are all untestable through `MockConnection` and all
   currently uncovered. Standing up (or extending) a live-Chrome tier is a
   resourcing decision with ongoing CI cost. Note the repo already has one:
   `tests/find_visible_only.rs` and `integration_phase4.rs` are gated on the
   `integration-tests` feature.

7. **Widening `zendriver-transport`'s public surface (#150-7).** Making
   `REDIAL_TIMEOUT` public would let the handshake-parity assertion live where both
   constants are visible, but `lib.rs` explicitly steers consumers away from
   depending on this crate directly. Deleting the tautological test and softening
   the doc is the cheaper honest option. Pick one.

8. **Splitting `CallError::Timeout` (#150-1).** The alternative to fixing the prose
   is a type-level distinction between "never enqueued" and "never answered", which
   every caller then has to match on. KISS argues for one variant with honest docs;
   someone should say so explicitly rather than leave it implied.

9. **`tempfile::NamedTempFile` and the two fingerprints copies (#155-11).**
   Adopting `tempfile` would give exclusive creation, 0600 mode, unique naming and
   drop-cleanup for free and delete about half of `persistence.rs`, but it adds a
   dependency and touching `zendriver-fingerprints` widens the blast radius of an
   already-large PR. New dependency plus cross-crate refactor is a maintainer call.

10. **Monitor consecutive-failure backstop (#155-8).** Adding a counter that
    latches after N non-`-32601` failures is hardening, not a defect fix, and it
    trades a small amount of complexity for protection against a failure mode
    nobody has observed. Decide whether the logging fix alone is enough.

11. **Puppeteer parity on modifier+text (#157-9).** Blink ignores the `text` field
    when a non-Shift modifier is held, verified against Chrome 151, so the current
    behavior is correct in practice. Matching Puppeteer's normalization is a
    consistency preference with no behavioral payoff. Worth doing only if the
    project wants byte-level parity with the reference implementation.

12. **`captureBeyondViewport` on on-screen clips (#158-12).** A deliberate,
    documented behavior change to existing callers' output. It needs a changelog
    entry at minimum, and someone should confirm it is acceptable rather than
    gate the flag on whether the clip actually escapes the rendered area.

13. **Chrome 105-120 degradation of `check_visible` (#157 security note).** The new
    predicate depends on `Element.checkVisibility` with Chrome 121 option names, so
    older Chrome silently degrades to a bare `display: none` test and the honeypot
    coverage quietly disappears. Nothing in the code or the book warns an operator
    pinning an older Chrome. Whether to warn, feature-detect, or raise the stated
    minimum is a support-policy decision.
