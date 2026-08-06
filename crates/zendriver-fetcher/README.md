# zendriver-fetcher

Chromium binary downloader for zendriver. Resolves a version against a
distribution's index, downloads it, unpacks it, and caches it under a
per-build directory.

Three distributions, differing only in how a version turns into a URL:

| Distribution | Index | Keyed by |
|---|---|---|
| Chrome for Testing (default) | one JSON manifest | version |
| ungoogled-chromium | three GitHub repos, one per OS | version (tag prefix) |
| Chromium snapshots | a GCS bucket per platform | revision |

## Command line

```bash
cargo install zendriver-fetcher --features cli

zendriver-fetch --distribution cft --version 146.0.7680.153 \
                --platform mac-arm64 --out ./chrome
```

Given every flag it never prompts and exits non-zero on failure. Leave a flag
out at a terminal and it lists what the distribution actually publishes for
your platform; outside a terminal it refuses to prompt and names the missing
flags rather than hanging a CI job.

The `cli` feature is off by default — this is a library first.

Part of the [zendriver-rs](https://github.com/TurtIeSocks/zendriver-rs) workspace.
See the [`zendriver`](https://crates.io/crates/zendriver) crate and the
[user guide](https://turtiesocks.github.io/zendriver-rs/) for full documentation.
