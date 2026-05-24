# Architecture

This chapter sketches the layered design that zendriver-rs sits on top
of. The goal is to give you enough mental model to debug surprises
("why did my evaluate fail mid-navigation?") and to reason about
performance ("is interception serializing my requests?").

## The big picture

```text
                            ┌─────────────────────────────────┐
                            │  Your code: Browser/Tab/Element │
                            │      query / actions / eval     │
                            └────────────────┬────────────────┘
                                             │
              ┌──────────────────────────────┼─────────────────────────────┐
              │                              │                             │
              ▼                              ▼                             ▼
   ┌────────────────────┐       ┌────────────────────────┐    ┌─────────────────────┐
   │  Stealth (boot JS) │       │  Element auto-refresh  │    │  Isolated-world eval│
   │  + protocol patch  │       │  + actionability gate  │    │  (sandbox per Tab)  │
   └─────────┬──────────┘       └───────────┬────────────┘    └──────────┬──────────┘
             │                              │                            │
             └──────────────────────────────┴────────────────────────────┘
                                            │
                                            ▼
                            ┌─────────────────────────────────┐
                            │   CDP Actor (single Tokio task) │
                            │   – cmd/response routing        │
                            │   – event fan-out + observers   │
                            └────────────────┬────────────────┘
                                             │   JSON-RPC
                                             ▼
                            ┌─────────────────────────────────┐
                            │     Chrome (subprocess)         │
                            │     CDP over WebSocket          │
                            └─────────────────────────────────┘
```

Every public type above the actor is a cheap handle (`Arc` clone +
session-id) — the actor is the single source of truth for outbound
commands and inbound events.

## The CDP transport actor

`zendriver-transport` runs a single Tokio task that owns the WebSocket
connection to Chrome. All command sends go through a
`mpsc::UnboundedSender`; every public handle holds a `Connection`
clone that wraps that sender. The actor task:

1. Reads commands from the channel, attaches a monotonically-increasing
   `id`, and writes them to the socket.
2. Reads frames from the socket, decodes them into `CdpInbound` (either
   a response to a command or an event), and routes them.
3. For responses: looks up the pending `oneshot::Sender` in a
   `HashMap<id, oneshot::Sender<Result<Value>>>` and resolves it.
4. For events: fans them out via a `tokio::sync::broadcast` so every
   subscriber gets a copy without blocking the actor loop.
5. For `Target.attachedToTarget`: invokes each registered
   [`TargetObserver`] (stealth installs JS bootstrap here) **before**
   releasing the debugger pause, so observers run during the gap.

[`TargetObserver`]: https://docs.rs/zendriver/latest/zendriver/trait.TargetObserver.html

The actor model gives you exactly-one-reader/writer per socket without
explicit locking, while keeping the public surface cloneable (`Tab`,
`Element` are `Clone + Send + Sync`). All concurrency happens in
user-space Futures handed back from `connection.call(...)`.

## Observer pattern

Most CDP usage thinks of events as "fire and forget — subscribe if you
care". zendriver-rs has two layers:

- **Broadcast subscribers** — `Tab` clones a per-target receiver from
  the broadcast channel. Event helpers (`expect_request`, etc) drop
  into this layer, filter on type + payload, and resolve a oneshot when
  the first match arrives.
- **Synchronous observers** — `TargetObserver` runs on
  `Target.attachedToTarget` **before** the new target's debugger pause
  is released. Stealth depends on this: the auto-attach observer
  dispatches `Page.addScriptToEvaluateOnNewDocument` while the page is
  still paused, so the bootstrap script lands before any page script.
  No race; no need for the script to detect its own arrival timing.

The same observer chain re-applies stealth on every new tab — that's
why `Browser::new_tab()` gives you a fully stealth-patched tab without
extra code.

## Auto-refresh on stale handles

CDP returns a `RemoteObjectId` (per-context handle) for every queried
element. Those handles invalidate when the page re-renders or
navigates — Chrome will return `Cannot find object with given id` on the
next call, which Playwright papers over by re-resolving the locator on
every action.

zendriver-rs takes a different bet: cache the `RemoteObjectId` on the
`Element` for speed, but transparently re-run the original query and
retry the action when the handle goes stale. The trigger:

```text
                      Element::click()
                            │
                            ▼
            ┌─── Runtime.callFunctionOn ───┐
            │   on cached RemoteObjectId   │
            └──────────────┬───────────────┘
                           │
                  ┌────────┴────────┐
                  │ success? ──► return │
                  └────────┬────────┘
                           │ stale
                           ▼
              re-run cached query origin
                  (find().css("..."))
                           │
                           ▼
              new RemoteObjectId; retry once
                           │
                  ┌────────┴────────┐
                  │ success? ──► return │
                  └────────┬────────┘
                           │ stale again
                           ▼
                  Err(ZendriverError::ElementStale)
```

Handles returned from raw `evaluate()` calls (no underlying query) can't
be replayed and surface `ZendriverError::NotRefreshable` on stale. The
borrow checker tracks the query scope for you — there's no way to use
an element across browser teardown.

## Isolated-world evaluation

`tab.evaluate(...)` runs JS in a sandboxed isolated world per tab: a V8
context that shares the DOM with the main world but has its own global
scope. This means:

- The page can't detect your eval via `Function.prototype.toString`
  drift, `window`-global mutation, or scope-leak tells.
- Your JS can't see page globals (`window.appConfig`, jQuery, etc).

For the main-world escape hatch, use `evaluate_main(...)`. It dispatches
the same `Runtime.evaluate` but targets the page's default context.

The isolated world is allocated lazily on first `evaluate()` and cached
per tab. After navigation Chrome invalidates the context; the next
`evaluate()` call re-allocates transparently. Frames each have their
own isolated world (allocated per frame contextId).

## Why these choices

- **CDP-direct (no WebDriver shim).** WebDriver's JSON wire serializes
  every command to disk-style protocol overhead — milliseconds per call
  on localhost, plus needing a separate `chromedriver` process. CDP is
  millisecond-roundtrip over a single socket and exposes the full
  protocol surface (interception, fetch, target tree, etc).
  Anti-detection also requires protocol-level control:
  `chromedriver` injects its own automation tells that we'd then have
  to scrub back out.
- **Single actor task.** Easier reasoning than a connection pool; no
  command-ordering ambiguity. The actor does no parsing past JSON-RPC
  framing, so it's not a CPU bottleneck even under interception load.
- **Tokio runtime.** Browser automation is I/O-heavy (every action
  costs at least one round-trip to Chrome); pinning ourselves to Tokio
  gives us the mature `tokio::time`, `tokio::sync`, `tokio::select`
  surface plus the ecosystem (`reqwest`, etc).
- **Auto-refresh by default.** Two-thirds of "flaky test" reports we
  triaged during P3 development were stale-handle races. Making it
  silent + a single retry covers >95% of cases without inviting the
  Playwright-style "every action re-finds, eating round-trips" cost.

## Crate split

| Crate | Purpose | Public? |
|-------|---------|---------|
| `zendriver` | High-level Browser/Tab/Element + traits | yes |
| `zendriver-transport` | Actor + WebSocket + observers | yes, but [SEMVER] looser |
| `zendriver-stealth` | Fingerprint composition + bootstrap JS | yes |
| `zendriver-interception` | `Fetch.*` actor + rule + stream API | yes, gated `interception` |
| `zendriver-cloudflare` | Turnstile bypass | yes, gated `cloudflare` |
| `zendriver-fetcher` | Chrome-for-Testing downloader | yes, gated `fetcher` |

[SEMVER]: https://github.com/cdpdriver/zendriver-rs/blob/main/SEMVER.md

The split lets you take a dep on only what you need (the
`zendriver-transport` crate is the heaviest; the optional sub-crates
each pull a small additional surface). It also lets future runtime
backends (e.g. embedding in WASM or smol) replace `zendriver-transport`
without touching the high-level types — the actor's public API is the
seam.

## See also

- [`zendriver-transport` rustdoc] — wire types, observer trait, the
  `MockConnection` test harness.
- [Stealth](./stealth.md) — what the bootstrap JS actually patches.
- [Interception](./interception.md) — how the `Fetch.*` actor sits on
  top of the same transport.

[`zendriver-transport` rustdoc]: https://docs.rs/zendriver-transport
