# Fetcher

The `fetcher` Cargo feature downloads a Chrome binary from Google's
[Chrome for Testing][cft] (CFT) distribution and hands back a path you
can pass to `BrowserBuilder::executable`. Useful in CI runners that
don't ship Chrome, in containers, or whenever you want a version pinned
independently of the host's Chrome install.

[cft]: https://googlechromelabs.github.io/chrome-for-testing/

Enable it in `Cargo.toml`:

```toml
[dependencies]
zendriver = { version = "0.1", features = ["fetcher"] }
```

Two entry points:

| Entry point | When to use |
|-------------|-------------|
| [`BrowserBuilder::ensure_chrome`] | Common case: just download Chrome and launch. One line, no configuration. |
| [`Fetcher`] (builder) | Pin a version / channel, customize the cache dir, register progress callbacks. |

[`BrowserBuilder::ensure_chrome`]: https://docs.rs/zendriver/latest/zendriver/struct.BrowserBuilder.html#method.ensure_chrome
[`Fetcher`]: https://docs.rs/zendriver/latest/zendriver/struct.Fetcher.html

## The one-liner

For the common "I just want Chrome" path:

```rust,no_run
# async fn ex() -> zendriver::Result<()> {
let browser = zendriver::Browser::builder()
    .ensure_chrome().await?
    .launch().await?;
# Ok(()) }
```

`ensure_chrome` resolves the latest stable CFT version for the host
platform, downloads + extracts it on cache miss, and points the
[`BrowserBuilder`] at the resulting binary. On a cache hit the call
returns in milliseconds and skips the network.

## The full builder

[`Fetcher::new`] returns a builder with sensible defaults. Configure as
needed, then call [`ensure_chrome`]:

```rust,no_run
{{#include ../../../crates/zendriver/examples/fetcher_demo.rs}}
```

[`ensure_chrome`]: https://docs.rs/zendriver/latest/zendriver/struct.Fetcher.html#method.ensure_chrome
[`Fetcher::new`]: https://docs.rs/zendriver/latest/zendriver/struct.Fetcher.html#method.new

Customization points:

- **`.version(VersionSpec)`** — pin a release.
  - `VersionSpec::Latest` — the newest entry in the manifest (default).
  - `VersionSpec::Stable` — alias for `Latest` today; will diverge if /
    when CFT exposes a stable-channel JSON.
  - `VersionSpec::Channel(Channel::Stable | Channel::Beta | Channel::Dev | Channel::Canary)` —
    `Stable` resolves through the same flat manifest as `Latest`; `Beta` /
    `Dev` / `Canary` resolve through Chrome for Testing's separate
    per-channel `last-known-good-versions-with-downloads.json` endpoint.
  - `VersionSpec::Explicit("126.0.6478.182".into())` — exact version
    string from the manifest.
- **`.platform(Platform)`** — override [`Platform::auto_detect`]. Useful
  for cross-compiling docker images for a different host arch.
- **`.cache_dir(path)`** — override the default cache root. Point at a
  shared CI volume so multiple jobs share one download.
- **`.on_progress(cb)`** — receive a [`FetcherProgress`] snapshot on
  every phase transition + per-chunk during download.

[`Platform::auto_detect`]: https://docs.rs/zendriver/latest/zendriver/enum.Platform.html#method.auto_detect
[`FetcherProgress`]: https://docs.rs/zendriver/latest/zendriver/struct.FetcherProgress.html

## Cache layout

Downloads land in the OS-conventional cache dir under `zendriver/chrome`:

- **Linux** — `${XDG_CACHE_HOME:-$HOME/.cache}/zendriver/chrome/`
- **macOS** — `~/Library/Caches/zendriver/chrome/`
- **Windows** — `%LOCALAPPDATA%\zendriver\chrome\`

Inside, each version gets its own subdirectory matching the CFT zip
layout verbatim:

```text
<cache_dir>/
  126.0.6478.182/
    chrome-linux64/
      chrome                                            (Linux)
    chrome-win64/
      chrome.exe                                        (Windows)
    chrome-mac-arm64/
      Google Chrome for Testing.app/Contents/MacOS/...  (macOS Apple Silicon)
```

Writes are atomic. The fetcher downloads + extracts into a
`<version>.tmp/` sibling, then a single `rename` promotes it to
`<version>/`. Crashing mid-download leaves a `.tmp/` that the next run
detects, deletes, and retries — no half-extracted binaries ever appear
under the canonical name.

## Progress callbacks

[`FetcherProgress`] carries:

- `phase` — one of [`Resolving`][res] / [`Downloading`][dl] /
  [`Extracting`][ex] / [`Verifying`][v] / [`Done`][d].
- `downloaded` / `total: Option<u64>` — bytes for the current phase,
  with `total` populated during `Downloading` from the
  `Content-Length` header.

[res]: https://docs.rs/zendriver/latest/zendriver/enum.FetcherPhase.html#variant.Resolving
[dl]: https://docs.rs/zendriver/latest/zendriver/enum.FetcherPhase.html#variant.Downloading
[ex]: https://docs.rs/zendriver/latest/zendriver/enum.FetcherPhase.html#variant.Extracting
[v]: https://docs.rs/zendriver/latest/zendriver/enum.FetcherPhase.html#variant.Verifying
[d]: https://docs.rs/zendriver/latest/zendriver/enum.FetcherPhase.html#variant.Done

The callback runs on Tokio worker threads. Render to a TUI / progress
bar inside it; heavier work (e.g. logging via I/O) should
`spawn_blocking` itself off the runtime to avoid stalling the download
task.

```rust,ignore
use indicatif::{ProgressBar, ProgressStyle};
use zendriver::{Fetcher, FetcherPhase};

let bar = ProgressBar::new(0);
let path = Fetcher::new()
    .on_progress(move |p| {
        if p.phase == FetcherPhase::Downloading {
            if let Some(t) = p.total { bar.set_length(t); }
            bar.set_position(p.downloaded);
        }
    })
    .ensure_chrome()
    .await?;
```

## CI use case

The motivating workflow: GitHub Actions / GitLab / etc runners that
don't have Chrome installed. Skipping Chrome from the system image and
letting the fetcher download Chrome inside the job has three wins:

1. **Reproducibility.** Pin `VersionSpec::Explicit(...)` so the same
   Chrome runs everywhere. No surprises when the runner image bumps.
2. **Smaller base images.** Don't bake Chrome into a hot container image
   if only a fraction of jobs need it.
3. **Parallel cache.** Point the fetcher at a runner-side volume (CFT
   binaries are ~150 MB compressed; one download serves every job).

A minimal `.github/workflows/test.yml` snippet:

```yaml
- uses: actions/cache@v4
  with:
    path: ~/.cache/zendriver/chrome
    key: zendriver-chrome-${{ runner.os }}-126.0.6478.182
- run: cargo test --features fetcher
```

`actions/cache` rehydrates the cache dir; the fetcher detects the cache
hit and skips the download. First run takes ~30 s on GitHub's free
runners; cached runs take &lt;1 s in `ensure_chrome`.

## When NOT to use it

- **You already have Chrome on the host** and don't care about
  version-pinning — the built-in PATH discovery is faster.
- **Network-restricted environments** that can't reach
  `https://googlechromelabs.github.io` or the CFT CDN — pre-populate
  the cache out-of-band or ship a Docker image with Chrome baked in.
- **You need Chrome stable on Linux ARM64** — CFT doesn't ship a
  `linux-arm64` build today;
  [`Platform::auto_detect`] returns `None` on that host and
  `ensure_chrome` errors out.
