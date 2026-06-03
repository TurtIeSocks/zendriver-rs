# `wait_for_idle` stuck-request hardening — design

Date: 2026-06-03
Status: approved

## Problem

`Tab::wait_for_idle` (and `wait_for_idle_with`) resolves only when the per-tab
in-flight request count stays at 0 for the whole `quiet_window`. A single
request that never reaches a terminal CDP event — a hung beacon, a long-poll,
Server-Sent-Events, a websocket-style upgrade, or just a stuck XHR — keeps the
count at ≥ 1 forever, so the call hangs until its outer `timeout` and returns
`Err(Timeout)`. This is the well-known fragility of Playwright's `networkidle`.

The preceding fix (launching the initial tab on `about:blank`) removed the
*trigger* that surfaced this in CI (Chrome New Tab Page requests hanging on a
network-restricted runner), but the underlying limitation remains for real
pages.

## Decision: opt-in age eviction, default unchanged

Add a `max_inflight_age: Option<Duration>`. A request that has been in flight
*longer than* this threshold is excluded from the idle calculation — treated as
background/stuck rather than active loading.

- `None` (default) → **identical to today's behavior**: no eviction; a stuck
  request hangs to the outer timeout. Existing callers are unaffected.
- `Some(age)` → the caller opts in with a threshold *they* choose.

Rationale: no age threshold can distinguish "a legitimate 5 s request, 4 s in"
from "an infinite request, 4 s in". The right value is a per-call tradeoff, not
a default the library should impose — consistent with the project's
"least-opinionated, everything-overridable, never lock a value" stance.

## API

```rust
/// Tunables for `Tab::wait_for_idle_opts`.
#[derive(Debug, Clone)]
pub struct IdleOptions {
    /// Outer bound; `Err(Timeout)` once it elapses. Default 30 s.
    pub timeout: Duration,
    /// The set must stay empty this long to count as idle. Default 500 ms.
    pub quiet_window: Duration,
    /// Requests in flight longer than this are ignored when judging idle.
    /// `None` (default) waits for every request to terminate.
    pub max_inflight_age: Option<Duration>,
}

impl Default for IdleOptions {
    // 30 s, 500 ms, None
}
```

Methods:

```rust
// unchanged signatures, now thin delegates over wait_for_idle_opts:
pub async fn wait_for_idle(&self) -> Result<()>;
pub async fn wait_for_idle_with(&self, timeout: Duration, quiet_window: Duration) -> Result<()>;

// new — full control:
pub async fn wait_for_idle_opts(&self, opts: IdleOptions) -> Result<()>;
```

Usage:

```rust
tab.wait_for_idle_opts(IdleOptions {
    max_inflight_age: Some(Duration::from_secs(5)),
    ..Default::default()
}).await?;
```

## Mechanism

- **Tracker** (`network_idle.rs`): `in_flight: Mutex<HashSet<String>>` becomes
  `Mutex<HashMap<String, tokio::time::Instant>>`. `Network.requestWillBeSent`
  inserts `(request_id, Instant::now())`; the three terminal events remove by
  id. The tracker uses the same `tokio::time` clock the wait loop reads, so ages
  are consistent.
- **Wait loop** (`tab.rs::wait_for_idle_opts`): compute an *active count* each
  iteration — `None` ⇒ `map.len()`; `Some(age)` ⇒ number of entries whose
  `now - inserted < age`. Idle accounting is otherwise unchanged (quiet-window
  accumulation, notifier + 50 ms fallback tick, outer-deadline check). A request
  crossing the age threshold fires no CDP event, so the 50 ms tick is what
  notices the age-out — already the loop's worst-case latency.
- Map membership is purely event-driven; it is **never** age-pruned. A `None`
  waiter therefore still waits forever as contracted even if a concurrent
  `Some` waiter is filtering the same map. A genuinely-stuck id lingers in the
  map until the tab is dropped — a negligible, documented memory cost.

## MCP exposure

`browser_wait_for_idle` (`crates/zendriver-mcp/src/tools/navigation.rs`) gains an
optional `max_inflight_age_ms: Option<u64>` on `IdleInput`, threaded into
`wait_for_idle_opts`. Regenerate and accept the `insta` schema snapshots.

## Testing

- **Unit** (`tab.rs` tests, `Tab::new_for_test` + `MockConnection`): emit
  `Network.requestWillBeSent` and **never** a terminal event. With
  `max_inflight_age: Some(300ms)` + 200 ms window the call resolves (~500 ms);
  with `None` + a 1 s outer timeout it returns `Err(Timeout)`. Short real
  durations, matching the existing idle tests.
- **Integration** (`integration_phase4.rs`): a fixture endpoint held open with
  `set_delay` far beyond the test, fetched on load; `wait_for_idle_opts` with a
  short `max_inflight_age` resolves well under the outer timeout — assert
  elapsed ≈ age + window, not the timeout.

## Out of scope

- Changing any default (stays `None`).
- The broadcast-`Lagged` dropped-event leak (separate latent issue; bus capacity
  1024, unrelated trigger).
- Age-pruning the map (would break `None`-waiter semantics).

## Public-API impact

New `pub struct IdleOptions` + `wait_for_idle_opts` method ⇒ regenerate
`crates/zendriver-mcp/public-api-baseline.txt`. No MCP coverage-ledger entry
needed (the capability stays reachable via `browser_wait_for_idle`).
