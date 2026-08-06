# Fetcher

The `fetcher` Cargo feature downloads a Chromium build and hands back a
path you can pass to `BrowserBuilder::executable`. Useful in CI runners
that don't ship Chrome, in containers, or whenever you want a version
pinned independently of the host's Chrome install.

It fetches Google's [Chrome for Testing][cft] (CFT) by default; two other
distributions are available, described under
[Distributions](#distributions).

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
- **`.distribution(Distribution)`** — choose which Chromium build to
  fetch. Defaults to `ChromeForTesting`; see below.

[`Platform::auto_detect`]: https://docs.rs/zendriver/latest/zendriver/enum.Platform.html#method.auto_detect
[`FetcherProgress`]: https://docs.rs/zendriver/latest/zendriver/struct.FetcherProgress.html

## Distributions

`Distribution::default()` is `ChromeForTesting`, so everything above
describes what you get without touching this knob. Only *resolution*
varies between distributions — download, integrity check, unpacking and
the atomic cache are one shared path.

| Distribution | Index | Keyed by | Packaging |
|---|---|---|---|
| `ChromeForTesting` | one JSON manifest | version | zip (all platforms) |
| `UngoogledChromium` | three GitHub repos, one per OS | version (tag prefix) | zip / AppImage / dmg |
| `ChromiumSnapshot` | a GCS bucket per platform | **revision** | zip (all platforms) |

```rust,no_run
# async fn ex() -> Result<(), zendriver::FetcherError> {
use zendriver::{Distribution, Fetcher, VersionSpec};

let chromium = Fetcher::new()
    .distribution(Distribution::UngoogledChromium)
    .version(VersionSpec::Explicit("151.0.7922.71".into()))
    .ensure_chrome()
    .await?;
# let _ = chromium; Ok(()) }
```

### ungoogled-chromium

Chromium with Google integration stripped out. There is no single
manifest: binaries come from three independent per-OS repos under the
`ungoogled-software` org, each releasing on its own cadence.

- Tags carry a packaging suffix (`151.0.7922.71-1.1`), so
  `VersionSpec::Explicit("151.0.7922.71")` matches on the version
  **prefix** rather than by equality.
- **Availability differs per platform, and that is not a bug.** On
  2026-08-06 Windows and Linux were on `151.0.7922.71` while macOS was
  still on `150.0.7871.46`. `list_builds` answers the question for the
  platform you actually asked about.
- Packaging differs too: Windows a zip, Linux an AppImage (taken over
  the `.tar.xz` so no xz decompressor enters the dependency graph), and
  macOS a `.dmg`. Disk images are unpacked with `hdiutil`, so
  ungoogled-on-macOS can only be fetched **from** a macOS host; every
  other combination is cross-platform.
- Release lookups hit `api.github.com`, which allows **60 requests per
  hour** unauthenticated. Set `GITHUB_TOKEN` to raise that to 5000; the
  rate-limit error names both.

### Chromium snapshots

Per-commit continuous builds from Google's GCS bucket, laid out
`<platform>/<revision>/chrome-<os>.zip`.

**Snapshots are keyed by revision, not by version**, and the bucket
publishes no version index — so there is nothing to look
`151.0.7922.76` up in. `VersionSpec::Explicit` is therefore *refused*
here rather than quietly resolved to the newest snapshot, since handing
back a different browser than the one requested is worse than an error.
Pin one with `VersionSpec::Revision(1674890)`, or take the tip with
`VersionSpec::Latest`.

Snapshots also use their own platform spelling (`Mac_Arm`, `Win_x64`,
`Linux_x64`) rather than CFT's (`mac-arm64`, `win64`, `linux64`); the
translation is internal, and you keep using the CFT names.

### Listing what is available

```rust,no_run
# async fn ex() -> Result<(), zendriver::FetcherError> {
use zendriver::{Distribution, Platform, list_builds};

for build in list_builds(Distribution::UngoogledChromium, Platform::MacArm64).await? {
    println!("{}", build.label);
}
# Ok(()) }
```

Snapshots return a single entry — the tip named by `LAST_CHANGE`. Older
snapshots remain reachable, but only by revision.

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

The other distributions namespace themselves one level deeper:

```text
<cache_dir>/
  ungoogled/151.0.7922.71/...
  snapshot/r1674890/...
```

CFT stays un-prefixed so caches populated by earlier versions of the
crate keep hitting. The others are prefixed because a Chrome version
number is not unique across distributions — CFT `151.0.7922.71` and
ungoogled `151.0.7922.71` are different binaries, and a shared directory
would serve whichever landed first.

Writes are atomic. The fetcher downloads + unpacks into a
`<build>.tmp/` sibling, then a single `rename` promotes it to
`<build>/`. Crashing mid-download leaves a `.tmp/` that the next run
detects, deletes, and retries — no half-extracted binaries ever appear
under the canonical name.

## The `zendriver-fetch` CLI

The same resolver, as a standalone binary:

```bash
cargo install zendriver-fetcher --features cli
```

The `cli` feature is off by default — this is a library first, and
`clap` has no business in the dependency graph of a crate that only
downloads a browser.

Fully specified, it never prompts and exits non-zero on failure, which
is what CI needs:

```bash
zendriver-fetch --distribution cft --version 146.0.7680.153 \
                --platform mac-arm64 --out ./chrome
```

It prints the resolved binary path on stdout and progress on stderr, so
`CHROME=$(zendriver-fetch ... )` works. `--quiet` drops the progress.

Run it with a distribution or version missing and it turns interactive:
it asks which distribution, lists the builds that distribution really
publishes for the resolved platform (newest first, paged), and confirms
before downloading.

**If stdin is not a terminal, it refuses to prompt** and prints the
flags it needed instead. A CLI that blocks on stdin inside CI does not
fail — it hangs until the job times out.

| Flag | Meaning |
|---|---|
| `-d, --distribution` | `cft`, `ungoogled`, or `snapshot` |
| `--version` | Browser version, or `latest` |
| `--revision` | Chromium revision (snapshots only; conflicts with `--version`) |
| `-p, --platform` | `linux64`, `mac-x64`, `mac-arm64`, `win32`, `win64`; defaults to this host |
| `-o, --out` | Cache directory; defaults to the OS cache dir |
| `-q, --quiet` | Suppress progress output |

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
  `ensure_chrome` errors out. (ungoogled-chromium's portablelinux repo
  *does* publish arm64 AppImages, but `Platform` has no `LinuxArm64`
  variant yet, so they aren't selectable.)
- **You want ungoogled-chromium for macOS from a Linux or Windows
  box** — it ships as a `.dmg`, and unpacking one needs macOS's
  `hdiutil`.
