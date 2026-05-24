# Error Reference

Every fallible API in zendriver returns
[`Result<T>`](https://docs.rs/zendriver/latest/zendriver/type.Result.html),
an alias for `std::result::Result<T, ZendriverError>`. This page lists
every public variant of [`ZendriverError`] plus the sub-crate errors
that flow into it via `#[from]`, with the common cause + the fix.

[`ZendriverError`]: https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html

The enum is `#[non_exhaustive]` — pattern matches should include a `_`
arm. Per [SEMVER](https://github.com/TurtIeSocks/zendriver-rs/blob/main/SEMVER.md),
new variants may land in minor releases.

## Top-level `ZendriverError` variants

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `Browser(BrowserError)` | Chrome launch / discovery failed. | See [BrowserError](#browsererror-variants) below. |
| `Transport(TransportError)` | WebSocket failure (Chrome crashed, socket reset). | Retry the operation; if recurring, check Chrome's stderr for crash dumps. |
| `Cdp { code, message, data }` | Chrome returned a CDP RPC error. | Inspect `message` — `Invalid params` usually means a stale `RemoteObjectId` or wrong type signature. |
| `ElementNotFound { selector }` | Query selector matched zero elements within the timeout. | Confirm the page actually rendered the element (`tab.wait_for_load()` / `wait_for_idle()`); check the selector with `tab.find().css(...).count()`. |
| `Timeout(Duration)` | Generic operation timeout. | Increase the timeout via the builder, or address the underlying slow operation. |
| `Navigation(String)` | Page navigation failed (DNS, refused, crashed) **or** an in-flight call lost its context to a navigation. | Check the URL / DNS / network; for the context-lost case, sequence the action after `tab.wait_for_load()`. |
| `JsException(String)` | A JS expression in `evaluate()` raised an exception. | Wrap the JS in `try { ... } catch (e) { return null; }` if you want a soft failure; otherwise fix the JS. |
| `ElementStale` | An element's CDP handle invalidated **and** the auto-refresh path failed. | Re-issue the original query manually. |
| `NotRefreshable` | An element returned from raw `evaluate()` went stale and can't be replayed (no underlying selector). | Use `tab.find()` instead of `evaluate` when you need an element you'll hold across DOM mutations. |
| `NotActionable(Duration, reason)` | Element wasn't visible/enabled/stable/hit-testable within the gate timeout. | See [FAQ entry](./faq.md#why-am-i-getting-notactionable). Use `click_fast` to skip the gate when intentional. |
| `FrameNotFound(String)` | `tab.frame_by_url/name/id(...)` matched no frame. | Confirm the iframe is loaded; `tab.frames().await?` to inspect what frames Chrome sees. |
| `TabNotFound(String)` | Tab registry lookup failed (auto-attach observer crashed or new-tab race window exceeded). | Restart the browser; report as a bug if it happens with a reliable repro. |
| `Cookie(String)` | Cookie operation refused by Chrome (malformed domain, mixed origin, etc.). | Read the message — most often a domain / path mismatch. |
| `Storage(String)` | DOM storage operation refused (origin mismatch). | Confirm `tab.url()` matches the storage origin before the call. |
| `HistoryNavigation(String)` | `back()` / `forward()` with no entry to go to. | Check `tab.history_length().await?` first. |
| `Serde(serde_json::Error)` | JSON serialization at the CDP boundary failed. | Almost always indicates a type-mismatch bug in zendriver — please file an issue with the failing call. |
| `Io(std::io::Error)` | File I/O failure (screenshot write, upload read). | Standard `std::io::Error` handling; check permissions. |
| `Stealth(StealthError)` | Fingerprint resolution failed at launch. | See [StealthError](#stealtherror-variants) below. |
| `Interception(InterceptionError)` | Interception-layer error (gated `interception`). | See [InterceptionError](#interceptionerror-variants) below. |
| `Cloudflare(CloudflareError)` | Cloudflare bypass error (gated `cloudflare`). | See [CloudflareError](#cloudflareerror-variants) below. |
| `Fetcher(FetcherError)` | Chrome download error (gated `fetcher`). | See [FetcherError](#fetchererror-variants) below. |

## `BrowserError` variants

Sub-error returned wrapped in `ZendriverError::Browser`.

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `ExecutableNotFound { searched }` | No Chrome on `$PATH` or in conventional install locations. | Install Chrome / Chromium, or pass `.executable(path)` to the builder. Use the `fetcher` feature to auto-download. |
| `SpawnFailed(io::Error)` | OS refused to spawn the binary (permissions, missing libs). | Check the wrapped `io::Error`; `Permission denied` → chmod, `No such file` → bad path. |
| `EarlyExit(ExitStatus)` | Chrome exited before printing `DevTools listening on`. Typical: `user_data_dir` locked by another Chrome, missing GPU sandbox on Linux. | Free the user-data-dir (kill stale Chrome processes), or pass `.arg("--no-sandbox")` if running in a container. |
| `WsTimeout` | Chrome printed nothing within the WS-endpoint wait window. | Try headed mode (`headless(false)`) to see Chrome's window — usually reveals a missing dependency. |
| `DevtoolsParse` | Stderr line matched expected pattern but URL didn't parse. | Should not happen with stable Chrome; file a bug. |
| `Cleanup(io::Error)` | `tempfile` cleanup of the `user_data_dir` failed. | Usually harmless; check filesystem permissions if persistent. |

## `TransportError` variants

Re-exported from `zendriver-transport`. Surfaced via
`ZendriverError::Transport`.

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `Disconnected` | Chrome closed the WebSocket without a Close frame. Typically Chrome crashed. | Restart the browser; check syslog / dmesg for OOM kills. |
| `Ws(tungstenite::Error)` | Underlying WebSocket error. | Inspect the wrapped error — `ConnectionClosed` is benign during shutdown. |
| `Frame(serde_json::Error)` | JSON framing failed on a CDP message. | Indicates a Chrome protocol drift; file an issue. |
| `Shutdown` | Actor was told to shut down; pending calls drain with this. | Expected during graceful `browser.close()`; only an error if it surprises you. |
| `ResponseDropped { id }` | Actor replied but the caller's `oneshot` receiver had dropped. | Should not happen in normal use; indicates a panic somewhere up-stack. |
| `Io(io::Error)` | I/O error inside tungstenite. | Standard I/O handling. |

`CallError` is the transport's per-call result type; it folds into
`ZendriverError` automatically via the `From` impl — you won't see it
directly in the public surface.

## `StealthError` variants

Sub-error returned wrapped in `ZendriverError::Stealth`.

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `PatchFailed { patch, source }` | A specific stealth patch CDP call failed. | Read the source `CallError`; usually means the target page navigated mid-patch. Retry the launch. |
| `ChromeVersionDetect(String)` | Probe of `chrome --version` failed. | Confirm the binary path; pass `.chrome_version(N)` to the stealth profile to skip the probe. |
| `SystemInfo(String)` | `sysinfo` couldn't read RAM / CPU count. | Pass `.memory_gb(N).cpu_count(N)` overrides to skip the auto-detect. |
| `InvalidOverride(String)` | A fingerprint override value was outside the validated range (e.g. `memory_gb = 0`). | Read the message; fix the override. |

## `InterceptionError` variants

Gated `interception`. Sub-error returned wrapped in
`ZendriverError::Interception`.

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `Call(CallError)` | Underlying CDP call failed. | Inspect inner error. |
| `InvalidPattern(String)` | URL pattern didn't parse as CDP wildcard syntax. | Patterns use `*` / `?` (not regex). Quote literal `*` characters. |
| `AlreadyStarted` | `start()` called twice on the same builder. | Builders are one-shot; create a new one if you need another actor. |
| `NotStarted` | Operation requires an active actor that hasn't started yet. | Call `start()` first. |
| `SubscriptionClosed` | The `subscribe()` stream's actor was torn down. | Stream ends naturally on `InterceptHandle` drop; expected during shutdown. |
| `InvalidResponse(String)` | A CDP response didn't carry the expected field (e.g. `Fetch.getResponseBody` returned no `body`). | Should not happen with stable Chrome; file a bug. |

## `CloudflareError` variants

Gated `cloudflare`. Sub-error returned wrapped in
`ZendriverError::Cloudflare`.

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `NoChallenge` | No Turnstile iframe was detected at call time. | Treat as success — the page was already cleared (cookie shortcut) or had no CF gate. |
| `ClearanceTimeout` | Deadline elapsed without resolution. | The challenge may be silent / escalated; pair with `StealthProfile::spoofed`, or switch to a residential proxy. |
| `Call(CallError)` | Underlying CDP call failed (typically the JS detection probe). | Inspect inner error. |
| `JsError(String)` | The detection / clearance JS raised an exception. | The page may be CSP-strict; ensure stealth `bypass_csp(true)` (the default for `spoofed`). |

## `FetcherError` variants

Gated `fetcher`. Sub-error returned wrapped in `ZendriverError::Fetcher`.

| Variant | Common cause | Fix |
|---------|-------------|-----|
| `Http(reqwest::Error)` | Network call to the CFT manifest / CDN failed. | Check connectivity; CFT URLs need outbound HTTPS to `googlechromelabs.github.io` and the CDN. |
| `Io(io::Error)` | Local FS write failed (cache, extract). | Check cache-dir permissions / free space. |
| `Manifest(serde_json::Error)` | Manifest JSON didn't parse. | Should not happen with the canonical URL; means the CFT side changed format — file an issue. |
| `VersionNotFound(version)` | `VersionSpec::Explicit("...")` string not present in manifest. | Drop a version (CFT only keeps the last N); use `VersionSpec::Latest` or a known version from the manifest. |
| `UnsupportedPlatform` | `Platform::auto_detect` returned `None`, or a non-`Stable` channel was requested. | Currently no fix for unsupported platforms (Linux arm64, BSDs); install Chrome out-of-band. |
| `IntegrityFailed { expected, actual }` | SHA256 of the downloaded zip doesn't match the manifest. | Delete the partial download under the cache dir; retry. |
| `Extraction(String)` | Zip extraction failed. | Free disk space; check for filesystem corruption. |

## Pattern-matching tips

- **Use `matches!`** for boolean checks on a single variant:
  ```rust,ignore
  if matches!(err, ZendriverError::ElementNotFound { .. }) {
      // soft-fail path
  }
  ```
- **Use `_` always** to handle future variants gracefully — `#[non_exhaustive]`
  requires it.
- **Sub-errors flatten via `#[from]`** — your `?` operator works across
  the boundary (e.g. `let body = paused.body().await?;` returns
  `InterceptionError` but converts to `ZendriverError::Interception`
  inside a function returning `zendriver::Result`).
