# Contributing

Issues and PRs welcome.

## Naming policy

- Builder pattern for configurable APIs (`tab.find().css("...").one().await`).
- `_fast` suffix for "skip realism, prefer speed" variants of input methods.
- `_main` suffix only for "in main world" JS evaluation (not a quality suffix).
- Avoid `_raw`, `_simple`, `_default` suffixes (ambiguous).

## Adding features

Each optional surface gates behind a Cargo feature flag (see
zendriver/Cargo.toml [features]). Examples in `crates/zendriver/examples/`
gated via `required-features`.

## Tests

- Unit tests: `cargo test --workspace --lib --locked`
- Integration tests: `cargo test --workspace --features integration-tests
  --test '*' --locked -- --test-threads=1` (requires Chrome installed)
- Nightly stealth: cron `0 6 * * *` against sannysoft + areyouheadless

## Releases

`scripts/publish.sh --dry-run` for verification; `scripts/publish.sh` for
actual publish (after `git tag vX.Y.Z`).
