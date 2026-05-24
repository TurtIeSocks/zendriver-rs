# Expect()

The `expect` Cargo feature ports Playwright's "pre-register then await"
pattern to zendriver-rs. Instead of polling for an event after triggering
it (which races against the response coming back faster than your
subscription registers), you register an expectation **before** the
action and await the returned `Future` afterwards. The subscriber is live
by the time `expect_*` returns, so the event cannot slip past.

Enable it in `Cargo.toml`:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["expect"] }
```

Four entry points on [`Tab`]:

| Method | CDP event | Returns |
|--------|-----------|---------|
| [`expect_request`] | `Network.requestWillBeSent` | [`RequestExpectation`] → [`MatchedRequest`] |
| [`expect_response`] | `Network.responseReceived` | [`ResponseExpectation`] → [`MatchedResponse`] |
| [`expect_dialog`] | `Page.javascriptDialogOpened` | [`DialogExpectation`] → [`MatchedDialog`] |
| [`expect_download`] | `Page.downloadWillBegin` + progress | [`DownloadExpectation`] → [`MatchedDownload`] |

[`Tab`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html
[`expect_request`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_request
[`expect_response`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_response
[`expect_dialog`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_dialog
[`expect_download`]: https://docs.rs/zendriver/latest/zendriver/struct.Tab.html#method.expect_download
[`RequestExpectation`]: https://docs.rs/zendriver/latest/zendriver/struct.RequestExpectation.html
[`MatchedRequest`]: https://docs.rs/zendriver/latest/zendriver/struct.MatchedRequest.html
[`ResponseExpectation`]: https://docs.rs/zendriver/latest/zendriver/struct.ResponseExpectation.html
[`MatchedResponse`]: https://docs.rs/zendriver/latest/zendriver/struct.MatchedResponse.html
[`DialogExpectation`]: https://docs.rs/zendriver/latest/zendriver/struct.DialogExpectation.html
[`MatchedDialog`]: https://docs.rs/zendriver/latest/zendriver/struct.MatchedDialog.html
[`DownloadExpectation`]: https://docs.rs/zendriver/latest/zendriver/struct.DownloadExpectation.html
[`MatchedDownload`]: https://docs.rs/zendriver/latest/zendriver/struct.MatchedDownload.html

## The race-free pattern

Naive polling races the network:

```rust,ignore
// WRONG — the click can fire the request and Chrome can return the
// response before our subscriber registers. We then poll forever.
go.click().await?;
let resp = wait_for_response("*/login").await?;  // race!
```

The correct flow pre-registers, then triggers:

```rust,ignore
// RIGHT — the oneshot subscription is live by the time expect_response
// returns. The request cannot complete before we're listening.
let expectation = tab.expect_response("*/login");
go.click().await?;
let resp = expectation.await?;  // safe
```

`expect_response` is sync — it spawns the subscriber task internally and
returns the awaitable handle synchronously, so any event Chrome emits
between the spawn point and the trigger action is captured.

## URL matching

`expect_request` and `expect_response` take any value that implements
`Into<UrlMatcher>`:

- `&str` / `String` — substring match (URL contains the needle).
- [`regex::Regex`] — full regex via `is_match`.

```rust,ignore
use regex::Regex;

let exp1 = tab.expect_response("/api/users");  // substring
let exp2 = tab.expect_response(Regex::new(r"^https://.*\.example\.com/v\d+/").unwrap());
```

`expect_dialog` and `expect_download` take no matcher — they fire on the
first event of their kind. If you need to filter further, inspect the
matched event in your code after `.await?`.

[`regex::Regex`]: https://docs.rs/regex/latest/regex/struct.Regex.html

## Full example: login response

This example renders a tiny form via `data:` URL, registers a response
expectation against `*/login`, clicks submit, and asserts the URL +
status:

```rust,no_run
{{#include ../../../crates/zendriver/examples/expect_login_response.rs}}
```

Three things worth noting:

1. The expectation is constructed **before** the click. Reversing those
   two lines reintroduces the race.
2. The `.timeout(Duration::from_secs(10))` overrides the default 30 s
   outer timeout. Use a tighter budget when you expect a fast local
   response — saves you 30 s of waiting on a test that's quietly broken.
3. `expectation.await?` resolves on the first match. If you need to
   collect every matching request over a window, use a `Stream` (build
   one via repeated `expect_request` calls or fall back to the
   interception API's `subscribe`).

## Dialogs

Pages that call `alert()` / `confirm()` / `prompt()` hang waiting for
user input. `expect_dialog` lets you handle the dialog programmatically:

```rust,ignore
let dlg = tab.expect_dialog();

// Trigger code that opens the dialog.
tab.evaluate_main::<()>("alert('hi')").await?;

let matched = dlg.await?;
println!("dialog type: {:?}, message: {}", matched.dialog_type, matched.message);
matched.accept(None).await?;  // dismiss with no prompt response
```

`MatchedDialog::accept(Some("text"))` supplies a response for `prompt()`
dialogs. `MatchedDialog::dismiss()` closes without accepting.

## Downloads

`expect_download` does extra per-tab wiring on its first call: allocates
a tempdir, dispatches `Browser.setDownloadBehavior { behavior:
"allowAndName", downloadPath }`, and starts a long-running progress
subscriber. Subsequent calls reuse the same setup, so the
per-`expect_download` cost is one CDP event subscription.

```rust,ignore
use std::path::PathBuf;

let dl = tab.expect_download().await?;

let link = tab.find().css("a[download]").one().await?;
link.click().await?;

let matched = dl.await?;

// Wait for completion then copy out of the tempdir to a stable path:
matched.save_to(PathBuf::from("/tmp/result.pdf")).await?;
```

The path returned by `MatchedDownload::path().await` points into a
per-tab tempdir (named by Chrome's CDP `guid`) and is `None` until the
transfer completes. The tempdir lives as long as the `Tab` does, so call
`save_to` to copy the file somewhere stable before the tab drops.

## Timeout semantics

All four expectations default to 30 s. Override per-call:

```rust,ignore
use std::time::Duration;

let exp = tab.expect_request("/api/")
    .timeout(Duration::from_secs(5));
```

On timeout the future resolves to
[`ZendriverError::Timeout`](https://docs.rs/zendriver/latest/zendriver/enum.ZendriverError.html#variant.Timeout).
The subscriber task is canceled before the error returns, so there's no
leaked listener.

## When to use which

- **`expect_response`** — confirm an API call returned (covers status +
  body). The most common one in tests.
- **`expect_request`** — assert *what* a page sent (headers, body,
  method). Useful for verifying CSRF tokens, auth bearer formats.
- **`expect_dialog`** — automate `alert` / `confirm` flows; required
  whenever the page may pop a dialog or your script will hang.
- **`expect_download`** — capture file downloads end-to-end; replaces
  the headless-Chrome "downloads vanish silently" footgun.

For continuous capture (every request matching a pattern, not just the
first), drop into [`Interception`](./interception.md)'s `subscribe()`
path instead.
